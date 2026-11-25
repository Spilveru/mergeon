//! Espacio de trabajo de un repositorio (`RepositoryWorkspace.vue`).
//!
//! Disposición estilo Fork: a la izquierda, fija en todas las vistas, la
//! columna con las dos filas de vista ("Local Changes" y "All Commits") sobre
//! el sidebar; a la derecha, la barra de documentos (archivos, diffs
//! históricos, historial…) y el panel de lo que esté activo. La barra de
//! documentos no existe mientras no haya ninguno abierto: el espacio vuelve
//! al contenido.

use std::collections::HashMap;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyView, ClickEvent, Context, Entity, FontWeight, MouseButton, ScrollHandle,
    SharedString, Subscription, Window, div, px,
};
use mergeon_app::AppModel;
use mergeon_model::workspace_tabs::{
    CHANGES_TAB, FileTab, FileTabDescriptor, HOME_TAB, RepositoryWorkspace, TabSource,
    file_tab_suffixes, path_breadcrumbs,
};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::style::text;
use mergeon_ui::{ActivePaletteExt, IconTone, Lucide, Radius};

use mergeon_view_changes::{ChangesMode, ChangesView};
use mergeon_view_commits::{CommitsView, CommitsViewEvent, HistoricalFileTabView};
use mergeon_view_sidebar::{Sidebar, SidebarResizer};

/// Alto de la barra de documentos.
pub const DOCUMENT_BAR_HEIGHT: f32 = 32.0;
/// Ancho máximo de una pestaña de documento (el nombre se corta).
const DOCUMENT_TAB_MAX_WIDTH: f32 = 240.0;
/// Íconos de las pestañas y de las filas de vista.
const TAB_ICON_SIZE: f32 = 14.0;

/// Subrayado de la pestaña activa. Va como hijo absoluto porque en GPUI un
/// elemento tiene un solo color de borde.
fn active_indicator(color: impl Into<gpui_kit::Hsla>) -> impl IntoElement {
    let color = color.into();
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(2.))
        .bg(color)
}

/// Línea de inserción de 2px al arrastrar una pestaña sobre otra: una sombra
/// interior desplazada, que no mueve el contenido como lo haría un borde.
pub(crate) fn insertion_marker(color: impl Into<gpui_kit::Hsla>) -> Vec<gpui_kit::BoxShadow> {
    vec![gpui_kit::BoxShadow {
        color: color.into(),
        offset: gpui_kit::point(px(2.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(0.),
        inset: true,
    }]
}

/// Una de las dos filas de vista de arriba del sidebar.
struct ViewRow {
    id: &'static str,
    tab: &'static str,
    label: &'static str,
    icon: Lucide,
    active: bool,
    /// Insignia con el número de cambios locales; 0 la oculta.
    count: usize,
}

/// Lo que el espacio de trabajo pinta del modelo: ruta y número de cambios
/// del repositorio, y el estado de sus pestañas.
type WorkspaceState = Option<(String, usize, RepositoryWorkspace)>;

fn workspace_state(model: &AppModel, repo_id: &str) -> WorkspaceState {
    let repo = model.repository(repo_id)?;
    Some((
        repo.path.clone(),
        repo.changes_count,
        model.repository_workspace(&repo.path),
    ))
}

/// Pestaña de documento que se está arrastrando.
#[derive(Clone)]
struct DraggedDocumentTab {
    index: usize,
    label: SharedString,
}

/// Lo que se ve bajo el puntero al arrastrar una pestaña de documento.
struct DocumentDragPreview {
    label: SharedString,
}

impl Render for DocumentDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .px_3()
            .py_1()
            .rounded(Radius::Xl.px(cx))
            .border_1()
            .border_color(palette.border_base)
            .bg(palette.bg_raised)
            .text_size(text::UI)
            .text_color(palette.text_1)
            .child(self.label.clone())
    }
}

pub struct RepositoryWorkspaceView {
    model: Entity<AppModel>,
    repo_id: String,
    shown: WorkspaceState,
    sidebar: Entity<Sidebar>,
    commits: Entity<CommitsView>,
    changes: Entity<ChangesView>,
    /// Paneles de las pestañas de archivo ya visitadas: siguen montados al
    /// ocultarse (las "ranuras" del original), por id de pestaña.
    file_panels: HashMap<String, AnyView>,
    file_changes: HashMap<String, Entity<ChangesView>>,
    file_history: HashMap<String, Entity<HistoricalFileTabView>>,
    files_open: bool,
    /// Scroll de la barra de documentos.
    file_tab_scroll: ScrollHandle,
    /// Pestaña activa (y cuántas había) que se trajo a la vista la última vez.
    scrolled_file_tab: Option<(String, usize)>,
    _subscriptions: Vec<Subscription>,
}

impl RepositoryWorkspaceView {
    pub fn new(
        repo_id: String,
        sidebar: Entity<Sidebar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = AppModel::global(cx);
        let commits = cx.new(|cx| CommitsView::new(repo_id.clone(), window, cx));
        let changes = cx.new(|cx| ChangesView::new(repo_id.clone(), ChangesMode::List, window, cx));
        let subscriptions = vec![
            // Solo repinta si cambia lo que pinta: el repositorio (ruta y
            // número de cambios) y sus pestañas. Las vistas de dentro vigilan
            // lo suyo.
            cx.observe(&model, |this: &mut Self, model, cx| {
                let state = workspace_state(model.read(cx), &this.repo_id);
                if state != this.shown {
                    this.shown = state;
                    cx.notify();
                }
            }),
            cx.subscribe(&commits, |this, _, event: &CommitsViewEvent, cx| {
                if let CommitsViewEvent::CommitPicked { repo_id, .. } = event {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.clear_viewed_ref(repo_id, cx));
                }
            }),
            cx.subscribe(&model, |this, _, event: &mergeon_app::AppEvent, _| {
                if let mergeon_app::AppEvent::WorkspaceTabReplaced { from, to, .. } = event {
                    if let Some(panel) = this.file_panels.remove(from) {
                        this.file_panels.insert(to.clone(), panel);
                    }
                    if let Some(view) = this.file_changes.remove(from) {
                        this.file_changes.insert(to.clone(), view);
                    }
                    if let Some(view) = this.file_history.remove(from) {
                        this.file_history.insert(to.clone(), view);
                    }
                }
            }),
        ];
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| {
                let state = workspace_state(this.model.read(cx), &this.repo_id);
                if state != this.shown {
                    this.shown = state;
                    cx.notify();
                }
            })
            .ok();
        });
        Self {
            shown: workspace_state(model.read(cx), &repo_id),
            model,
            repo_id,
            sidebar,
            commits,
            changes,
            file_panels: HashMap::new(),
            file_changes: HashMap::new(),
            file_history: HashMap::new(),
            files_open: true,
            file_tab_scroll: ScrollHandle::new(),
            scrolled_file_tab: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn commits(&self) -> &Entity<CommitsView> {
        &self.commits
    }

    /// Local Changes del repositorio (la caja de commit con "Fixup into…").
    pub fn changes(&self) -> &Entity<ChangesView> {
        &self.changes
    }

    fn file_panel(
        &mut self,
        tab: &FileTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyView {
        if let Some(panel) = self.file_panels.get(&tab.id) {
            return panel.clone();
        }
        let panel: AnyView = match tab.source {
            TabSource::Working | TabSource::Index => {
                let view = cx.new(|cx| {
                    ChangesView::new(
                        self.repo_id.clone(),
                        ChangesMode::FileTab {
                            tab_id: tab.id.clone(),
                        },
                        window,
                        cx,
                    )
                });
                view.update(cx, |view, cx| view.set_files_open(self.files_open, cx));
                self.file_changes.insert(tab.id.clone(), view.clone());
                view.into()
            }
            TabSource::Commit | TabSource::Tree | TabSource::History => {
                let view = cx.new(|cx| {
                    HistoricalFileTabView::new(self.repo_id.clone(), tab.id.clone(), window, cx)
                });
                view.update(cx, |view, cx| view.set_files_open(self.files_open, cx));
                self.file_history.insert(tab.id.clone(), view.clone());
                view.into()
            }
        };
        self.file_panels.insert(tab.id.clone(), panel.clone());
        panel
    }

    fn activate(&mut self, path: &str, tab: &str, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.activate_workspace_tab(path, tab, cx);
        });
    }

    fn close(&mut self, path: &str, tab: &str, cx: &mut Context<Self>) {
        self.file_panels.remove(tab);
        self.file_changes.remove(tab);
        self.file_history.remove(tab);
        self.model.update(cx, |model, cx| {
            model.close_workspace_tab(path, tab, cx);
        });
    }

    fn toggle_file_list(&mut self, cx: &mut Context<Self>) {
        self.files_open = !self.files_open;
        let open = self.files_open;
        for view in self.file_changes.values() {
            view.update(cx, |view, cx| view.set_files_open(open, cx));
        }
        for view in self.file_history.values() {
            view.update(cx, |view, cx| view.set_files_open(open, cx));
        }
        cx.notify();
    }

    /// Una fila de vista de arriba del sidebar ("Local Changes", "All
    /// Commits"), como las de Fork.
    fn view_row(&self, row: ViewRow, path: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let ViewRow {
            id,
            tab,
            label,
            icon,
            active,
            count,
        } = row;
        let palette = *cx.palette();
        let path = path.to_string();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(8.))
            .rounded(Radius::Md.px(cx))
            .text_size(text::BODY)
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(if active {
                palette.text_1
            } else {
                palette.text_2
            })
            .when(active, |row| {
                row.bg(palette.sidebar_row_selected)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!active, |row| {
                row.hover(move |row| row.bg(palette.sidebar_row_hover))
            })
            .child(
                mergeon_ui::icon(icon)
                    .size(px(TAB_ICON_SIZE))
                    .tone(IconTone::Current),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(count > 0, |row| {
                // Señal: contador en mono sobre gris (`count`).
                row.child(
                    div()
                        .id(SharedString::from(format!("{id}-count")))
                        .test_support()
                        .aria_label(SharedString::from(count.to_string()))
                        .flex_shrink_0()
                        .min_w(px(18.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded(Radius::Xs.px(cx))
                        .bg(palette.bg_pressed)
                        .text_color(palette.text_1)
                        .font_family(cx.theme().mono_font_family.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(text::CAPTION)
                        .line_height(px(16.))
                        .text_center()
                        .child(count.to_string()),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.activate(&path, tab, cx)))
            .test_support()
            .aria_label(SharedString::from(label))
    }

    /// Las dos filas de vista sobre el sidebar.
    fn view_nav(
        &self,
        path: &str,
        active_tab: &str,
        changes_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .id("workspace-view-nav")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w_full()
            .gap(px(2.))
            .p(px(6.))
            .bg(palette.sidebar)
            .border_r_1()
            .border_b_1()
            .border_color(palette.sidebar_border)
            .child(self.view_row(
                ViewRow {
                    id: "workspace-view-changes",
                    tab: CHANGES_TAB,
                    label: "Local Changes",
                    icon: Lucide::FilePenLine,
                    active: active_tab == CHANGES_TAB,
                    count: changes_count,
                },
                path,
                cx,
            ))
            .child(self.view_row(
                ViewRow {
                    id: "workspace-view-commits",
                    tab: HOME_TAB,
                    label: "All Commits",
                    icon: Lucide::GitCommitHorizontal,
                    active: active_tab == HOME_TAB,
                    count: 0,
                },
                path,
                cx,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn file_tab(
        &self,
        tab: &FileTab,
        index: usize,
        active: bool,
        is_last: bool,
        suffix: Option<String>,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let menu = {
            let model = self.model.clone();
            let path = path.to_string();
            let id = tab.id.clone();
            let preview = tab.preview;
            let history = history_descriptor(tab);
            move |_: &mut Window, _: &mut gpui_kit::App| {
                file_tab_menu(&model, &path, &id, preview, is_last, history.clone())
            }
        };
        let icon = document_icon(tab);
        let middle_path = path.to_string();
        let middle_id = tab.id.clone();
        let click_path = path.to_string();
        let click_id = tab.id.clone();
        let close_path = path.to_string();
        let close_id = tab.id.clone();
        let drop_path = path.to_string();
        let label: SharedString = tab.label().to_string().into();
        let title = if tab.path.is_empty() {
            tab.label().to_string()
        } else {
            tab.path.clone()
        };
        let tooltip = if tab.preview {
            format!(
                "{title} · {} · Preview (double-click to keep open)",
                tab.context()
            )
        } else {
            format!("{title} · {}", tab.context())
        };
        let group = SharedString::from(format!("file-tab-group-{}", tab.id));
        let hover_bg = palette.bg_hover;
        let text_2 = palette.text_2;

        div()
            .id(SharedString::from(format!("file-tab-{}", tab.id)))
            .group(group.clone())
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .h_full()
            .max_w(px(DOCUMENT_TAB_MAX_WIDTH))
            .pl(px(10.))
            .pr(px(4.))
            .border_r_1()
            .border_color(palette.border_hair)
            .text_size(text::UI)
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(if active {
                palette.text_1
            } else {
                palette.text_3
            })
            .when(active, |el| {
                el.bg(palette.background)
                    .child(active_indicator(palette.foreground))
            })
            .when(!active, |el| {
                el.hover(move |el| el.bg(hover_bg).text_color(text_2))
            })
            .tooltip(mergeon_ui::tooltip(tooltip))
            // Clic central: cerrar.
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.close(&middle_path, &middle_id, cx)),
            )
            // Clic: activar. Doble clic: fijar la de vista previa.
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.model.update(cx, |model, cx| {
                        model.pin_workspace_tab(&click_path, &click_id, cx);
                    });
                } else {
                    this.activate(&click_path, &click_id, cx);
                }
            }))
            // Arrastrar para reordenar (y fijarla).
            .on_drag(
                DraggedDocumentTab {
                    index,
                    label: label.clone(),
                },
                |dragged, _, _, cx| {
                    let label = dragged.label.clone();
                    cx.new(|_| DocumentDragPreview { label })
                },
            )
            .drag_over::<DraggedDocumentTab>(move |style, _, _, _| {
                style.shadow(insertion_marker(palette.primary))
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedDocumentTab, _, cx| {
                    this.model.update(cx, |model, cx| {
                        model.move_workspace_tab(&drop_path, dragged.index, index, cx);
                    });
                }),
            )
            .child(
                mergeon_ui::icon(icon)
                    .size(px(TAB_ICON_SIZE))
                    .tone(IconTone::Current),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_baseline()
                    .gap(px(5.))
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w(px(24.))
                            .truncate()
                            .when(tab.preview, |label| label.italic())
                            .child(label.clone()),
                    )
                    .when_some(suffix, |el, suffix| {
                        // La carpeta que distingue dos archivos homónimos.
                        el.child(
                            div()
                                .flex_shrink(1.)
                                .min_w_0()
                                .truncate()
                                .text_size(text::CAPTION)
                                .text_color(palette.text_3)
                                .child(suffix),
                        )
                    }),
            )
            .child(
                // × solo en la activa y al pasar por encima.
                div()
                    .id(SharedString::from(format!("file-tab-close-{}", tab.id)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded(Radius::Md.px(cx))
                    .cursor_pointer()
                    .text_color(palette.text_3)
                    .when(!active, |el| {
                        el.opacity(0.).group_hover(group, |style| style.opacity(1.))
                    })
                    .hover(move |el| el.bg(palette.bg_pressed).text_color(palette.text_1))
                    .child(
                        mergeon_ui::icon(Lucide::X)
                            .size(px(12.))
                            .tone(IconTone::Current),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close(&close_path, &close_id, cx)
                    })),
            )
            .test_support()
            .aria_label(label)
            .context_menu_items(menu)
            .into_any_element()
    }

    /// "▾": todos los documentos abiertos en una lista, para llegar a los que
    /// no caben.
    fn document_list_button(
        &self,
        path: &str,
        workspace: &RepositoryWorkspace,
        suffixes: &[Option<String>],
    ) -> impl IntoElement {
        let model = self.model.clone();
        let path = path.to_string();
        let active = workspace.active_tab.clone();
        let entries: Vec<(String, String, String)> = workspace
            .tabs
            .iter()
            .zip(suffixes)
            .map(|(tab, suffix)| {
                let label = match suffix {
                    Some(suffix) => format!("{} · {suffix}", tab.label()),
                    None => tab.label().to_string(),
                };
                (tab.id.clone(), label, tab.context())
            })
            .collect();
        div().flex_none().px_1().child(
            mergeon_ui::button("workspace-tab-list")
                .ghost()
                .xs()
                .icon(Lucide::ChevronDown)
                .tooltip("Open documents")
                .dropdown_menu_items(move |_, _| {
                    entries
                        .iter()
                        .map(|(id, label, context)| {
                            let (model, path, id) = (model.clone(), path.clone(), id.clone());
                            MenuItem::new(label.clone())
                                .detail(context.clone())
                                .checked(active == id)
                                .list_box()
                                .on_click(move |_, cx| {
                                    model.update(cx, |model, cx| {
                                        model.activate_workspace_tab(&path, &id, cx)
                                    });
                                })
                        })
                        .collect()
                }),
        )
    }

    /// La barra de documentos: solo existe con alguno abierto.
    fn document_bar(
        &mut self,
        path: &str,
        workspace: &RepositoryWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = *cx.palette();
        self.scroll_active_file_tab_into_view(&workspace.active_tab, &workspace.tabs, window);
        let count = workspace.tabs.len();
        let suffixes = file_tab_suffixes(&workspace.tabs);
        let tabs: Vec<AnyElement> = workspace
            .tabs
            .iter()
            .zip(suffixes.iter().cloned())
            .enumerate()
            .map(|(index, (tab, suffix))| {
                self.file_tab(
                    tab,
                    index,
                    workspace.active_tab == tab.id,
                    index + 1 == count,
                    suffix,
                    path,
                    cx,
                )
            })
            .collect();
        div()
            .id("workspace-tabs")
            .test_support()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(DOCUMENT_BAR_HEIGHT))
            .border_b_1()
            .border_color(palette.border_hair)
            .bg(palette.bg_sunken)
            .child(
                div()
                    .id("workspace-file-tabs")
                    .flex()
                    .flex_1()
                    .w_0()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.file_tab_scroll)
                    .children(tabs),
            )
            .when(count >= 2, |bar| {
                bar.child(self.document_list_button(path, workspace, &suffixes))
            })
    }

    /// Cabecera del documento activo: migas de su ruta (`src › view › a.rs`),
    /// su contexto en una pastilla y las acciones.
    fn document_header(&self, tab: &FileTab, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let crumbs = path_breadcrumbs(&tab.path);
        let last = crumbs.len().saturating_sub(1);
        let mut trail = div()
            .id("file-tab-breadcrumbs")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(4.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(text::UI);
        if crumbs.is_empty() {
            trail = trail.child(
                div()
                    .text_color(palette.text_1)
                    .font_weight(FontWeight::MEDIUM)
                    .child(tab.label().to_string()),
            );
        }
        for (index, crumb) in crumbs.iter().enumerate() {
            if index > 0 {
                trail = trail.child(
                    mergeon_ui::icon(Lucide::ChevronRight)
                        .size(px(12.))
                        .text_color(palette.text_3),
                );
            }
            let is_file = index == last;
            trail = trail.child(
                div()
                    .when(!is_file, |crumb| crumb.flex_shrink(1.).min_w(px(16.)))
                    .when(is_file, |crumb| crumb.flex_shrink_0())
                    .truncate()
                    .text_color(if is_file {
                        palette.text_1
                    } else {
                        palette.text_3
                    })
                    .when(is_file, |crumb| crumb.font_weight(FontWeight::MEDIUM))
                    .child(crumb.to_string()),
            );
        }
        let (id, source) = (tab.id.clone(), tab.source);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .px(px(8.))
            .border_b_1()
            .border_color(palette.border_hair)
            .child(
                mergeon_ui::button("toggle-file-list")
                    .ghost()
                    .xs()
                    .icon(Lucide::PanelLeft)
                    .tooltip("Toggle file list")
                    .selected(self.files_open)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_file_list(cx))),
            )
            .child(
                trail
                    .test_support()
                    .aria_label(SharedString::from(crumbs.join(" › "))),
            )
            .child(
                div()
                    .id("file-tab-context")
                    .flex_shrink_0()
                    .h(px(18.))
                    .px(px(6.))
                    .rounded(Radius::Xs.px(cx))
                    .bg(palette.bg_pressed)
                    .text_color(palette.text_2)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(text::CAPTION)
                    .line_height(px(18.))
                    .child(tab.context())
                    .test_support()
                    .aria_label(SharedString::from(tab.context())),
            )
            .child(
                mergeon_ui::button("file-tab-open-in-editor")
                    .ghost()
                    .xs()
                    .icon(Lucide::CodeXml)
                    .tooltip("Open in editor")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_file_tab_in_editor(&id, source, cx);
                    })),
            )
    }

    /// "Open in editor" de la cabecera: el archivo que muestra la pestaña (su
    /// versión del working tree), en la línea a la vista si se sabe.
    fn open_file_tab_in_editor(&mut self, id: &str, source: TabSource, cx: &mut Context<Self>) {
        match source {
            TabSource::Working | TabSource::Index => {
                if let Some(view) = self.file_changes.get(id) {
                    view.update(cx, |view, cx| view.open_selected_in_editor(cx));
                }
            }
            TabSource::Commit | TabSource::Tree | TabSource::History => {
                if let Some(view) = self.file_history.get(id) {
                    view.update(cx, |view, cx| view.open_shown_file_in_editor(cx));
                }
            }
        }
    }

    /// Lleva la pestaña de archivo activa a la vista cuando cambia.
    fn scroll_active_file_tab_into_view(
        &mut self,
        active_tab: &str,
        tabs: &[FileTab],
        window: &mut Window,
    ) {
        let target = (active_tab.to_string(), tabs.len());
        if self.scrolled_file_tab.as_ref() == Some(&target) {
            return;
        }
        if let Some(index) = tabs.iter().position(|tab| tab.id == active_tab) {
            self.file_tab_scroll.scroll_to_item(index);
        }
        // GPUI calcula el desplazamiento con el tamaño de la tira del
        // fotograma anterior: en el primero aún no lo tiene, así que se
        // repite en el siguiente.
        if self.file_tab_scroll.bounds().size.width > px(0.) {
            self.scrolled_file_tab = Some(target);
        } else {
            window.request_animation_frame();
        }
    }
}

/// Ícono de una pestaña de documento.
fn document_icon(tab: &FileTab) -> Lucide {
    if tab.path.is_empty() {
        Lucide::Folders
    } else if tab.source == TabSource::History {
        Lucide::FileClock
    } else {
        Lucide::FileCodeCorner
    }
}

/// "File history" del archivo de una pestaña: desde su commit, o desde HEAD si
/// es del working tree o del índice. `None` en el árbol sin archivo y en la
/// propia pestaña de historial.
fn history_descriptor(tab: &FileTab) -> Option<FileTabDescriptor> {
    if tab.path.is_empty() || tab.source == TabSource::History {
        return None;
    }
    let commit = match tab.source {
        TabSource::Commit | TabSource::Tree => Some(tab.commit_hash.as_str()),
        _ => None,
    };
    Some(FileTabDescriptor::history(tab.path.clone(), commit))
}

/// Menú contextual de una pestaña de archivo.
fn file_tab_menu(
    model: &Entity<AppModel>,
    path: &str,
    id: &str,
    preview: bool,
    is_last: bool,
    history: Option<FileTabDescriptor>,
) -> Vec<MenuItem> {
    let run = |action: fn(&mut AppModel, &str, &str, &mut Context<AppModel>)| {
        let (model, path, id) = (model.clone(), path.to_string(), id.to_string());
        move |_: &mut Window, cx: &mut gpui_kit::App| {
            model.update(cx, |model, cx| action(model, &path, &id, cx));
        }
    };
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    };
    let mut items = Vec::new();
    if preview {
        items.push(MenuItem::new("Keep open").icon(Lucide::Pin).on_click(run(
            |model, path, id, cx| model.pin_workspace_tab(path, id, cx),
        )));
        items.push(MenuItem::separator());
    }
    items.extend([
        MenuItem::new("Close")
            .icon(Lucide::X)
            .shortcut(format!("{modifier}W"))
            .on_click(run(|model, path, id, cx| {
                model.close_workspace_tab(path, id, cx)
            })),
        MenuItem::new("Close others").on_click(run(|model, path, id, cx| {
            model.close_other_workspace_tabs(path, id, cx)
        })),
        MenuItem::new("Close to the right")
            .disabled(is_last)
            .on_click(run(|model, path, id, cx| {
                model.close_workspace_tabs_to_the_right(path, id, cx)
            })),
        MenuItem::new("Close all").on_click(run(|model, path, _, cx| {
            model.close_all_workspace_tabs(path, cx)
        })),
    ]);
    if let Some(descriptor) = history {
        let (model, path) = (model.clone(), path.to_string());
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("File history")
                .icon(Lucide::FileClock)
                .on_click(move |_, cx| {
                    model.update(cx, |model, cx| {
                        model.open_workspace_file(&path, &descriptor, false, cx);
                    });
                }),
        );
    }
    items
}

impl Render for RepositoryWorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pinta desde `self.shown` sin leer `AppModel` en `render`: leer el
        // modelo aquí registraría la entidad en GPUI y repintaría la ventana
        // ante cualquier aviso ajeno.
        let Some((path, changes_count, workspace)) = self.shown.clone() else {
            return div().into_any_element();
        };
        let active_tab = workspace.active_tab.clone();
        // Las pestañas que ya no existen sueltan su panel.
        let open: Vec<String> = workspace.tabs.iter().map(|tab| tab.id.clone()).collect();
        self.file_panels.retain(|id, _| open.contains(id));
        self.file_changes.retain(|id, _| open.contains(id));
        self.file_history.retain(|id, _| open.contains(id));
        let changes_active = active_tab == CHANGES_TAB;
        self.changes
            .update(cx, |view, cx| view.set_active(changes_active, cx));
        if workspace.tabs.is_empty() {
            self.scrolled_file_tab = None;
        }

        // Izquierda, fija en todas las vistas: las filas de vista y el sidebar.
        let left = div()
            .id("workspace-left")
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .min_h_0()
            .child(self.view_nav(&path, &active_tab, changes_count, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.sidebar.clone()),
            );

        let panel = if active_tab == HOME_TAB {
            div()
                .id("workspace-commits")
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .child(self.commits.clone())
                .into_any_element()
        } else if active_tab == CHANGES_TAB {
            div()
                .id("workspace-changes")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .child(self.changes.clone())
                .into_any_element()
        } else if let Some(tab) = workspace.tabs.iter().find(|tab| tab.id == active_tab) {
            let tab = tab.clone();
            let body = self.file_panel(&tab, window, cx);
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .child(self.document_header(&tab, cx))
                .child(body)
                .into_any_element()
        } else {
            div().flex_1().into_any_element()
        };

        let right = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .when(!workspace.tabs.is_empty(), |right| {
                right.child(self.document_bar(&path, &workspace, window, cx))
            })
            .child(panel);

        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(left)
            .child(SidebarResizer::new(&self.sidebar))
            .child(right)
            .into_any_element()
    }
}
