//! Sección "Worktrees" del sidebar: los worktrees del repositorio con su rama
//! y si tienen cambios, abrir uno como pestaña, crear, quitar, bloquear o
//! desbloquear y podar. También sabe qué ramas están desprotegidas en otro
//! worktree, para marcarlas en Branches y avisar antes de un checkout que git
//! rechazaría.
//!
//! La lista (`git worktree list`, una lectura que solo mira archivos de
//! `.git`) se lee al cambiar de repositorio y tras cada mutación, siempre en
//! segundo plano; el estado de cada worktree (`git status` cortado en el
//! primer byte) solo mientras la sección está desplegada.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, SharedString, Subscription, Task,
    TestSupportExt as _, WeakEntity, Window, div, px, rems,
};
use mergeon_app::{AddRepository, AppModel, StructuredOutcome as _, SyncRequest};
use mergeon_backend::Worktree;
use mergeon_model::dialogs::{ConfirmAnswer, ConfirmOptions};
use mergeon_ui::menu::MenuTriggerExt as _;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide};

use crate::dialogs::{
    NewWorktreeDialog, NewWorktreeDialogEvent, NewWorktreeOptions, open_new_worktree_dialog,
};
use crate::flows::{self, RepoRef};
use crate::menus::{MenuEntry, SidebarCommand};
use crate::render_sections::menu_items;
use crate::section;
use crate::sidebar::Sidebar;
use crate::virtual_rows::{RowHeights, virtual_rows};

/// Clave de `settings.sidebarSections` (entre las que el store conserva sin
/// conocerlas) que recuerda si la sección está desplegada.
pub(crate) const SECTION_KEY: &str = "worktrees";
/// Como mucho, cuántos worktrees miran su estado (cada uno es un `git status`).
const MAX_STATUS_READS: usize = 50;
/// Cuántos `git status` de worktrees van a la vez.
const STATUS_READS_AT_ONCE: usize = 4;

/// Lo que el sidebar sabe de los worktrees del repositorio activo.
#[derive(Default)]
pub(crate) struct WorktreeState {
    repo_id: Option<String>,
    pub list: Rc<Vec<Worktree>>,
    /// Rama → índice en `list` del worktree que la tiene desprotegida (este
    /// incluido): cada fila de Branches lo mira sin recorrer la lista.
    pub branch_worktrees: HashMap<String, usize>,
    /// Ruta → tiene cambios sin commitear (lo que ya se leyó).
    pub changes: Rc<HashMap<String, bool>>,
    pub loaded: bool,
    pub error: Option<String>,
    task: Option<Task<()>>,
    reload_queued: bool,
    status_task: Option<Task<()>>,
    status_queued: bool,
    /// Hay que releer el estado de todos los worktrees (la lista cambió, la
    /// sección se desplegó o se pidió refrescar). Si no, tras una mutación
    /// solo se relee el de esta ventana: los demás no los toca.
    status_stale: bool,
    pub dialog: Option<(Entity<NewWorktreeDialog>, Subscription)>,
    /// Lecturas de la lista hechas (tests).
    reads: usize,
    /// Lecturas de estado (`git status`) de worktrees hechas (tests).
    status_reads: usize,
}

/// Los 7 primeros caracteres de un hash.
/// Si la sección Worktrees está desplegada (por defecto, no).
fn worktrees_expanded_in(sections: &mergeon_store::SidebarSections) -> bool {
    sections
        .extra
        .get(SECTION_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

impl Sidebar {
    // ------------------------------------------------------------------
    // Lectura
    // ------------------------------------------------------------------

    /// Los worktrees del repositorio activo, el principal primero.
    pub fn worktrees(&self) -> &[Worktree] {
        &self.worktrees.list
    }

    /// Si el worktree de `path` tiene cambios (`None`: todavía no se sabe).
    pub fn worktree_has_changes(&self, path: &str) -> Option<bool> {
        self.worktrees.changes.get(path).copied()
    }

    /// La ruta del *otro* worktree en el que está desprotegida `branch`.
    pub fn branch_worktree(&self, branch: &str) -> Option<&str> {
        let index = *self.worktrees.branch_worktrees.get(branch)?;
        let worktree = self.worktrees.list.get(index)?;
        (!worktree.is_current).then_some(worktree.path.as_str())
    }

    /// Rama → ruta de cada worktree con una rama desprotegida.
    fn branch_paths(&self) -> HashMap<String, String> {
        self.worktrees
            .branch_worktrees
            .iter()
            .filter_map(|(branch, index)| {
                let worktree = self.worktrees.list.get(*index)?;
                Some((branch.clone(), worktree.path.clone()))
            })
            .collect()
    }

    /// El diálogo «New Worktree» abierto, si lo hay.
    pub fn new_worktree_dialog(&self) -> Option<&Entity<NewWorktreeDialog>> {
        self.worktrees.dialog.as_ref().map(|(dialog, _)| dialog)
    }

    /// Cuántas veces se leyó la lista de worktrees.
    #[doc(hidden)]
    pub fn worktree_reads(&self) -> usize {
        self.worktrees.reads
    }

    /// Cuántas veces se leyó el estado de un worktree.
    #[doc(hidden)]
    pub fn worktree_status_reads(&self) -> usize {
        self.worktrees.status_reads
    }

    pub(crate) fn worktrees_expanded(&self, cx: &App) -> bool {
        worktrees_expanded_in(self.model.read(cx).sidebar_sections())
    }

    pub(crate) fn toggle_worktrees_section(&mut self, cx: &mut Context<Self>) {
        let expanded = !self.worktrees_expanded(cx);
        self.model
            .update(cx, |model, cx| {
                model.update_sidebar_sections(cx, |sections| {
                    sections
                        .extra
                        .insert(SECTION_KEY.to_string(), serde_json::Value::Bool(expanded));
                })
            })
            .detach();
        if expanded {
            self.worktrees.status_stale = true;
            self.load_worktree_changes(cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Carga
    // ------------------------------------------------------------------

    /// Relee la lista de worktrees del repositorio activo. Con una lectura en
    /// vuelo, se repite al acabar (una sola vez, por muchas que se pidan).
    pub(crate) fn load_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            self.worktrees = WorktreeState::default();
            return;
        };
        if self.worktrees.repo_id.as_deref() != Some(repo.id.as_str()) {
            let dialog = self.worktrees.dialog.take();
            self.worktrees = WorktreeState {
                repo_id: Some(repo.id.clone()),
                dialog,
                ..Default::default()
            };
        }
        if self.worktrees.task.is_some() {
            self.worktrees.reload_queued = true;
            return;
        }
        self.worktrees.reads += 1;
        let read = self
            .model
            .update(cx, |model, cx| model.get_worktrees(&repo.path, cx));
        self.worktrees.task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                this.worktrees.task = None;
                if this.worktrees.repo_id.as_deref() != Some(repo.id.as_str()) {
                    return;
                }
                let state = &mut this.worktrees;
                let changed = match result {
                    Ok(list) => {
                        let changed = *state.list != list || !state.loaded || state.error.is_some();
                        if changed {
                            state.branch_worktrees = list
                                .iter()
                                .enumerate()
                                .filter_map(|(index, worktree)| {
                                    Some((worktree.branch.clone()?, index))
                                })
                                .collect();
                            state.list = Rc::new(list);
                            state.status_stale = true;
                        }
                        state.error = None;
                        state.loaded = true;
                        changed
                    }
                    Err(error) => {
                        let message = error.message().to_string();
                        let changed = state.error.as_deref() != Some(message.as_str());
                        state.error = Some(message);
                        state.loaded = true;
                        changed
                    }
                };
                if changed {
                    cx.notify();
                }
                if std::mem::take(&mut this.worktrees.reload_queued) {
                    this.load_worktrees(cx);
                } else {
                    this.load_worktree_changes(cx);
                }
            });
        }));
    }

    /// Lee si cada worktree tiene cambios, solo con la sección desplegada y
    /// de pocos en pocos. Si la lista no cambió desde la última vez, solo se
    /// relee el worktree de esta ventana (el único que una mutación de aquí
    /// puede ensuciar): una mutación no lanza decenas de `git status`. Avisa
    /// una vez al final, y solo si algo cambió.
    fn load_worktree_changes(&mut self, cx: &mut Context<Self>) {
        if !self.worktrees_expanded(cx) || !self.worktrees.loaded {
            return;
        }
        if self.worktrees.status_task.is_some() {
            self.worktrees.status_queued = true;
            return;
        }
        let full = std::mem::take(&mut self.worktrees.status_stale);
        let paths: Vec<String> = self
            .worktrees
            .list
            .iter()
            .filter(|worktree| !worktree.bare && !worktree.prunable)
            .filter(|worktree| full || worktree.is_current)
            .take(MAX_STATUS_READS)
            .map(|worktree| worktree.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        self.worktrees.status_reads += paths.len();
        let model = self.model.clone();
        let previous = self.worktrees.changes.clone();
        self.worktrees.status_task = Some(cx.spawn(async move |this, cx| {
            // Sin lectura completa se parte de lo que ya se sabía.
            let mut changes = if full {
                HashMap::new()
            } else {
                (*previous).clone()
            };
            for batch in paths.chunks(STATUS_READS_AT_ONCE) {
                let reads: Vec<_> = batch
                    .iter()
                    .map(|path| {
                        let read =
                            model.update(cx, |model, cx| model.worktree_has_changes(path, cx));
                        (path.clone(), read)
                    })
                    .collect();
                for (path, read) in reads {
                    if let Ok(dirty) = read.await {
                        changes.insert(path, dirty);
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.worktrees.status_task = None;
                if *this.worktrees.changes != changes {
                    this.worktrees.changes = Rc::new(changes);
                    cx.notify();
                }
                if std::mem::take(&mut this.worktrees.status_queued) {
                    this.load_worktree_changes(cx);
                }
            });
        }));
    }

    /// Refrescar a mano: la lista y el estado de todos los worktrees.
    pub(crate) fn refresh_worktrees(&mut self, cx: &mut Context<Self>) {
        self.worktrees.status_stale = true;
        self.load_worktrees(cx);
    }

    // ------------------------------------------------------------------
    // Órdenes
    // ------------------------------------------------------------------

    /// Las órdenes de la sección (y "Check out in new worktree…" de una rama).
    pub(crate) fn run_worktree_command(
        &mut self,
        command: SidebarCommand,
        repo: RepoRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        match command {
            SidebarCommand::NewWorktree => self.open_new_worktree(None, window, cx),
            SidebarCommand::CheckoutInNewWorktree(branch) => {
                if let Some(path) = self.branch_worktree(&branch).map(str::to_string) {
                    model.update(cx, |model, cx| {
                        model.notify_warning(
                            &mergeon_git_ops::branch_in_other_worktree_message(&branch, &path),
                            cx,
                        );
                    });
                    return;
                }
                self.open_new_worktree(Some(branch), window, cx);
            }
            SidebarCommand::OpenWorktree(path) => open_worktree(&model, &path, cx),
            SidebarCommand::CopyWorktreePath(path) => {
                cx.write_to_clipboard(ClipboardItem::new_string(path));
                model.update(cx, |model, cx| {
                    model.notify_success("Worktree path copied to clipboard", cx);
                });
            }
            SidebarCommand::RemoveWorktree(path) => {
                let worktree = self
                    .worktrees
                    .list
                    .iter()
                    .find(|worktree| worktree.path == path)
                    .cloned();
                let Some(worktree) = worktree else {
                    return;
                };
                let dirty = self.worktree_has_changes(&path);
                remove_worktree(&model, repo, worktree, dirty, cx).detach();
            }
            SidebarCommand::LockWorktree(path) => {
                set_locked(&model, repo, path, true, cx).detach();
            }
            SidebarCommand::UnlockWorktree(path) => {
                set_locked(&model, repo, path, false, cx).detach();
            }
            SidebarCommand::PruneWorktrees => prune(&model, repo, cx).detach(),
            _ => {}
        }
    }

    /// «New Worktree»: con `branch`, en "Existing branch" sobre ella.
    fn open_new_worktree(
        &mut self,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let (branches, current) = {
            let model = self.model.read(cx);
            let Some(repository) = model.repository(repo.id.as_str()) else {
                return;
            };
            (
                repository
                    .branches
                    .iter()
                    .map(|branch| branch.name.clone())
                    .collect::<Vec<_>>(),
                repository
                    .current_branch
                    .clone()
                    .filter(|branch| !branch.is_empty()),
            )
        };
        let mut options = NewWorktreeOptions::new(repo.path.clone())
            .branches(branches)
            .busy_branches(self.branch_paths())
            .start_point(current.unwrap_or_else(|| "HEAD".to_string()));
        if let Some(branch) = branch {
            options = options.existing_branch(branch);
        }
        // Uno a la vez: el anterior no se queda huérfano en la pila.
        self.dismiss_new_worktree_dialog(window, cx);
        let dialog = open_new_worktree_dialog(options, window, cx);
        let model = self.model.clone();
        let subscription = cx.subscribe(&dialog, move |this, _, event, cx| match event {
            NewWorktreeDialogEvent::Submit {
                options,
                open_after,
            } => {
                this.worktrees.dialog = None;
                add_worktree(&model, repo.clone(), options.clone(), *open_after, cx).detach();
            }
            NewWorktreeDialogEvent::Close => {
                this.worktrees.dialog = None;
            }
        });
        self.worktrees.dialog = Some((dialog, subscription));
    }

    /// El menú de un worktree.
    pub fn worktree_menu(&self, path: &str, cx: &App) -> Vec<MenuEntry> {
        let Some(worktree) = self.worktrees.list.iter().find(|item| item.path == path) else {
            return Vec::new();
        };
        let busy = self.active_repo(cx).is_some_and(|repo| {
            [
                "worktree-remove",
                "worktree-lock",
                "worktree-unlock",
                "worktree-prune",
            ]
            .iter()
            .any(|operation| self.is_operation_running(&repo.id, operation, cx))
        });
        worktree_menu(worktree, busy)
    }

    // ------------------------------------------------------------------
    // Pintado
    // ------------------------------------------------------------------

    /// La sección "Worktrees".
    pub(crate) fn render_worktree_section(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = worktrees_expanded_in(&self.render_key().sections);
        let state = &self.worktrees;
        let count = state.list.len();
        let can_prune = state
            .list
            .iter()
            .any(|worktree| worktree.prunable && !worktree.locked);
        let row_height = section::row_height(window.rem_size());

        section::section()
            .child(
                section::header("worktrees", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_worktrees_section(cx)))
                    .child(section::chevron(expanded))
                    .child(section::title("Worktrees", cx))
                    .when(can_prune, |header| {
                        header.child(
                            section::header_action(
                                "prune-worktrees",
                                Lucide::Eraser,
                                "Prune worktrees whose folder is gone",
                                "worktrees",
                                cx,
                                command_listener(SidebarCommand::PruneWorktrees, cx),
                            )
                            .test_support(),
                        )
                    })
                    .child(
                        section::header_action(
                            "new-worktree",
                            Lucide::Plus,
                            "New worktree…",
                            "worktrees",
                            cx,
                            command_listener(SidebarCommand::NewWorktree, cx),
                        )
                        .test_support(),
                    )
                    .child(if state.loaded {
                        section::count(count, cx)
                    } else {
                        section::count_loading()
                    })
                    .test_support(),
            )
            .when(expanded, |this| {
                this.child(if let Some(error) = &state.error {
                    section::content(false).child(
                        div()
                            .id("worktrees-error")
                            .test_support()
                            .pt(rems(0.3))
                            .pr(rems(0.875))
                            .pb(rems(0.45))
                            .pl(rems(1.625))
                            .text_size(rems(0.8125))
                            .text_color(cx.palette().destructive_foreground)
                            .child(error.clone()),
                    )
                } else if !state.loaded {
                    section::content(false).child(section::empty("Loading worktrees…", cx))
                } else if count <= 1 {
                    // Solo el principal: se lista igual, con la pista de cómo
                    // crear otro.
                    section::content(false)
                        .child(self.worktree_rows_element(row_height, cx))
                        .child(section::empty(
                            "No other worktrees. Use + or a branch's menu to add one.",
                            cx,
                        ))
                } else {
                    section::content(false).child(self.worktree_rows_element(row_height, cx))
                })
            })
    }

    fn worktree_rows_element(
        &self,
        row_height: gpui_kit::Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let list = self.worktrees.list.clone();
        let changes = self.worktrees.changes.clone();
        let tooltips = self.row_tooltips;
        virtual_rows(
            "worktree-rows",
            RowHeights::uniform(list.len(), row_height),
            move |range, _, cx| {
                list[range]
                    .iter()
                    .map(|worktree| {
                        worktree_row(
                            worktree,
                            changes.get(&worktree.path).copied(),
                            tooltips,
                            &view,
                            cx,
                        )
                    })
                    .collect()
            },
        )
    }
}

fn command_listener(
    command: SidebarCommand,
    cx: &Context<Sidebar>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, _, window, cx| this.run_command(command.clone(), window, cx))
}

fn row_id(kind: &str, key: &str) -> SharedString {
    SharedString::from(format!("{kind}:{key}"))
}

/// Qué tiene desprotegido un worktree, para la fila: la rama, o el commit
/// (desanclado), o "bare".
pub fn worktree_target_label(worktree: &Worktree) -> String {
    if worktree.bare {
        return "bare".to_string();
    }
    match (&worktree.branch, &worktree.head) {
        (Some(branch), _) => branch.clone(),
        (None, Some(head)) => format!("detached at {}", short_hash(head)),
        (None, None) => "no commits".to_string(),
    }
}

/// El texto de ayuda de una fila.
pub fn worktree_tooltip(worktree: &Worktree, dirty: Option<bool>) -> String {
    let mut lines = vec![worktree.path.clone(), worktree_target_label(worktree)];
    if worktree.is_main {
        lines.push("Main worktree".to_string());
    }
    if worktree.is_current {
        lines.push("This window's worktree".to_string());
    }
    if dirty == Some(true) {
        lines.push("Has uncommitted changes".to_string());
    }
    if worktree.locked {
        lines.push(match &worktree.lock_reason {
            Some(reason) => format!("Locked: {reason}"),
            None => "Locked".to_string(),
        });
    }
    if worktree.prunable {
        lines.push("Its folder is gone: prune to forget it".to_string());
    }
    if !worktree.is_current && !worktree.prunable && !worktree.bare {
        lines.push("Double-click to open in a new tab".to_string());
    }
    lines.join("\n")
}

/// El menú de un worktree como datos.
pub fn worktree_menu(worktree: &Worktree, busy: bool) -> Vec<MenuEntry> {
    let path = worktree.path.clone();
    let openable = !worktree.prunable && !worktree.bare;
    let mut items = vec![
        MenuEntry::item(
            "Open in new tab",
            Lucide::ExternalLink,
            SidebarCommand::OpenWorktree(path.clone()),
        )
        .disabled(!openable || worktree.is_current),
        MenuEntry::item(
            "Copy path",
            Lucide::Copy,
            SidebarCommand::CopyWorktreePath(path.clone()),
        ),
    ];
    if !worktree.is_main {
        items.push(MenuEntry::Separator);
        items.push(if worktree.locked {
            MenuEntry::item(
                "Unlock",
                Lucide::LockOpen,
                SidebarCommand::UnlockWorktree(path.clone()),
            )
            .disabled(busy)
        } else {
            MenuEntry::item(
                "Lock",
                Lucide::Lock,
                SidebarCommand::LockWorktree(path.clone()),
            )
            .disabled(busy)
        });
        if worktree.prunable {
            items.push(
                MenuEntry::item("Prune", Lucide::Eraser, SidebarCommand::PruneWorktrees)
                    .disabled(busy || worktree.locked),
            );
        } else {
            items.push(
                MenuEntry::item(
                    "Remove worktree...",
                    Lucide::Trash,
                    SidebarCommand::RemoveWorktree(path),
                )
                .danger()
                .disabled(busy || worktree.is_current || worktree.locked),
            );
        }
    }
    items
}

fn worktree_row(
    worktree: &Worktree,
    dirty: Option<bool>,
    tooltips: bool,
    view: &WeakEntity<Sidebar>,
    cx: &mut App,
) -> AnyElement {
    let palette = *cx.palette();
    let path = worktree.path.clone();
    let (icon, tone) = if worktree.prunable {
        (Lucide::FolderX, IconTone::Danger)
    } else if worktree.locked {
        (Lucide::Lock, IconTone::Muted)
    } else {
        (Lucide::FolderGit2, IconTone::Muted)
    };
    let openable = !worktree.prunable && !worktree.bare && !worktree.is_current;
    let mono = gpui_kit::component::ActiveTheme::theme(cx)
        .mono_font_family
        .clone();

    let open = {
        let view = view.clone();
        let path = path.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let _ = view.update(cx, |this, cx| {
                this.run_command(SidebarCommand::OpenWorktree(path.clone()), window, cx);
            });
        }
    };

    let group = section::row_group("worktree", &path);
    section::item(row_id("worktree", &path), cx)
        // Ancho fijo al del tramo: las rutas y ramas largas se recortan en
        // vez de empujar los botones fuera del sidebar.
        .w_full()
        .relative()
        .group(group.clone())
        .overflow_hidden()
        .when(openable, |row| row.cursor_pointer())
        // El de esta ventana no se abre: sin resaltado de "clicable".
        .when(!worktree.is_current, |row| {
            row.hover(|style| style.bg(palette.sidebar_row_hover))
        })
        .when(tooltips, |row| {
            row.tooltip(mergeon_ui::tooltip(worktree_tooltip(worktree, dirty)))
        })
        .on_click({
            let open = open.clone();
            move |event: &ClickEvent, window, cx| {
                if openable && event.click_count() == 2 {
                    open(event, window, cx);
                }
            }
        })
        .child(if worktree.is_current {
            section::head_mark(cx).into_any_element()
        } else {
            section::item_icon(icon, tone).into_any_element()
        })
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .items_baseline()
                .gap(rems(0.4))
                // La carpeta cede antes que la rama, pero nunca del todo: las
                // dos se recortan con un reparto parejo.
                .child(
                    div()
                        .flex_shrink(1.)
                        .min_w(rems(2.5))
                        .max_w(gpui_kit::relative(0.6))
                        .truncate()
                        .when(worktree.is_current, |name| {
                            name.font_weight(gpui_kit::FontWeight::MEDIUM)
                        })
                        .when(worktree.prunable, |name| name.line_through())
                        .child(worktree.name().to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(2.5))
                        .truncate()
                        .font_family(mono)
                        .text_size(px(11.))
                        .text_color(palette.text_3)
                        .child(worktree_target_label(worktree)),
                ),
        )
        .when(dirty == Some(true), |row| {
            row.child(
                div()
                    .id(row_id("worktree-dirty", &path))
                    .test_support()
                    .flex_shrink_0()
                    .size(px(7.))
                    .rounded_full()
                    .bg(palette.warning)
                    .tooltip(mergeon_ui::tooltip("Uncommitted changes")),
            )
        })
        // Sin nada que abrir no hay botones (ni su hueco).
        .when(openable, |row| {
            row.child(
                section::item_actions_on_hover(group, cx).child(
                    section::item_action(
                        row_id("worktree-open", &path),
                        Lucide::ExternalLink,
                        "Open in new tab",
                        false,
                        true,
                        cx,
                        open,
                    )
                    .test_support(),
                ),
            )
        })
        .test_support()
        .context_menu_items({
            let view = view.clone();
            move |_, cx| {
                let entries = view
                    .upgrade()
                    .map(|entity| entity.read(cx).worktree_menu(&path, cx))
                    .unwrap_or_default();
                menu_items(&view, entries)
            }
        })
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Flujos
// ---------------------------------------------------------------------------

/// Abre la carpeta del worktree como repositorio, en otra pestaña (si ya
/// está abierta, la activa).
pub(crate) fn open_worktree(model: &Entity<AppModel>, path: &str, cx: &mut App) {
    model
        .update(cx, |model, cx| {
            model.add_repository(path, AddRepository::default(), cx)
        })
        .detach();
}

fn add_worktree(
    model: &Entity<AppModel>,
    repo: RepoRef,
    options: mergeon_git_ops::AddWorktreeOptions,
    open_after: bool,
    cx: &mut App,
) -> Task<()> {
    flows::single_flight(
        model,
        &repo.clone(),
        "worktree-add",
        cx,
        async move |model, cx| {
            let path = options.path.clone();
            let result = model
                .update(cx, |model, cx| model.add_worktree(&repo.path, options, cx))
                .await;
            flows::apply(&model, &repo, &result, cx);
            if !result.to_operation_result().success {
                return;
            }
            // Una rama nueva aparece en Branches.
            flows::refresh(&model, &repo, SyncRequest::new().branches(true), cx).await;
            if open_after {
                cx.update(|cx| open_worktree(&model, &path, cx));
            }
        },
    )
}

fn remove_worktree(
    model: &Entity<AppModel>,
    repo: RepoRef,
    worktree: Worktree,
    dirty: Option<bool>,
    cx: &mut App,
) -> Task<()> {
    let branch = worktree
        .branch
        .as_deref()
        .map(|branch| format!(" The branch '{branch}' stays."))
        .unwrap_or_default();
    let warning = if dirty == Some(true) {
        " It has uncommitted changes: git keeps the worktree unless you tick the option, and \
         then they are lost for good."
    } else {
        ""
    };
    let confirm = model.update(cx, |model, cx| {
        model.confirm_with_option(
            ConfirmOptions {
                title: Some(format!("Remove worktree '{}'?", worktree.name())),
                description: Some(format!(
                    "This deletes the folder {}.{branch}{warning}",
                    worktree.path
                )),
                confirm_text: Some("Remove".to_string()),
                cancel_text: Some("Cancel".to_string()),
                variant: Some("destructive".to_string()),
                option_label: Some("Also discard its uncommitted changes (--force)".to_string()),
                option_checked: false,
                ..Default::default()
            },
            cx,
        )
    });
    let model = model.clone();
    cx.spawn(async move |cx| {
        let answer = confirm.await;
        if !answer.confirmed() {
            return;
        }
        let force = matches!(answer, ConfirmAnswer::WithOption { checked: true, .. });
        let path = worktree.path.clone();
        let task = cx.update(|cx| {
            flows::single_flight(
                &model,
                &repo.clone(),
                "worktree-remove",
                cx,
                async move |model, cx| {
                    let result = model
                        .update(cx, |model, cx| {
                            model.remove_worktree(&repo.path, &path, force, cx)
                        })
                        .await;
                    flows::apply(&model, &repo, &result, cx);
                },
            )
        });
        task.await;
    })
}

fn set_locked(
    model: &Entity<AppModel>,
    repo: RepoRef,
    path: String,
    lock: bool,
    cx: &mut App,
) -> Task<()> {
    let operation = if lock {
        "worktree-lock"
    } else {
        "worktree-unlock"
    };
    flows::single_flight(
        model,
        &repo.clone(),
        operation,
        cx,
        async move |model, cx| {
            let result = model
                .update(cx, |model, cx| {
                    model.set_worktree_locked(&repo.path, &path, lock, None, cx)
                })
                .await;
            flows::apply(&model, &repo, &result, cx);
        },
    )
}

fn prune(model: &Entity<AppModel>, repo: RepoRef, cx: &mut App) -> Task<()> {
    flows::single_flight(
        model,
        &repo.clone(),
        "worktree-prune",
        cx,
        async move |model, cx| {
            let result = model
                .update(cx, |model, cx| model.prune_worktrees(&repo.path, cx))
                .await;
            flows::apply(&model, &repo, &result, cx);
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(path: &str) -> Worktree {
        Worktree {
            path: path.to_string(),
            head: Some("0123456789abcdef".to_string()),
            branch: Some("feature".to_string()),
            ..Default::default()
        }
    }

    fn labels(entries: &[MenuEntry]) -> Vec<&str> {
        entries.iter().map(MenuEntry::label).collect()
    }

    #[test]
    fn labels_what_each_worktree_has_checked_out() {
        let mut item = worktree("/code/shop-feature");
        assert_eq!(worktree_target_label(&item), "feature");
        item.branch = None;
        item.detached = true;
        assert_eq!(worktree_target_label(&item), "detached at 0123456");
        item.bare = true;
        assert_eq!(worktree_target_label(&item), "bare");
    }

    #[test]
    fn the_main_and_current_worktrees_cannot_be_removed() {
        let mut main = worktree("/code/shop");
        main.is_main = true;
        main.is_current = true;
        assert_eq!(
            labels(&worktree_menu(&main, false)),
            ["Open in new tab", "Copy path"]
        );
        assert!(worktree_menu(&main, false)[0].is_disabled());

        let linked = worktree("/code/shop-feature");
        let menu = worktree_menu(&linked, false);
        assert_eq!(
            labels(&menu),
            [
                "Open in new tab",
                "Copy path",
                "",
                "Lock",
                "Remove worktree..."
            ]
        );
        assert!(menu[4].is_danger() && !menu[4].is_disabled());

        let mut locked = linked.clone();
        locked.locked = true;
        let menu = worktree_menu(&locked, false);
        assert_eq!(menu[3].label(), "Unlock");
        assert!(menu[4].is_disabled(), "a locked worktree is unlocked first");

        let mut gone = linked;
        gone.prunable = true;
        let menu = worktree_menu(&gone, false);
        assert!(menu[0].is_disabled());
        assert_eq!(menu[4].label(), "Prune");
    }

    #[test]
    fn the_tooltip_says_where_it_is_and_what_is_going_on() {
        let mut item = worktree("/code/shop-feature");
        item.locked = true;
        item.lock_reason = Some("usb".into());
        let text = worktree_tooltip(&item, Some(true));
        assert!(text.contains("/code/shop-feature"));
        assert!(text.contains("Has uncommitted changes"));
        assert!(text.contains("Locked: usb"));
        assert!(text.contains("Double-click to open in a new tab"));
    }
}
