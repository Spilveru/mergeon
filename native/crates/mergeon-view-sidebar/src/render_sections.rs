//! Las secciones del sidebar: `SidebarBranchSection.vue`,
//! `SidebarRemoteSection.vue`, `SidebarTagSection.vue`,
//! `SidebarStashSection.vue` y `SidebarSubmoduleSection.vue`.

use gpui_kit::component::Icon;
use gpui_kit::prelude::*;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ClickEvent, Context, FontWeight, MouseButton, SharedString,
    TestSupportExt as _, WeakEntity, Window, div, px, rems,
};
use mergeon_backend::Submodule;
use mergeon_git_query::SubmoduleState;
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, AppIcon, IconTone, Lucide, MergeonStyled as _, Radius};

use crate::menus::{MenuEntry, SidebarCommand};
use crate::section;
use crate::sidebar::{RemoteRow, RepoSnapshot, Section, Sidebar};
use crate::virtual_rows::{RowHeights, VirtualRows, virtual_rows};

/// Construye las filas `range` de un tramo a partir del frame.
type RowsFn =
    fn(&mut Sidebar, Range<usize>, &RepoSnapshot, &mut Context<Sidebar>) -> Vec<AnyElement>;

/// La estrella rellena de una rama destacada (`class="fill-current"`): el
/// catálogo solo trae el contorno.
const STAR_FILLED_SVG: &[u8] = include_bytes!("../assets/star-filled.svg");

/// La rama que se arrastra sobre otra para abrir Branch Actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraggedBranch(pub String);

/// Lo que acompaña al cursor mientras se arrastra una rama (la
/// `custom-drag-image` del original).
struct BranchDragPreview {
    name: SharedString,
}

impl Render for BranchDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .flex()
            .items_center()
            .px(rems(0.7))
            .py(rems(0.45))
            .bg(palette.popover)
            .text_color(palette.popover_foreground)
            .border_1()
            .border_color(with_alpha(palette.primary, 0.35))
            .radius(Radius::Md, cx)
            .text_size(rems(0.75))
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .shadow(mergeon_ui::style::floating_shadow(cx))
            .child(self.name.clone())
    }
}

fn row_id(kind: &str, key: &str) -> SharedString {
    SharedString::from(format!("{kind}:{key}"))
}

/// Entradas de `mergeon_ui` a partir de la descripción del menú.
pub(crate) fn menu_items(sidebar: &WeakEntity<Sidebar>, entries: Vec<MenuEntry>) -> Vec<MenuItem> {
    entries
        .into_iter()
        .map(|entry| match entry {
            MenuEntry::Separator => MenuItem::separator(),
            MenuEntry::Item {
                label,
                icon,
                disabled,
                danger,
                command,
            } => {
                let mut item = MenuItem::new(label).icon(icon).disabled(disabled);
                if danger {
                    item = item.danger();
                }
                if let Some(command) = command {
                    let sidebar = sidebar.clone();
                    item = item.on_click(move |window, cx| {
                        sidebar
                            .update(cx, |this, cx| this.run_command(command.clone(), window, cx))
                            .ok();
                    });
                }
                item
            }
        })
        .collect()
}

fn submodule_state_label(submodule: &Submodule) -> &'static str {
    match submodule.state {
        SubmoduleState::Ok => "Checked out at the recorded commit",
        SubmoduleState::Uninitialized => "Not initialized yet",
        SubmoduleState::OutOfSync => "Checked out at a different commit than the recorded one",
        SubmoduleState::Conflict => "Has merge conflicts",
    }
}

/// `submoduleTooltip`.
pub(crate) fn submodule_tooltip(submodule: &Submodule) -> String {
    let describe = submodule
        .describe
        .as_deref()
        .filter(|describe| !describe.is_empty())
        .map(|describe| format!(" — {describe}"))
        .unwrap_or_default();
    format!(
        "{}: {}{describe}",
        submodule.path,
        submodule_state_label(submodule)
    )
}

impl Sidebar {
    fn command_listener(
        command: SidebarCommand,
        cx: &Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _, window, cx| this.run_command(command.clone(), window, cx))
    }

    /// El botón de estrella de una rama (`.action-btn-star`).
    fn star_button(
        id: SharedString,
        branch_name: &str,
        starred: bool,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent = cx.palette().accent;
        let icon = if starred {
            AppIcon::from_icon(Icon::default().data(STAR_FILLED_SVG)).tone(IconTone::Active)
        } else {
            mergeon_ui::icon(Lucide::Star).tone(IconTone::Muted)
        };
        let command = SidebarCommand::ToggleStarBranch(branch_name.to_string());
        div()
            .id(id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded_full()
            .cursor_pointer()
            .hover(|style| style.bg(accent))
            .tooltip(mergeon_ui::tooltip(tooltip))
            .child(icon.size(px(12.)))
            .on_click(cx.listener(move |this, _, window, cx| {
                // `@click.stop`: no navega ni hace checkout.
                cx.stop_propagation();
                this.run_command(command.clone(), window, cx);
            }))
            .test_support()
    }

    /// Fila de rama: más densa que las demás. `exists == false` es una
    /// destacada cuya rama se borró: ni se ve ni se desprotege, así que no
    /// ofrece ni clic ni su tooltip.
    fn branch_row(
        &self,
        id: SharedString,
        active: bool,
        exists: bool,
        cx: &App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let palette = cx.palette();
        section::item(id, cx)
            .pr(rems(0.65))
            .pl(rems(1.25))
            .when(exists, |row| row.cursor_pointer())
            // Señal: la rama actual no se rellena; la marcan el cuadrado
            // naranja y el peso (ver `branch_name`).
            .when(!active, |row| {
                row.hover(|style| style.bg(palette.sidebar_row_hover))
            })
            .when(active, |row| row.text_color(palette.sidebar_foreground))
            .when(self.row_tooltips && exists, |row| {
                row.tooltip(mergeon_ui::tooltip(
                    "Click to view · Double-click to checkout",
                ))
            })
    }

    /// El nombre de una rama; el de la rama activa va en peso medio. El peso
    /// se pone aquí y no en la fila porque el menú contextual, que cuelga de
    /// ella, lo heredaría.
    fn branch_name(name: &str, active: bool) -> gpui_kit::Div {
        div()
            .min_w_0()
            .truncate()
            .when(active, |this| this.font_weight(FontWeight::SEMIBOLD))
            .child(name.to_string())
    }

    /// Clic: ver el tip de la rama en el historial. Doble clic: checkout.
    fn branch_click_listener(
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let name = branch_name.to_string();
        cx.listener(
            move |this, event: &ClickEvent, window, cx| match event.click_count() {
                1 => this.view_branch(&name, cx),
                2 => this.run_command(SidebarCommand::CheckoutBranch(name.clone()), window, cx),
                _ => {}
            },
        )
    }

    fn local_branch_context_menu(
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static {
        let sidebar = cx.entity().downgrade();
        let name = branch_name.to_string();
        move |_, cx| {
            let entries = sidebar
                .upgrade()
                .map(|entity| entity.read(cx).local_branch_menu(&name, cx))
                .unwrap_or_default();
            menu_items(&sidebar, entries)
        }
    }

    fn render_starred_row(
        &self,
        snapshot: &RepoSnapshot,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let branch_name = snapshot.data.starred_branches[index].as_str();
        let active = branch_name == snapshot.current_branch;
        let viewing = snapshot.viewed_ref.as_deref() == Some(branch_name);
        let group = SharedString::from(format!("starred-row-{branch_name}"));
        // Sin la rama real (se borró y sigue destacada) no hay menú que mostrar.
        let has_menu = snapshot.data.starred_exists[index];

        let row = self
            .branch_row(row_id("starred", branch_name), active, has_menu, cx)
            .group(group.clone())
            .map(|row| {
                if viewing {
                    section::viewing(row, 1.25, cx)
                } else {
                    row
                }
            })
            // Una destacada que ya no existe no se puede ver ni desproteger:
            // el doble clic solo daría un error.
            .when(has_menu, |row| {
                row.on_click(Self::branch_click_listener(branch_name, cx))
            })
            .when(!has_menu, |row| row.opacity(0.6))
            .child(if active {
                section::head_mark(cx).into_any_element()
            } else {
                section::item_icon(Lucide::GitBranch, IconTone::Muted).into_any_element()
            })
            .child(Self::branch_name(branch_name, active))
            .child(
                // Acciones reveladas al pasar por la fila.
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .ml_auto()
                    .opacity(0.)
                    .group_hover(group, |style| style.opacity(1.))
                    .child(Self::star_button(
                        row_id("unstar", branch_name),
                        branch_name,
                        true,
                        "Unstar branch",
                        cx,
                    )),
            )
            .test_support();

        if has_menu {
            row.context_menu_items(Self::local_branch_context_menu(branch_name, cx))
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    fn render_branch_row(
        &self,
        snapshot: &RepoSnapshot,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let branch = &snapshot.data.branches[index];
        let is_starred = snapshot.data.branch_starred[index];
        let name = branch.name.clone();
        let viewing = snapshot.viewed_ref.as_deref() == Some(name.as_str());
        let dragged = self.dragged_branch.as_deref() == Some(name.as_str());
        let sidebar = cx.entity().downgrade();

        self.branch_row(row_id("branch", &name), branch.active, true, cx)
            .map(|row| {
                if viewing {
                    section::viewing(row, 1.25, cx)
                } else {
                    row
                }
            })
            .when(branch.is_remote, |row| row.opacity(0.7))
            .when(dragged, |row| row.opacity(0.5))
            .on_click(Self::branch_click_listener(&name, cx))
            .on_drag(DraggedBranch(name.clone()), move |dragged, _, _, cx| {
                let name = dragged.0.clone();
                sidebar
                    .update(cx, |this, cx| {
                        this.dragged_branch = Some(name.clone());
                        cx.notify();
                    })
                    .ok();
                cx.new(|_| BranchDragPreview { name: name.into() })
            })
            .drag_over::<DraggedBranch>({
                let name = name.clone();
                move |style, dragged, _, _| {
                    if dragged.0 == name {
                        return style;
                    }
                    // `outline: 1px dashed var(--primary); outline-offset: -2px`:
                    // un borde, con el relleno 1px más corto para no mover la fila.
                    style
                        .border_1()
                        .border_dashed()
                        .border_color(palette.primary)
                        .pt(rems(0.2 - 0.0625))
                        .pb(rems(0.2 - 0.0625))
                        .pr(rems(0.65 - 0.0625))
                        .pl(rems(1.25 - 0.0625))
                }
            })
            .on_drop(cx.listener({
                let name = name.clone();
                move |this, dragged: &DraggedBranch, window, cx| {
                    this.drop_branch(&dragged.0, &name, window, cx);
                }
            }))
            .child(if branch.active {
                section::head_mark(cx).into_any_element()
            } else {
                section::item_icon(Lucide::GitBranch, IconTone::Muted).into_any_element()
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(rems(0.35))
                    .child(Self::branch_name(&name, branch.active))
                    .when(branch.has_tracking, |this| {
                        this.child(
                            mergeon_ui::icon(Lucide::Cloud)
                                .size(px(12.))
                                .tone(IconTone::Active)
                                .opacity(0.8),
                        )
                    })
                    // Desprotegida en otro worktree (no se puede hacer checkout aquí).
                    .when_some(self.branch_worktree(&name), |this, path| {
                        this.child(
                            div()
                                .id(row_id("branch-worktree", &name))
                                .test_support()
                                .flex()
                                .flex_shrink_0()
                                .child(
                                    mergeon_ui::icon(Lucide::FolderGit2)
                                        .size(px(12.))
                                        .tone(IconTone::Muted),
                                )
                                .tooltip(mergeon_ui::tooltip(format!(
                                    "Checked out in the worktree at {path}"
                                ))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .ml_auto()
                    .gap(rems(0.3))
                    .when(branch.ahead > 0, |this| {
                        this.child(
                            div()
                                .id(row_id("ahead", &name))
                                .text_size(rems(0.6875))
                                .font_weight(FontWeight::MEDIUM)
                                .px(rems(0.24))
                                .py(rems(0.05))
                                .rounded(Radius::Sm.px(cx))
                                .bg(with_alpha(palette.success, 0.12))
                                .text_color(palette.success)
                                .tooltip(mergeon_ui::tooltip(format!(
                                    "{} commits ahead",
                                    branch.ahead
                                )))
                                .child(format!("↑{}", branch.ahead)),
                        )
                    })
                    .when(branch.behind > 0, |this| {
                        this.child(
                            div()
                                .id(row_id("behind", &name))
                                .text_size(rems(0.6875))
                                .font_weight(FontWeight::MEDIUM)
                                .px(rems(0.24))
                                .py(rems(0.05))
                                .rounded(Radius::Sm.px(cx))
                                .bg(with_alpha(palette.warning, 0.14))
                                .text_color(palette.warning)
                                .tooltip(mergeon_ui::tooltip(format!(
                                    "{} commits behind",
                                    branch.behind
                                )))
                                .child(format!("↓{}", branch.behind)),
                        )
                    })
                    .child(Self::star_button(
                        row_id("star", &name),
                        &name,
                        is_starred,
                        if is_starred {
                            "Unstar branch"
                        } else {
                            "Star branch"
                        },
                        cx,
                    ))
                    .when(branch.active, |this| {
                        this.child(
                            mergeon_ui::icon(Lucide::Check)
                                .size(px(12.))
                                .tone(IconTone::Active),
                        )
                    }),
            )
            .test_support()
            .context_menu_items(Self::local_branch_context_menu(&name, cx))
            .into_any_element()
    }

    /// Un tramo de filas virtuales de una sección: solo se construyen las que
    /// quedan a la vista (ver [`crate::virtual_rows`]).
    pub(crate) fn section_rows(
        id: &'static str,
        heights: RowHeights,
        snapshot: &Rc<RepoSnapshot>,
        rows: RowsFn,
        cx: &mut Context<Self>,
    ) -> VirtualRows {
        let view = cx.entity().downgrade();
        let snapshot = snapshot.clone();
        virtual_rows(id, heights, move |range, _, cx| {
            view.update(cx, |this, cx| rows(this, range, &snapshot, cx))
                .unwrap_or_default()
        })
    }

    fn starred_rows(
        &mut self,
        range: Range<usize>,
        snapshot: &RepoSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .map(|index| self.render_starred_row(snapshot, index, cx))
            .collect()
    }

    fn branch_rows(
        &mut self,
        range: Range<usize>,
        snapshot: &RepoSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filtered = snapshot.filtered_branches.clone();
        filtered[range]
            .iter()
            .map(|index| self.render_branch_row(snapshot, *index, cx))
            .collect()
    }

    /// `SidebarBranchSection.vue`: Starred y Branches.
    pub(crate) fn render_branch_sections(
        &self,
        snapshot: &Rc<RepoSnapshot>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = *cx.palette();
        let starred_expanded = self.section_shown_expanded(Section::Starred);
        let branches_expanded = self.section_shown_expanded(Section::Branches);
        let starred_count = snapshot.data.starred_branches.len();
        let branch_count = snapshot.filtered_branches.len();
        let branches_loading = snapshot.data.first_load.branches;
        let row_height = section::row_height(window.rem_size());

        let starred = section::section()
            .child(
                section::header("starred", cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_section(Section::Starred, cx);
                    }))
                    .child(section::chevron(starred_expanded))
                    .child(section::title("Starred", cx))
                    .child(section::count(starred_count, cx))
                    .test_support(),
            )
            .when(starred_expanded, |this| {
                this.child(
                    section::content(false)
                        .when(starred_count == 0, |content| {
                            content.child(section::empty("No starred branches", cx))
                        })
                        .when(starred_count > 0, |content| {
                            content.child(Self::section_rows(
                                "starred-rows",
                                RowHeights::uniform(starred_count, row_height),
                                snapshot,
                                Self::starred_rows,
                                cx,
                            ))
                        }),
                )
            });

        let branches = section::section()
            .child(
                section::header("branches", cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_section(Section::Branches, cx);
                    }))
                    .child(section::chevron(branches_expanded))
                    .child(section::title("Branches", cx))
                    .child(
                        section::header_action_with(
                            "cleanup-branches",
                            section::HeaderActionState {
                                // Con una operación a medias no se borran ramas.
                                enabled: snapshot.can_mutate,
                                busy: false,
                            },
                            Lucide::BrushCleaning,
                            "Clean up merged branches…",
                            "branches",
                            cx,
                            Self::command_listener(SidebarCommand::CleanupMergedBranches, cx),
                        )
                        .test_support(),
                    )
                    .child(if branches_loading {
                        section::count_loading()
                    } else {
                        section::count(branch_count, cx)
                    })
                    .test_support()
                    .context_menu_items({
                        let sidebar = cx.entity().downgrade();
                        move |_, cx| {
                            let entries = sidebar
                                .upgrade()
                                .map(|entity| entity.read(cx).branches_section_menu(cx))
                                .unwrap_or_default();
                            menu_items(&sidebar, entries)
                        }
                    }),
            )
            .when(branches_expanded, |this| {
                this.child(
                    section::content(true)
                        .when(branch_count == 0 && branches_loading, |content| {
                            content.child(section::empty("Loading branches…", cx))
                        })
                        .when(
                            branch_count == 0 && !branches_loading && !snapshot.filter_active,
                            |content| content.child(section::empty("No branches found", cx)),
                        )
                        // El filtro no deja ninguna: se dice y se ofrece quitarlo.
                        .when(branch_count == 0 && snapshot.filter_active, |content| {
                            content.child(
                                section::empty("No branches match the filter.", cx)
                                    .flex()
                                    .flex_wrap()
                                    .gap(rems(0.35))
                                    .child(
                                        div()
                                            .id("clear-branch-filter")
                                            .text_color(palette.primary)
                                            .cursor_pointer()
                                            .hover(|style| style.underline())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.set_filter_text("", window, cx);
                                            }))
                                            .child("Clear filter")
                                            .test_support(),
                                    ),
                            )
                        })
                        .when(!snapshot.detached_head_label.is_empty(), |content| {
                            content.child(
                                section::item("detached-head", cx)
                                    .pr(rems(0.65))
                                    .pl(rems(1.25))
                                    .cursor_default()
                                    .bg(with_alpha(palette.primary, 0.08))
                                    .border_t_1()
                                    .border_b_1()
                                    .border_color(with_alpha(palette.border, 0.6))
                                    .child(section::item_icon(
                                        Lucide::GitCommitVertical,
                                        IconTone::Active,
                                    ))
                                    .child(section::item_name(snapshot.detached_head_label.clone()))
                                    .test_support(),
                            )
                        })
                        .when(branch_count > 0, |content| {
                            content.child(Self::section_rows(
                                "branch-rows",
                                RowHeights::uniform(branch_count, row_height),
                                snapshot,
                                Self::branch_rows,
                                cx,
                            ))
                        }),
                )
            });

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(starred)
            .child(branches)
    }

    /// La fila de un remoto: nombre, número de ramas y su menú.
    fn render_remote_row(
        &self,
        snapshot: &RepoSnapshot,
        remote: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let remote = &snapshot.data.remotes[remote];
        let remote_name = remote.name.clone();
        section::item(row_id("remote", &remote_name), cx)
            .child(section::item_icon(Lucide::Cloud, IconTone::Muted))
            .child(section::item_name(remote_name.clone()).font_weight(FontWeight::SEMIBOLD))
            .child(section::count(remote.branches.len(), cx))
            .test_support()
            .context_menu_items({
                let sidebar = weak.clone();
                move |_, cx| {
                    let entries = sidebar
                        .upgrade()
                        .map(|entity| entity.read(cx).remote_menu(&remote_name, cx))
                        .unwrap_or_default();
                    menu_items(&sidebar, entries)
                }
            })
            .into_any_element()
    }

    /// Una rama de un remoto, sangrada bajo él.
    fn render_remote_branch_row(
        &self,
        snapshot: &RepoSnapshot,
        remote: usize,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let weak = cx.entity().downgrade();
        let remote_name = snapshot.data.remotes[remote].name.clone();
        let branch = &snapshot.data.remotes[remote].branches[index];
        let full_name = format!("{remote_name}/{branch}");
        let viewing = snapshot.viewed_ref.as_deref() == Some(full_name.as_str());
        // Sangrada bajo su remoto con su propio relleno: el resaltado (hover,
        // "viendo") llega de borde a borde como en las demás filas.
        let row = section::item(row_id("remote-branch", &full_name), cx)
            .pl(rems(2.6))
            .cursor_pointer()
            .hover(|style| style.bg(palette.sidebar_row_hover))
            .map(|row| {
                if viewing {
                    section::viewing(row, 2.6, cx)
                } else {
                    row
                }
            })
            .when(self.row_tooltips, |row| {
                row.tooltip(mergeon_ui::tooltip(format!(
                    "Click to view · Double-click to checkout {branch}"
                )))
            })
            .on_click(cx.listener({
                let remote_name = remote_name.clone();
                let branch = branch.clone();
                move |this, event: &ClickEvent, window, cx| match event.click_count() {
                    1 => this.view_remote_branch(&remote_name, &branch, cx),
                    2 => this.run_command(
                        SidebarCommand::CheckoutRemoteBranch {
                            remote: remote_name.clone(),
                            branch: branch.clone(),
                        },
                        window,
                        cx,
                    ),
                    _ => {}
                }
            }))
            .child(section::item_icon(Lucide::GitBranch, IconTone::Muted))
            .child(section::item_name(branch.clone()))
            .test_support()
            .context_menu_items({
                let sidebar = weak.clone();
                let remote_name = remote_name.clone();
                let branch = branch.clone();
                move |_, cx| {
                    let entries = sidebar
                        .upgrade()
                        .map(|entity| {
                            entity
                                .read(cx)
                                .remote_branch_menu(&remote_name, &branch, cx)
                        })
                        .unwrap_or_default();
                    menu_items(&sidebar, entries)
                }
            });
        row.into_any_element()
    }

    fn remote_rows(
        &mut self,
        range: Range<usize>,
        snapshot: &RepoSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        snapshot.data.remote_rows[range]
            .iter()
            .map(|row| match *row {
                RemoteRow::Remote(remote) => self.render_remote_row(snapshot, remote, cx),
                RemoteRow::Branch(remote, index) => {
                    self.render_remote_branch_row(snapshot, remote, index, cx)
                }
            })
            .collect()
    }

    /// `SidebarRemoteSection.vue`.
    pub(crate) fn render_remote_section(
        &self,
        snapshot: &Rc<RepoSnapshot>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = *cx.palette();
        let expanded = self.section_shown_expanded(Section::Remotes);
        let remote_count = snapshot.data.remotes.len();
        let loading = snapshot.data.first_load.remotes;

        section::section()
            .child(
                section::header("remotes", cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_section(Section::Remotes, cx);
                    }))
                    // `@contextmenu.prevent="emit('add-remote')"`
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, window, cx| {
                            this.run_command(SidebarCommand::AddRemote, window, cx);
                        }),
                    )
                    .child(section::chevron(expanded))
                    .child(section::title("Remotes", cx))
                    .child(
                        section::header_action(
                            "add-remote",
                            Lucide::Plus,
                            "Add Remote",
                            "remotes",
                            cx,
                            Self::command_listener(SidebarCommand::AddRemote, cx),
                        )
                        .test_support(),
                    )
                    .child(if loading {
                        section::count_loading()
                    } else {
                        section::count(remote_count, cx)
                    })
                    .test_support(),
            )
            .when(expanded, |this| {
                this.child(if loading {
                    section::content(false).child(section::empty("Loading remotes…", cx))
                } else if remote_count == 0 {
                    section::content(false).child(
                        section::empty("No remotes configured", cx)
                            .flex()
                            .flex_col()
                            .gap(rems(0.625))
                            .child(
                                div().flex().child(
                                    div()
                                        .id("add-remote-empty")
                                        .border_1()
                                        .border_color(palette.border)
                                        .rounded(Radius::Md.px(cx))
                                        .bg(palette.card)
                                        .text_color(palette.foreground)
                                        .px(rems(0.6))
                                        .py(rems(0.35))
                                        .text_size(rems(0.75))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style
                                                .bg(palette.sidebar_row_hover)
                                                .border_color(with_alpha(palette.primary, 0.45))
                                        })
                                        .on_click(Self::command_listener(
                                            SidebarCommand::AddRemote,
                                            cx,
                                        ))
                                        .child("Add remote")
                                        .test_support(),
                                ),
                            ),
                    )
                } else {
                    section::content(false).child(Self::section_rows(
                        "remote-rows",
                        snapshot.data.remote_heights.clone(),
                        snapshot,
                        Self::remote_rows,
                        cx,
                    ))
                })
            })
    }

    fn tag_rows(
        &mut self,
        range: Range<usize>,
        snapshot: &RepoSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let weak = cx.entity().downgrade();
        let can_mutate = snapshot.can_mutate;
        snapshot.data.tags[range]
            .iter()
            .map(|tag| {
                let group = section::row_group("tag", tag);
                section::item(row_id("tag", tag), cx)
                    .relative()
                    .group(group.clone())
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.sidebar_row_hover))
                    .when(self.row_tooltips, |row| {
                        row.tooltip(mergeon_ui::tooltip("Click to view this tag's commit"))
                    })
                    .on_click(cx.listener({
                        let tag = tag.clone();
                        move |this, _, _, cx| this.view_tag(&tag, cx)
                    }))
                    .child(section::item_icon(Lucide::Tag, IconTone::Muted))
                    .child(section::item_name(tag.clone()))
                    .child(
                        section::item_actions_on_hover(group, cx)
                            .child(
                                section::item_action(
                                    row_id("checkout-tag", tag),
                                    Lucide::GitBranch,
                                    "Checkout tag",
                                    false,
                                    can_mutate,
                                    cx,
                                    Self::command_listener(
                                        SidebarCommand::CheckoutTag(tag.clone()),
                                        cx,
                                    ),
                                )
                                .test_support(),
                            )
                            .child(
                                section::item_action(
                                    row_id("delete-tag", tag),
                                    Lucide::Trash,
                                    "Delete tag",
                                    true,
                                    can_mutate,
                                    cx,
                                    Self::command_listener(
                                        SidebarCommand::DeleteTag(tag.clone()),
                                        cx,
                                    ),
                                )
                                .test_support(),
                            ),
                    )
                    .test_support()
                    .context_menu_items({
                        let sidebar = weak.clone();
                        let tag = tag.clone();
                        move |_, cx| {
                            let entries = sidebar
                                .upgrade()
                                .map(|entity| entity.read(cx).tag_menu(&tag, cx))
                                .unwrap_or_default();
                            menu_items(&sidebar, entries)
                        }
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// `SidebarTagSection.vue`.
    pub(crate) fn render_tag_section(
        &self,
        snapshot: &Rc<RepoSnapshot>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.section_shown_expanded(Section::Tags);
        let tag_count = snapshot.data.tags.len();
        let loading = snapshot.data.first_load.tags;
        let row_height = section::row_height(window.rem_size());

        section::section()
            .child(
                section::header("tags", cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_section(Section::Tags, cx);
                    }))
                    .child(section::chevron(expanded))
                    .child(section::title("Tags", cx))
                    .child(
                        section::header_action_with(
                            "create-tag",
                            section::HeaderActionState {
                                enabled: snapshot.can_mutate,
                                busy: false,
                            },
                            Lucide::Plus,
                            "Create Tag",
                            "tags",
                            cx,
                            Self::command_listener(SidebarCommand::CreateTag, cx),
                        )
                        .test_support(),
                    )
                    .child(if loading {
                        section::count_loading()
                    } else {
                        section::count(tag_count, cx)
                    })
                    .test_support(),
            )
            .when(expanded, |this| {
                this.child(if loading {
                    section::content(true).child(section::empty("Loading tags…", cx))
                } else if tag_count == 0 {
                    section::content(true).child(section::empty("No tags found", cx))
                } else {
                    section::content(true).child(Self::section_rows(
                        "tag-rows",
                        RowHeights::uniform(tag_count, row_height),
                        snapshot,
                        Self::tag_rows,
                        cx,
                    ))
                })
            })
    }

    fn stash_rows(
        &mut self,
        range: Range<usize>,
        snapshot: &RepoSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let weak = cx.entity().downgrade();
        let can_mutate = snapshot.can_mutate;
        let row_tooltips = self.row_tooltips;
        snapshot.data.stashes[range]
            .iter()
            .map(|stash| {
                let hash = stash.hash.clone();
                let group = section::row_group("stash", &hash);
                let action = |kind: &str,
                              icon: Lucide,
                              tooltip: &'static str,
                              danger: bool,
                              command: SidebarCommand,
                              cx: &Context<Self>| {
                    section::item_action(
                        row_id(kind, &hash),
                        icon,
                        tooltip,
                        danger,
                        can_mutate,
                        cx,
                        Self::command_listener(command, cx),
                    )
                    .test_support()
                };

                section::item(row_id("stash", &hash), cx)
                    .relative()
                    .group(group.clone())
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.sidebar_row_hover))
                    .when(row_tooltips, |row| {
                        row.tooltip(mergeon_ui::tooltip("Click to inspect this stash"))
                    })
                    .on_click(Self::command_listener(
                        SidebarCommand::SelectStash(hash.clone()),
                        cx,
                    ))
                    .child(section::item_icon(Lucide::Archive, IconTone::Muted))
                    .child(section::item_name(if stash.message.is_empty() {
                        "Stash".to_string()
                    } else {
                        stash.message.clone()
                    }))
                    .child(
                        section::item_actions_on_hover(group, cx)
                            .child(action(
                                "stash-branch",
                                Lucide::GitBranchPlus,
                                "Create branch from stash",
                                false,
                                SidebarCommand::BranchFromStash(hash.clone()),
                                cx,
                            ))
                            .child(action(
                                "stash-apply",
                                Lucide::ArrowDownToLine,
                                "Apply stash",
                                false,
                                SidebarCommand::ApplyStash(hash.clone()),
                                cx,
                            ))
                            .child(action(
                                "stash-pop",
                                Lucide::PackageOpen,
                                "Pop stash",
                                false,
                                SidebarCommand::PopStash(hash.clone()),
                                cx,
                            ))
                            .child(action(
                                "stash-drop",
                                Lucide::Trash,
                                "Drop stash",
                                true,
                                SidebarCommand::DropStash(hash.clone()),
                                cx,
                            )),
                    )
                    .test_support()
                    .context_menu_items({
                        let sidebar = weak.clone();
                        let hash = hash.clone();
                        move |_, cx| {
                            let entries = sidebar
                                .upgrade()
                                .map(|entity| entity.read(cx).stash_menu(&hash, cx))
                                .unwrap_or_default();
                            menu_items(&sidebar, entries)
                        }
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// `SidebarStashSection.vue`.
    pub(crate) fn render_stash_section(
        &self,
        snapshot: &Rc<RepoSnapshot>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.section_shown_expanded(Section::Stashes);
        let stash_count = snapshot.data.stashes.len();
        let loading = snapshot.data.first_load.stashes;
        let row_height = section::row_height(window.rem_size());

        section::section()
            .child(
                section::header("stashes", cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_section(Section::Stashes, cx);
                    }))
                    .child(section::chevron(expanded))
                    .child(section::title("Stashes", cx))
                    .child(if loading {
                        section::count_loading()
                    } else {
                        section::count(stash_count, cx)
                    })
                    .test_support(),
            )
            .when(expanded, |this| {
                this.child(
                    section::content(false)
                        .when(loading, |content| {
                            content.child(section::empty("Loading stashes…", cx))
                        })
                        .when(stash_count == 0 && !loading, |content| {
                            content.child(section::empty("No stashes saved", cx))
                        })
                        .when(stash_count > 0, |content| {
                            content.child(Self::section_rows(
                                "stash-rows",
                                RowHeights::uniform(stash_count, row_height),
                                snapshot,
                                Self::stash_rows,
                                cx,
                            ))
                        }),
                )
            })
    }

    fn submodule_rows(
        &mut self,
        range: Range<usize>,
        snapshot: &RepoSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let row_tooltips = self.row_tooltips;
        snapshot.data.submodules[range]
            .iter()
            .map(|submodule| {
                let path = submodule.path.clone();
                let group = section::row_group("submodule", &path);
                let uninitialized = submodule.state == SubmoduleState::Uninitialized;
                let (icon, tone) = match submodule.state {
                    SubmoduleState::Uninitialized => (Lucide::CircleDashed, IconTone::Active),
                    SubmoduleState::OutOfSync => (Lucide::CircleAlert, IconTone::Active),
                    SubmoduleState::Conflict => (Lucide::CircleAlert, IconTone::Danger),
                    SubmoduleState::Ok => (Lucide::FolderGit2, IconTone::Muted),
                };

                section::item(row_id("submodule", &path), cx)
                    .relative()
                    .group(group.clone())
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.sidebar_row_hover))
                    .when(row_tooltips, |row| {
                        row.tooltip(mergeon_ui::tooltip(submodule_tooltip(submodule)))
                    })
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, event: &ClickEvent, window, cx| {
                            if event.click_count() == 2 {
                                this.run_command(
                                    SidebarCommand::OpenSubmodule(path.clone()),
                                    window,
                                    cx,
                                );
                            }
                        }
                    }))
                    .child(section::item_icon(icon, tone))
                    .child(section::item_name(path.clone()))
                    .child(
                        section::item_actions_on_hover(group, cx)
                            .child(
                                section::item_action(
                                    row_id("submodule-update", &path),
                                    Lucide::RefreshCw,
                                    if uninitialized {
                                        "Initialize submodule"
                                    } else {
                                        "Update submodule"
                                    },
                                    false,
                                    true,
                                    cx,
                                    Self::command_listener(
                                        SidebarCommand::UpdateSubmodule(path.clone()),
                                        cx,
                                    ),
                                )
                                .test_support(),
                            )
                            .when(!uninitialized, |actions| {
                                actions.child(
                                    section::item_action(
                                        row_id("submodule-open", &path),
                                        Lucide::ExternalLink,
                                        "Open as repository tab",
                                        false,
                                        true,
                                        cx,
                                        Self::command_listener(
                                            SidebarCommand::OpenSubmodule(path.clone()),
                                            cx,
                                        ),
                                    )
                                    .test_support(),
                                )
                            }),
                    )
                    .test_support()
                    .into_any_element()
            })
            .collect()
    }

    /// `SidebarSubmoduleSection.vue`.
    pub(crate) fn render_submodule_section(
        &self,
        snapshot: &Rc<RepoSnapshot>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.section_shown_expanded(Section::Submodules);
        let submodules = &snapshot.data.submodules;
        let loading = snapshot.data.submodules_loading;
        let row_height = section::row_height(window.rem_size());

        section::section()
            .child(
                section::header("submodules", cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_section(Section::Submodules, cx);
                    }))
                    .child(section::chevron(expanded))
                    .child(section::title("Submodules", cx))
                    .when(!submodules.is_empty(), |header| {
                        header.child(
                            section::header_action(
                                "update-all-submodules",
                                Lucide::RefreshCw,
                                "Update all submodules",
                                "submodules",
                                cx,
                                Self::command_listener(SidebarCommand::UpdateAllSubmodules, cx),
                            )
                            .test_support(),
                        )
                    })
                    .child(if loading && submodules.is_empty() {
                        section::count_loading()
                    } else {
                        section::count(submodules.len(), cx)
                    })
                    .test_support(),
            )
            .when(expanded, |this| {
                this.child(if loading {
                    section::content(false).child(section::empty("Loading submodules…", cx))
                } else if submodules.is_empty() {
                    section::content(false)
                        .child(section::empty("No submodules in this repository", cx))
                } else {
                    section::content(false).child(Self::section_rows(
                        "submodule-rows",
                        RowHeights::uniform(submodules.len(), row_height),
                        snapshot,
                        Self::submodule_rows,
                        cx,
                    ))
                })
            })
    }
}
