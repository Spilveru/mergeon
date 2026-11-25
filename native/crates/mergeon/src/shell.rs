//! Cáscara de la aplicación (`App.vue`): la barra de título con las pestañas
//! de proyecto, "+" (abrir repositorio) tras ellas y, en su borde derecho, la
//! lista de repositorios, el log de operaciones y Settings; debajo, el
//! contenido (inicio, ajustes o el espacio de trabajo del repositorio activo).
//!
//! Ya no hay barra de actividad lateral: sus botones caben en la barra de
//! título y la app gana 48px de ancho en todas las vistas.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui_kit::Focusable as _;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, MouseButton, ScrollHandle, SharedString, Subscription, Window, div, px,
};
use mergeon_app::shell::WindowRequest;
use mergeon_app::{AppEvent, AppModel, RepositoryTabLabel};
use mergeon_model::keyboard_shortcuts::{ShortcutAction, effective_combo_label};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::style::text;
use mergeon_ui::{
    ActivePaletteExt, IconTone, Lucide, Radius, SpinnerColor, SpinnerSize, ThemePreference,
};
use mergeon_updater::UpdateStatus;
use mergeon_view_home::{HomeView, SettingsEvent, SettingsView};
use mergeon_view_sidebar::{Sidebar, SidebarEvent};
use mergeon_view_toolbar::{
    HistoryPosition, OperationStatusSlot, StatusBarView, ToolbarEvent, ToolbarView,
};

use crate::commands::{self, COMMANDS, Command, CommandContext, Picker};
use crate::file_index::FileIndex;
use crate::palette::{
    CommandPalette, DEFAULT_PLACEHOLDER, FileIntent, PaletteAction, PaletteEntry, PaletteEvent,
};
use crate::views;
use crate::workspace::{RepositoryWorkspaceView, insertion_marker};
use mergeon_model::command_palette::PaletteKind;
use mergeon_model::workspace_tabs::FileTabDescriptor;
use mergeon_ui::dialog::{AppDialog, DialogId, DialogWidth};
use mergeon_view_sidebar::SidebarCommand;

const TAB_BAR_HEIGHT: f32 = 36.0;
/// Ancho máximo de una pestaña (el nombre se corta con elipsis). Las pestañas
/// no se encogen por debajo de su contenido, como en VS Code y Zed: con
/// muchas, la tira se desplaza (rueda), la activa se trae a la vista y "▾"
/// las lista todas.
const TAB_MAX_WIDTH: f32 = 220.0;

/// Un botón de solo ícono del borde de la barra de título.
struct TitleBarButton {
    id: &'static str,
    icon: Lucide,
    tooltip: SharedString,
    active: bool,
    /// Punto de aviso (errores sin ver, actualización disponible).
    dot: Option<gpui_kit::Hsla>,
}

/// Qué marca lleva una pestaña de repositorio junto al nombre. El color se
/// reserva para lo que pide atención; los cambios sin commitear son lo normal
/// y van en gris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabMark {
    None,
    /// Cambios locales sin commitear (punto neutro).
    Dirty,
    /// Merge, rebase, cherry-pick… a medias (punto `warning`).
    Operation,
    /// Conflictos sin resolver (punto `destructive`).
    Conflict,
}

impl TabMark {
    fn describe(self) -> Option<&'static str> {
        match self {
            TabMark::None => None,
            TabMark::Dirty => Some("Uncommitted changes"),
            TabMark::Operation => Some("Operation in progress"),
            TabMark::Conflict => Some("Merge conflicts"),
        }
    }
}

/// Una pestaña de la barra de título (Home, un repositorio o Settings).
struct ProjectTab {
    id: SharedString,
    icon: Lucide,
    /// Tono `active`/`muted` del ícono, como las pestañas de repositorio.
    icon_active_tone: bool,
    name: SharedString,
    /// Carpeta que distingue a un repositorio de otro con el mismo nombre.
    suffix: Option<SharedString>,
    title: Option<SharedString>,
    active: bool,
    mark: TabMark,
    /// Operación en curso o en cola (spinner en lugar del ícono).
    busy: bool,
    closable: bool,
    /// Solo en las de repositorio: menú contextual y arrastre.
    repository: Option<RepositoryTabMenu>,
}

/// Lo que la pestaña de un repositorio necesita para su menú y su arrastre.
#[derive(Clone)]
struct RepositoryTabMenu {
    index: usize,
    id: String,
    path: String,
    label: SharedString,
    is_last: bool,
    is_only: bool,
}

/// Lo que la barra lee de cada repositorio abierto.
#[derive(Debug, Clone, PartialEq)]
struct RepositoryTabInfo {
    id: String,
    path: String,
    label: RepositoryTabLabel,
    mark: TabMark,
    busy: bool,
}

/// Pestaña de repositorio que se está arrastrando.
#[derive(Clone)]
struct DraggedRepositoryTab {
    index: usize,
    label: SharedString,
}

/// Lo que se ve bajo el puntero al arrastrar una pestaña.
struct DragPreview {
    label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .px_3()
            .py_1()
            .rounded(Radius::Xl.px(cx))
            .border_1()
            .border_color(palette.border_base)
            .bg(palette.bg_raised)
            .text_size(text::BODY)
            .text_color(palette.text_1)
            .child(self.label.clone())
    }
}

/// Una acción del modelo que puede invocarse varias veces.
type ModelAction = Rc<dyn Fn(&mut AppModel, &mut Context<AppModel>)>;

/// Lo que hace una pestaña al cerrarse (botón y clic central).
type TabCloseHandler = Rc<dyn Fn(&mut AppShell, &mut Context<AppShell>)>;

/// Menú contextual de una pestaña de repositorio.
fn repository_tab_menu(
    model: &Entity<AppModel>,
    tab: &RepositoryTabMenu,
    shortcuts: &BTreeMap<String, String>,
) -> Vec<MenuItem> {
    let action = |run: ModelAction| {
        let model = model.clone();
        move |_: &mut Window, cx: &mut gpui_kit::App| {
            let run = run.clone();
            model.update(cx, move |model, cx| run(model, cx));
        }
    };
    let id = tab.id.clone();
    let close = {
        let id = id.clone();
        action(Rc::new(move |model, cx| {
            model
                .request_close_repositories(vec![id.clone()], cx)
                .detach();
        }))
    };
    let close_others = {
        let id = id.clone();
        action(Rc::new(move |model, cx| {
            model.close_other_repositories(&id, cx).detach();
        }))
    };
    let close_right = {
        let id = id.clone();
        action(Rc::new(move |model, cx| {
            model.close_repositories_to_the_right(&id, cx).detach();
        }))
    };
    let reveal = {
        let path = tab.path.clone();
        action(Rc::new(move |model, cx| {
            model.show_item_in_folder(&path, cx).detach();
        }))
    };
    let open_in_editor = {
        let path = tab.path.clone();
        action(Rc::new(move |model, cx| {
            model.open_in_editor(&path, None, cx).detach();
        }))
    };
    let copy_path = {
        let path = tab.path.clone();
        move |_: &mut Window, cx: &mut gpui_kit::App| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.clone()));
        }
    };
    // `mod+w` cierra documentos; el repositorio, `mod+shift+w` (o lo que el
    // usuario haya configurado).
    let close_shortcut = effective_combo_label(
        ShortcutAction::CloseRepository,
        shortcuts,
        cfg!(target_os = "macos"),
    );
    let mut close_item = MenuItem::new("Close").icon(Lucide::X).on_click(close);
    if let Some(shortcut) = close_shortcut {
        close_item = close_item.shortcut(shortcut);
    }
    vec![
        close_item,
        MenuItem::new("Close others")
            .disabled(tab.is_only)
            .on_click(close_others),
        MenuItem::new("Close to the right")
            .disabled(tab.is_last)
            .on_click(close_right),
        MenuItem::separator(),
        MenuItem::new("Copy path")
            .icon(Lucide::Copy)
            .on_click(copy_path),
        MenuItem::new("Show in folder")
            .icon(Lucide::FolderOpen)
            .on_click(reveal),
        MenuItem::new("Open in editor")
            .icon(Lucide::CodeXml)
            .on_click(open_in_editor),
    ]
}

/// Si una acción de la paleta trabaja sobre el repositorio activo (y por
/// tanto hay que volver al que lo era al abrirla).
fn palette_action_uses_repository(action: &PaletteAction) -> bool {
    use ShortcutAction as A;
    match action {
        PaletteAction::OpenRepository { .. } | PaletteAction::OpenRecent { .. } => false,
        PaletteAction::Branch(_)
        | PaletteAction::RemoteBranch { .. }
        | PaletteAction::File { .. }
        | PaletteAction::FileHistory { .. }
        | PaletteAction::Compare { .. }
        | PaletteAction::Fixup { .. }
        | PaletteAction::PullRequest { .. } => true,
        PaletteAction::Command(command) => !matches!(
            command,
            Command::GoHome
                | Command::ToggleTheme
                | Command::Scope(_)
                | Command::Shortcut(
                    A::OpenRepository
                        | A::CloneFromGitHub
                        | A::CloneFromUrl
                        | A::ReloadApplication
                        | A::Quit
                        | A::MinimizeWindow
                        | A::ToggleDevTools
                        | A::OpenPreferences
                        | A::OpenShortcutsHelp
                        | A::ShowOperationsLog
                        | A::ReopenClosedRepository
                        | A::NextRepositoryTab
                        | A::PreviousRepositoryTab
                        | A::SelectRepositoryTab(_)
                        | A::OpenCommandPalette
                )
        ),
    }
}

/// Commit seleccionado (1-based) y commits cargados en el historial de un
/// espacio de trabajo; `None` mientras no hay historial cargado.
fn history_position(
    workspace: &Entity<RepositoryWorkspaceView>,
    cx: &gpui_kit::App,
) -> Option<HistoryPosition> {
    let commits = workspace.read(cx).commits().read(cx);
    let rows = commits.commits();
    if rows.is_empty() {
        return None;
    }
    let selected = commits.selected_commit().and_then(|selected| {
        rows.iter()
            .position(|row| row.full_hash == selected.full_hash && row.hash == selected.hash)
            .map(|index| index + 1)
    });
    Some(HistoryPosition {
        selected,
        total: rows.len(),
    })
}

/// Lo que la cáscara pinta del modelo (rail, pestañas y qué contenido va
/// debajo). Solo se repinta si cambia; lo de dentro de cada vista hija lo
/// vigila esa vista.
#[derive(Debug, Clone, PartialEq)]
struct ShellState {
    /// Lo que pinta cada pestaña de repositorio.
    tabs: Vec<RepositoryTabInfo>,
    active_index: Option<usize>,
    settings_tab_open: bool,
    show_preferences: bool,
    restoring: bool,
    unseen_errors: bool,
    update_status: UpdateStatus,
    keyboard_shortcuts: BTreeMap<String, String>,
}

impl ShellState {
    fn read(model: &AppModel) -> Self {
        Self {
            tabs: model
                .open_repositories()
                .iter()
                .zip(model.repository_tab_labels())
                .map(|(repo, label)| RepositoryTabInfo {
                    id: repo.id.clone(),
                    path: repo.path.clone(),
                    label,
                    mark: if repo.conflict_state.in_conflict {
                        TabMark::Conflict
                    } else if repo
                        .operation_state
                        .as_ref()
                        .is_some_and(|state| state.in_progress)
                    {
                        TabMark::Operation
                    } else if repo.is_dirty {
                        TabMark::Dirty
                    } else {
                        TabMark::None
                    },
                    busy: repo.mutation_queue.active.is_some()
                        || !repo.mutation_queue.queued.is_empty(),
                })
                .collect(),
            active_index: model.active_repository_index(),
            settings_tab_open: model.settings_tab_open(),
            show_preferences: model.show_preferences(),
            restoring: model.is_restoring_workspace(),
            unseen_errors: model.operations_log().unseen_errors > 0,
            update_status: model.updater_state().status,
            keyboard_shortcuts: model.keyboard_shortcuts().clone(),
        }
    }
}

/// La cáscara de la ventana, para quien solo tiene `App` (el atajo de la
/// paleta llega como acción global).
pub struct ShellHandle(pub gpui_kit::WeakEntity<AppShell>);

impl gpui_kit::Global for ShellHandle {}

/// El repositorio activo al abrir la paleta: sus acciones van a él.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteTarget {
    pub repo_id: String,
    pub repo_path: String,
}

/// Cómo se abre la paleta: la de siempre o como selector de un comando.
#[derive(Debug, Clone, Default)]
struct PaletteOptions {
    placeholder: Option<String>,
    /// La consulta con la que empieza (un prefijo como `/`).
    query: Option<String>,
    /// Con los archivos del repositorio, y qué hace uno elegido.
    files: Option<FileIntent>,
}

struct OpenPalette {
    dialog: DialogId,
    view: Entity<CommandPalette>,
    _subscriptions: Vec<Subscription>,
}

pub struct AppShell {
    model: Entity<AppModel>,
    shown: ShellState,
    workspaces: HashMap<String, Entity<RepositoryWorkspaceView>>,
    toolbar: Entity<ToolbarView>,
    operation_status: Entity<OperationStatusSlot>,
    status_bar: Entity<StatusBarView>,
    sidebar: Entity<Sidebar>,
    home: Entity<HomeView>,
    settings: Entity<SettingsView>,
    /// Scroll de la tira de pestañas de proyecto.
    tab_scroll: ScrollHandle,
    /// Pestaña que se trajo a la vista por última vez: hijo de la tira,
    /// número de repositorios y lo que cambia su ancho (punto, spinner).
    scrolled_tab: Option<(usize, usize, bool, bool)>,
    /// Archivos de cada repositorio para "Go to file…".
    file_index: Entity<FileIndex>,
    palette: Option<OpenPalette>,
    _subscriptions: Vec<Subscription>,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        cx.set_global(ShellHandle(cx.entity().downgrade()));
        let initial_shortcuts = model.read(cx).keyboard_shortcuts().clone();
        if !initial_shortcuts.is_empty() {
            crate::rebind_shortcuts(&initial_shortcuts, cx);
        }
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let home = cx.new(|cx| HomeView::new(window, cx));
        let settings = cx.new(|cx| SettingsView::new(window, cx));
        let toolbar = cx.new(|cx| ToolbarView::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &toolbar,
                window,
                |this, _, event: &ToolbarEvent, window, cx| match event {
                    ToolbarEvent::OpenBranchPicker => this.open_branch_picker(window, cx),
                },
            ),
            cx.subscribe_in(&sidebar, window, Self::handle_sidebar_event),
            cx.subscribe(&settings, |this, _, event: &SettingsEvent, cx| {
                if matches!(event, SettingsEvent::Close) {
                    this.model
                        .update(cx, |model, cx| model.hide_preferences(cx));
                }
            }),
            cx.observe(&model, |this: &mut Self, model, cx| {
                let state = ShellState::read(model.read(cx));
                if state != this.shown {
                    this.shown = state;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&model, window, Self::handle_app_event),
            cx.observe_window_activation(window, |this, window, cx| {
                let focused = window.is_window_active();
                this.model
                    .update(cx, |model, cx| model.set_window_focused(focused, cx));
            }),
        ];
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| {
                let state = ShellState::read(this.model.read(cx));
                if state != this.shown {
                    this.shown = state;
                    cx.notify();
                }
            })
            .ok();
        });
        Self {
            shown: ShellState::read(model.read(cx)),
            model,
            workspaces: HashMap::new(),
            toolbar,
            operation_status: cx.new(|cx| OperationStatusSlot::new(window, cx)),
            status_bar: cx.new(|cx| StatusBarView::new(window, cx)),
            sidebar,
            home,
            settings,
            tab_scroll: ScrollHandle::new(),
            scrolled_tab: None,
            file_index: cx.new(|_| FileIndex::new()),
            palette: None,
            _subscriptions: subscriptions,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn file_index(&self) -> &Entity<FileIndex> {
        &self.file_index
    }

    /// El espacio de trabajo montado de un repositorio.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn workspace(&self, repo_id: &str) -> Option<&Entity<RepositoryWorkspaceView>> {
        self.workspaces.get(repo_id)
    }

    /// La paleta abierta, si la hay.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn command_palette(&self) -> Option<&Entity<CommandPalette>> {
        self.palette.as_ref().map(|open| &open.view)
    }

    /// Lo que lista la paleta: comandos, repositorios y las ramas del
    /// repositorio activo (los archivos llegan aparte).
    fn palette_entries(&self, cx: &gpui_kit::App) -> Vec<PaletteEntry> {
        let model = self.model.read(cx);
        let context = CommandContext::read(model);
        let skip_action_dialogs = model.skip_action_dialogs();
        let shortcuts = model.keyboard_shortcuts();
        let mut entries: Vec<PaletteEntry> = COMMANDS
            .iter()
            .filter(|spec| spec.visible(skip_action_dialogs))
            .map(|spec| PaletteEntry {
                kind: PaletteKind::Command,
                icon: spec.icon,
                label: spec.display_label(skip_action_dialogs).into(),
                context: spec.group.into(),
                shortcut: spec.shortcut_label_with(shortcuts).map(Into::into),
                enabled: spec.available(context),
                action: PaletteAction::Command(spec.command),
            })
            .collect();

        let active = model.active_repository_index();
        let open = model.open_repositories();
        for (index, (repo, label)) in open.iter().zip(model.repository_tab_labels()).enumerate() {
            entries.push(PaletteEntry {
                kind: PaletteKind::Repository,
                icon: Lucide::FolderGit2,
                label: label.text().into(),
                context: if active == Some(index) {
                    format!("Current · {}", repo.path).into()
                } else {
                    repo.path.clone().into()
                },
                shortcut: None,
                enabled: true,
                action: PaletteAction::OpenRepository {
                    id: repo.id.clone(),
                },
            });
        }
        for recent in model.recent_repositories() {
            if open.iter().any(|repo| repo.path == recent.path) {
                continue;
            }
            entries.push(PaletteEntry {
                kind: PaletteKind::Repository,
                icon: Lucide::Clock,
                label: recent.name.clone().into(),
                context: format!("Recent · {}", recent.path).into(),
                shortcut: None,
                enabled: true,
                action: PaletteAction::OpenRecent {
                    path: recent.path.clone(),
                },
            });
        }

        if let Some(repo) = model.active_repository() {
            for branch in repo.branches.iter().filter(|branch| !branch.is_remote) {
                entries.push(PaletteEntry {
                    kind: PaletteKind::Branch,
                    icon: Lucide::GitBranch,
                    label: branch.name.clone().into(),
                    context: if branch.active {
                        "Current branch".into()
                    } else {
                        match &branch.upstream {
                            Some(upstream) => format!("Local · {upstream}").into(),
                            None => "Local branch".into(),
                        }
                    },
                    shortcut: None,
                    enabled: !branch.active,
                    action: PaletteAction::Branch(branch.name.clone()),
                });
            }
            for remote in &repo.remotes {
                for branch in remote.branches.iter().filter(|branch| *branch != "HEAD") {
                    entries.push(PaletteEntry {
                        kind: PaletteKind::Branch,
                        icon: Lucide::Globe,
                        label: format!("{}/{branch}", remote.name).into(),
                        context: "Remote branch".into(),
                        shortcut: None,
                        enabled: true,
                        action: PaletteAction::RemoteBranch {
                            remote: remote.name.clone(),
                            branch: branch.clone(),
                        },
                    });
                }
            }
        }
        entries
    }

    /// Ctrl+Shift+P / ⌘⇧P: abre la paleta sobre el repositorio activo.
    pub fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &self.palette {
            if mergeon_ui::dialog::open_count(window, cx) > 0 {
                let input = open.view.read(cx).input().clone();
                let handle = input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                return;
            }
            self.palette = None;
        }
        // Encima de otro diálogo, no.
        if mergeon_ui::dialog::open_count(window, cx) > 0 {
            return;
        }

        let target = self
            .model
            .read(cx)
            .active_repository()
            .map(|repo| PaletteTarget {
                repo_id: repo.id.clone(),
                repo_path: repo.path.clone(),
            });
        let entries = self.palette_entries(cx);
        let options = PaletteOptions {
            files: Some(FileIntent::Open),
            ..PaletteOptions::default()
        };
        self.show_palette(entries, options, target, window, cx);
    }

    /// Clic en la rama de la barra: la paleta con solo las ramas (`@`).
    pub fn open_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if mergeon_ui::dialog::open_count(window, cx) > 0 {
            return;
        }
        let target = self
            .model
            .read(cx)
            .active_repository()
            .map(|repo| PaletteTarget {
                repo_id: repo.id.clone(),
                repo_path: repo.path.clone(),
            });
        if target.is_none() {
            return;
        }
        let entries = self.palette_entries(cx);
        let options = PaletteOptions {
            placeholder: Some("Switch branch… (type a name)".to_string()),
            query: Some(
                mergeon_model::command_palette::PaletteScope::Branches
                    .prefix()
                    .to_string(),
            ),
            files: None,
        };
        self.show_palette(entries, options, target, window, cx);
    }

    /// Los comandos que piden elegir algo ("File history…", "Compare with
    /// current branch…", "Fixup into…") reabren la paleta con esa lista,
    /// sobre el mismo repositorio.
    fn open_picker(
        &mut self,
        picker: Picker,
        target: Option<PaletteTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.is_none() || mergeon_ui::dialog::open_count(window, cx) > 0 {
            return;
        }
        let (entries, options) = match picker {
            Picker::FileHistory => (
                Vec::new(),
                PaletteOptions {
                    placeholder: Some("File history of… (type a path)".to_string()),
                    query: Some("/".to_string()),
                    files: Some(FileIntent::History),
                },
            ),
            Picker::CompareBranch => {
                let current = self
                    .model
                    .read(cx)
                    .active_repository()
                    .and_then(|repo| repo.current_branch.clone())
                    .filter(|branch| !branch.is_empty())
                    .unwrap_or_else(|| "HEAD".to_string());
                (
                    self.compare_entries(cx),
                    PaletteOptions {
                        placeholder: Some(format!("Compare '{current}' with… (branch)")),
                        ..PaletteOptions::default()
                    },
                )
            }
            Picker::FixupTarget => {
                let entries = target
                    .as_ref()
                    .map(|target| self.fixup_entries(&target.repo_id, cx))
                    .unwrap_or_default();
                if entries.is_empty() {
                    self.model.update(cx, |model, cx| {
                        model.notify_info("There are no commits to fix up on this branch.", cx);
                    });
                    return;
                }
                (
                    entries,
                    PaletteOptions {
                        placeholder: Some("Fixup into… (commit)".to_string()),
                        ..PaletteOptions::default()
                    },
                )
            }
            Picker::PullRequest => {
                let entries = self.pull_request_entries(cx);
                if entries.is_empty() {
                    self.model.update(cx, |model, cx| {
                        model.notify_info("There are no open pull requests to check out.", cx);
                    });
                    return;
                }
                (
                    entries,
                    PaletteOptions {
                        placeholder: Some("Checkout pull request… (number or title)".to_string()),
                        ..PaletteOptions::default()
                    },
                )
            }
        };
        self.show_palette(entries, options, target, window, cx);
    }

    /// "Compare with current branch…": las ramas locales (menos la actual) y
    /// remotas del repositorio activo.
    fn compare_entries(&self, cx: &gpui_kit::App) -> Vec<PaletteEntry> {
        let Some(repo) = self.model.read(cx).active_repository() else {
            return Vec::new();
        };
        let local = repo
            .branches
            .iter()
            .filter(|branch| !branch.is_remote && !branch.active)
            .map(|branch| PaletteEntry {
                kind: PaletteKind::Branch,
                icon: Lucide::GitBranch,
                label: branch.name.clone().into(),
                context: "Local branch".into(),
                shortcut: None,
                enabled: true,
                action: PaletteAction::Compare {
                    reference: branch.name.clone(),
                },
            });
        let remote = repo.remotes.iter().flat_map(|remote| {
            remote
                .branches
                .iter()
                .filter(|branch| *branch != "HEAD")
                .map(|branch| {
                    let reference = format!("{}/{branch}", remote.name);
                    PaletteEntry {
                        kind: PaletteKind::Branch,
                        icon: Lucide::Globe,
                        label: reference.clone().into(),
                        context: "Remote branch".into(),
                        shortcut: None,
                        enabled: true,
                        action: PaletteAction::Compare { reference },
                    }
                })
        });
        local.chain(remote).collect()
    }

    /// "Fixup into…": los commits que ofrece la caja de commit de Local
    /// Changes (ya leídos fuera del hilo de la interfaz).
    fn fixup_entries(&self, repo_id: &str, cx: &gpui_kit::App) -> Vec<PaletteEntry> {
        let Some(workspace) = self.workspaces.get(repo_id) else {
            return Vec::new();
        };
        let changes = workspace.read(cx).changes().read(cx);
        changes
            .fixup_targets()
            .iter()
            .map(|target| PaletteEntry {
                kind: PaletteKind::Command,
                icon: Lucide::GitCommitHorizontal,
                label: format!("{} {}", target.short_hash, target.subject).into(),
                context: if target.published {
                    "Pushed · asks first".into()
                } else {
                    "Not pushed".into()
                },
                shortcut: None,
                enabled: true,
                action: PaletteAction::Fixup {
                    hash: target.hash.clone(),
                },
            })
            .collect()
    }

    /// "Checkout pull request…": los PRs abiertos que conoce el modelo del
    /// repositorio activo (ya leídos por la sección del sidebar).
    fn pull_request_entries(&self, cx: &gpui_kit::App) -> Vec<PaletteEntry> {
        let Some(state) = self.model.read(cx).active_pull_requests() else {
            return Vec::new();
        };
        state
            .pulls
            .iter()
            .map(|item| {
                let pull = &item.pull;
                PaletteEntry {
                    kind: PaletteKind::Command,
                    icon: Lucide::GitPullRequest,
                    label: format!("#{} {}", pull.number, pull.title).into(),
                    context: if pull.head_ref.is_empty() {
                        SharedString::default()
                    } else {
                        pull.head_ref.clone().into()
                    },
                    shortcut: None,
                    enabled: true,
                    action: PaletteAction::PullRequest {
                        number: pull.number,
                    },
                }
            })
            .collect()
    }

    /// Abre el diálogo de la paleta con estas entradas (y los archivos de
    /// `target` si `options.files` lo pide).
    fn show_palette(
        &mut self,
        entries: Vec<PaletteEntry>,
        options: PaletteOptions,
        target: Option<PaletteTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| {
            CommandPalette::with_options(
                entries,
                options
                    .placeholder
                    .as_deref()
                    .unwrap_or(DEFAULT_PLACEHOLDER),
                options.files.unwrap_or_default(),
                window,
                cx,
            )
        });
        if let Some(query) = &options.query {
            view.update(cx, |view, cx| view.set_query(query, window, cx));
        }

        let mut subscriptions = Vec::new();
        if let Some(target) = target.as_ref().filter(|_| options.files.is_some()) {
            let path = target.repo_path.clone();
            self.file_index
                .update(cx, |index, cx| index.ensure(&path, cx));
            let sync_files = {
                let view = view.clone();
                let path = path.clone();
                move |index: &Entity<FileIndex>, cx: &mut gpui_kit::App| {
                    let (files, loading) = {
                        let index = index.read(cx);
                        (
                            index.get(&path).cloned().unwrap_or_default(),
                            index.is_loading(&path),
                        )
                    };
                    view.update(cx, |view, cx| view.set_files(files, loading, cx));
                }
            };
            sync_files(&self.file_index, cx);
            subscriptions.push(cx.observe(&self.file_index, move |_, index, cx| {
                sync_files(&index, cx);
            }));
        }
        subscriptions.push(cx.subscribe_in(&view, window, {
            let target = target.clone();
            move |this: &mut Self, _, event: &PaletteEvent, window, cx| {
                this.close_command_palette(window, cx);
                if let PaletteEvent::Run(action) = event {
                    this.run_palette_action(action.clone(), target.clone(), window, cx);
                }
            }
        }));

        let shell = cx.entity().downgrade();
        let dialog = AppDialog::new()
            .width(DialogWidth::Xl2)
            .top_anchored()
            .flush_body()
            .close_button(false)
            .body({
                let view = view.clone();
                move |_, _| view.clone()
            })
            .on_close(move |_, cx| {
                shell.update(cx, |shell, _| shell.palette = None).ok();
            })
            .open(window, cx);
        let handle = view.read(cx).input().read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.palette = Some(OpenPalette {
            dialog,
            view,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.palette.take() {
            mergeon_ui::dialog::close_id(window, cx, open.dialog);
        }
    }

    /// Ejecuta lo elegido en la paleta sobre el repositorio que estaba activo
    /// al abrirla (si entretanto se cerró, se avisa y no se hace nada).
    pub fn run_palette_action(
        &mut self,
        action: PaletteAction,
        target: Option<PaletteTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        if palette_action_uses_repository(&action)
            && let Some(target) = &target
        {
            let (index, active, settings) = {
                let model = model.read(cx);
                (
                    model
                        .open_repositories()
                        .iter()
                        .position(|repo| repo.id == target.repo_id),
                    model.active_repository_index(),
                    model.show_preferences(),
                )
            };
            match index {
                None => {
                    model.update(cx, |model, cx| {
                        model.notify_warning("That repository is no longer open.", cx);
                    });
                    return;
                }
                Some(index) if active != Some(index) || settings => {
                    model.update(cx, |model, cx| model.select_repository_tab(index, cx));
                }
                Some(_) => {}
            }
        }

        match action {
            PaletteAction::Command(Command::Pick(picker)) => {
                self.open_picker(picker, target, window, cx);
            }
            PaletteAction::Command(Command::Sidebar(task)) => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.run_command(task.command(), window, cx);
                });
            }
            PaletteAction::Command(command) => commands::run(command, &model, cx),
            PaletteAction::PullRequest { number } => {
                let section = self.sidebar.read(cx).pull_requests_section().clone();
                section.update(cx, |section, cx| {
                    section.run_command(
                        mergeon_view_sidebar::PullRequestCommand::Checkout(number),
                        window,
                        cx,
                    );
                });
            }
            PaletteAction::Branch(name) => self.sidebar.update(cx, |sidebar, cx| {
                sidebar.run_command(SidebarCommand::CheckoutBranch(name), window, cx);
            }),
            PaletteAction::RemoteBranch { remote, branch } => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.run_command(
                        SidebarCommand::CheckoutRemoteBranch { remote, branch },
                        window,
                        cx,
                    );
                })
            }
            PaletteAction::OpenRepository { id } => model.update(cx, |model, cx| {
                if let Some(index) = model
                    .open_repositories()
                    .iter()
                    .position(|repo| repo.id == id)
                {
                    model.select_repository_tab(index, cx);
                }
            }),
            PaletteAction::OpenRecent { path } => model.update(cx, |model, cx| {
                model
                    .open_repository_path(
                        &path,
                        "The selected directory is not a valid Git repository",
                        cx,
                    )
                    .detach();
            }),
            PaletteAction::File { path, head } => {
                let Some(target) = target else {
                    return;
                };
                // La paleta reutiliza la pestaña de vista previa.
                model.update(cx, |model, cx| {
                    model.open_workspace_file_preview(
                        &target.repo_path,
                        &FileTabDescriptor::tree(path, &head),
                        cx,
                    );
                });
            }
            PaletteAction::FileHistory { path } => {
                let Some(target) = target else {
                    return;
                };
                model.update(cx, |model, cx| {
                    model.open_workspace_file_preview(
                        &target.repo_path,
                        &FileTabDescriptor::history(path, None),
                        cx,
                    );
                });
            }
            PaletteAction::Compare { reference } => self.sidebar.update(cx, |sidebar, cx| {
                sidebar.run_command(SidebarCommand::CompareWithCurrent(reference), window, cx);
            }),
            PaletteAction::Fixup { hash } => {
                let Some(changes) = target
                    .and_then(|target| self.workspaces.get(&target.repo_id))
                    .map(|workspace| workspace.read(cx).changes().clone())
                else {
                    return;
                };
                changes.update(cx, |changes, cx| {
                    let chosen = changes
                        .fixup_targets()
                        .iter()
                        .find(|candidate| candidate.hash == hash)
                        .cloned();
                    if let Some(chosen) = chosen {
                        changes.commit_fixup(&chosen, cx);
                    }
                });
            }
        }
    }

    fn handle_app_event(
        &mut self,
        _model: &Entity<AppModel>,
        event: &AppEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppEvent::ThemeChanged { dark } => {
                let preference = if *dark {
                    ThemePreference::Dark
                } else {
                    ThemePreference::Light
                };
                mergeon_ui::theme::apply(preference, cx);
            }
            AppEvent::ShortcutsChanged => {
                let overrides = self.model.read(cx).keyboard_shortcuts().clone();
                crate::rebind_shortcuts(&overrides, cx);
            }
            AppEvent::ShowDialog(request) => {
                views::open_dialog(request, window, cx);
            }
            AppEvent::Window(WindowRequest::Minimize) => window.minimize_window(),
            AppEvent::Window(WindowRequest::Quit) => {
                let shutdown = self.model.update(cx, |model, cx| model.shutdown(cx));
                cx.spawn(async move |_, cx| {
                    shutdown.await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
            AppEvent::Window(WindowRequest::Reloaded) => {
                self.workspaces.clear();
            }
            AppEvent::Window(WindowRequest::ToggleDevTools) => {}
            AppEvent::MutationRecorded { repo_id, .. } => {
                // Los archivos pudieron cambiar: la próxima paleta los relee.
                if let Some(path) = self
                    .model
                    .read(cx)
                    .repository(repo_id.as_str())
                    .map(|repo| repo.path.clone())
                {
                    self.file_index
                        .update(cx, |index, _| index.invalidate(&path));
                }
                return;
            }
            _ => {}
        }
        cx.notify();
    }

    fn handle_sidebar_event(
        &mut self,
        _sidebar: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::CommitSelectionRequested { repo_id, request } => {
                if let Some(workspace) = self.workspaces.get(repo_id) {
                    let commits = workspace.read(cx).commits().clone();
                    let hash = request.hash.clone();
                    commits.update(cx, |view, cx| view.request_commit_selection(&hash, cx));
                }
            }
            SidebarEvent::OpenStashDiff { repo_path, stash } => {
                mergeon_view_changes::open_stash_diff_dialog(repo_path, &stash.hash, window, cx);
            }
            SidebarEvent::OpenComparison(request) => {
                let view = mergeon_view_commits::open_compare_view(request, window, cx);
                let repo_id = request.repo_id.clone();
                // Doble clic en un commit: se cierra la comparación y el
                // historial lo selecciona (como el hash de "File history" o
                // del blame: activa el repositorio y la pestaña Commits).
                cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
                    let mergeon_view_commits::CompareViewEvent::ShowCommit { full_hash } = event;
                    mergeon_ui::dialog::close(window, cx);
                    this.model.update(cx, |model, cx| {
                        model.show_commit_in_history(&repo_id, full_hash, cx);
                    });
                })
                .detach();
            }
        }
    }

    fn workspace_for(
        &mut self,
        repo_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<RepositoryWorkspaceView> {
        if let Some(workspace) = self.workspaces.get(repo_id) {
            return workspace.clone();
        }
        let id = repo_id.to_string();
        let sidebar = self.sidebar.clone();
        let workspace = cx.new(|cx| RepositoryWorkspaceView::new(id, sidebar, window, cx));
        self.workspaces
            .insert(repo_id.to_string(), workspace.clone());
        workspace
    }

    /// Botón de solo ícono de la barra de título (abrir repositorio, log,
    /// Settings), con su punto de aviso. No es zona de arrastre.
    fn title_bar_button(
        &self,
        button: TitleBarButton,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let TitleBarButton {
            id,
            icon,
            tooltip,
            active,
            dot,
        } = button;
        div()
            .relative()
            .flex_none()
            .occlude()
            .child(
                mergeon_ui::button(id)
                    .ghost()
                    .xs()
                    .icon(icon)
                    .tooltip(tooltip)
                    .selected(active)
                    .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx))),
            )
            .when_some(dot, |button, color| {
                button.child(
                    div()
                        .absolute()
                        .right(px(3.))
                        .top(px(3.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(color),
                )
            })
            .into_any_element()
    }

    /// El borde derecho de la barra de título: log de operaciones y Settings.
    fn render_title_bar_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // Pinta desde `self.shown` sin leer `AppModel` en `render`: leer el
        // modelo aquí registraría la entidad en GPUI y repintaría toda la
        // ventana ante cualquier aviso ajeno.
        let settings_open = self.shown.show_preferences;
        let unseen_errors = self.shown.unseen_errors;
        let has_update = matches!(
            self.shown.update_status,
            UpdateStatus::Available | UpdateStatus::Downloading | UpdateStatus::Downloaded
        );
        let is_mac = cfg!(target_os = "macos");
        let shortcuts = &self.shown.keyboard_shortcuts;
        let ops_log_tooltip = tooltip_with_shortcut(
            "Operation log",
            ShortcutAction::ShowOperationsLog,
            shortcuts,
            is_mac,
        );
        let settings_tooltip = if has_update {
            "Settings · Update available".to_string()
        } else {
            tooltip_with_shortcut(
                "Settings",
                ShortcutAction::OpenPreferences,
                shortcuts,
                is_mac,
            )
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .px(px(6.))
            .child(self.title_bar_button(
                TitleBarButton {
                    id: "titlebar-operations-log",
                    icon: Lucide::RotateCcwClock,
                    tooltip: ops_log_tooltip.into(),
                    active: false,
                    dot: unseen_errors.then_some(palette.destructive.into()),
                },
                |this, cx| {
                    this.model.update(cx, |model, cx| {
                        model.handle_shortcut(ShortcutAction::ShowOperationsLog, cx)
                    });
                },
                cx,
            ))
            .child(self.title_bar_button(
                TitleBarButton {
                    id: "titlebar-settings",
                    icon: Lucide::Settings,
                    tooltip: settings_tooltip.into(),
                    active: settings_open,
                    dot: has_update.then_some(palette.primary.into()),
                },
                move |this, cx| {
                    let section = if has_update { "help" } else { "appearance" };
                    this.model
                        .update(cx, |model, cx| model.open_settings(section, cx));
                },
                cx,
            ))
    }

    fn project_tab(
        &self,
        tab: ProjectTab,
        on_select: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        on_close: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let ProjectTab {
            id,
            icon,
            icon_active_tone,
            name,
            suffix,
            title,
            active,
            mark,
            busy,
            closable,
            repository,
        } = tab;
        let palette = *cx.palette();
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let on_close: TabCloseHandler = Rc::new(on_close);
        let group = SharedString::from(format!("{id}-group"));
        let title = match (title, mark.describe()) {
            (Some(title), Some(mark)) => Some(SharedString::from(format!("{title} · {mark}"))),
            (title, _) => title,
        };
        // `AppIcon` del original: tono `active`/`muted` en repos, `default` en el resto.
        let icon_tone = match (icon_active_tone, active) {
            (true, true) => IconTone::Active,
            (true, false) => IconTone::Muted,
            (false, _) => IconTone::Default,
        };

        let tab = div()
            .id(SharedString::from(format!("{id}-select")))
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .max_w(px(TAB_MAX_WIDTH))
            .h(px(26.))
            .pl(px(10.))
            .pr(px(8.))
            .rounded(Radius::Md.px(cx))
            .text_size(text::BODY)
            .cursor_pointer()
            .text_color(if active { foreground } else { muted })
            .when(!active && !closable, |tab| {
                tab.hover(move |tab| tab.bg(palette.accent).text_color(foreground))
            })
            .when(!active && closable, |tab| {
                tab.hover(move |tab| tab.text_color(foreground))
            })
            .when_some(title, |tab, title| tab.tooltip(mergeon_ui::tooltip(title)))
            .child(if busy {
                // Hay una operación en curso o en cola en este repositorio.
                mergeon_ui::spinner()
                    .size(SpinnerSize::Sm)
                    .color(SpinnerColor::Primary)
                    .into_any_element()
            } else {
                mergeon_ui::icon(icon)
                    .size(px(14.))
                    .tone(icon_tone)
                    .into_any_element()
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_shrink(1.)
                            .min_w(px(32.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(name),
                    )
                    .when_some(suffix, |label, suffix| {
                        // La carpeta que distingue repositorios homónimos.
                        label.child(
                            div()
                                .flex_shrink(1.)
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(text::CAPTION)
                                .text_color(muted)
                                .child(suffix),
                        )
                    }),
            )
            .when(mark != TabMark::None, |tab| {
                let color: gpui_kit::Hsla = match mark {
                    TabMark::Conflict => palette.destructive.into(),
                    TabMark::Operation => palette.warning.into(),
                    TabMark::Dirty | TabMark::None => palette.text_2.into(),
                };
                tab.child(div().flex_none().size(px(6.)).rounded_full().bg(color))
            })
            .on_click(cx.listener(move |this, _, _, cx| on_select(this, cx)));

        // La pestaña tapa la zona de arrastre de la barra de título. Sin esto
        // el clic arrancaba "mover ventana" al menor movimiento del ratón, y
        // en Windows el sistema se quedaba con él (HTCAPTION). Deja pasar la
        // rueda: con `occlude` la tira no se podía desplazar sobre una pestaña.
        let container = div()
            .id(id.clone())
            .group(group.clone())
            .block_mouse_except_scroll()
            .flex()
            .flex_none()
            .items_center()
            .rounded(Radius::Md.px(cx))
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .when(active, |tab| {
                tab.bg(palette.card)
                    .border_color(border)
                    .shadow(vec![gpui_kit::BoxShadow {
                        color: palette.shadow_raised.into(),
                        offset: gpui_kit::point(px(0.), px(1.)),
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
            })
            .when(!active && closable, |tab| {
                tab.hover(move |tab| tab.bg(palette.accent))
            })
            .child(tab);

        if !closable {
            return container.test_support().into_any_element();
        }
        let container = container
            .pr(px(4.))
            // Clic central: cerrar, como en navegadores y editores.
            .on_mouse_up(MouseButton::Middle, {
                let on_close = on_close.clone();
                cx.listener(move |this, _, _, cx| on_close(this, cx))
            })
            .child(
                div()
                    .id(SharedString::from(format!("{id}-close")))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded(Radius::Md.px(cx))
                    .cursor_pointer()
                    .text_color(muted)
                    // × solo en la activa y al pasar por encima.
                    .when(!active, |button| {
                        button
                            .opacity(0.)
                            .group_hover(group.clone(), |style| style.opacity(1.))
                    })
                    .hover(move |button| button.bg(palette.bg_pressed))
                    .child(
                        mergeon_ui::icon(Lucide::X)
                            .size(px(12.))
                            .tone(IconTone::Muted),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| on_close(this, cx)))
                    .test_support(),
            );

        let Some(repository) = repository else {
            return container.test_support().into_any_element();
        };
        let index = repository.index;
        let drag_label = repository.label.clone();
        let model = self.model.clone();
        container
            // Arrastrar para reordenar.
            .on_drag(
                DraggedRepositoryTab {
                    index,
                    label: drag_label,
                },
                |dragged, _, _, cx| {
                    let label = dragged.label.clone();
                    cx.new(|_| DragPreview { label })
                },
            )
            .drag_over::<DraggedRepositoryTab>(move |style, _, _, _| {
                style.shadow(insertion_marker(palette.primary))
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedRepositoryTab, _, cx| {
                    this.model.update(cx, |model, cx| {
                        model.move_repository_tab(dragged.index, index, cx);
                    });
                }),
            )
            .context_menu_items(move |_, cx| {
                let shortcuts = model.read(cx).keyboard_shortcuts().clone();
                repository_tab_menu(&model, &repository, &shortcuts)
            })
            .into_any_element()
    }

    /// Lleva la pestaña activa a la vista cuando cambia (o cambia el número de
    /// pestañas), sin pelear con el scroll que haga el usuario después.
    fn scroll_active_tab_into_view(
        &mut self,
        target: (usize, usize, bool, bool),
        window: &mut Window,
    ) {
        let child_index = target.0;
        if self.scrolled_tab == Some(target) {
            return;
        }
        self.tab_scroll.scroll_to_item(child_index);
        // GPUI calcula el desplazamiento con el tamaño de la tira del
        // fotograma anterior: en el primero aún no lo tiene, así que se
        // repite en el siguiente.
        if self.tab_scroll.bounds().size.width > px(0.) {
            self.scrolled_tab = Some(target);
        } else {
            window.request_animation_frame();
        }
    }

    fn render_tab_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let repositories: Vec<RepositoryTabInfo> = self.shown.tabs.clone();
        let active_index = self.shown.active_index;
        let settings_open = self.shown.settings_tab_open;
        let settings_active = self.shown.show_preferences;
        let restoring = self.shown.restoring;
        let home_active = !settings_active && active_index.is_none();
        let repository_count = repositories.len();

        // Hijos de la tira: Home (0), un repositorio por pestaña y Settings.
        if !restoring {
            let active_child = if settings_active {
                repository_count + 1
            } else {
                active_index.map_or(0, |index| index + 1)
            };
            // El punto de cambios o el spinner llegan después y ensanchan la
            // pestaña: también cuentan para volver a traerla a la vista.
            let active_repository = active_index
                .filter(|_| !settings_active)
                .and_then(|index| repositories.get(index));
            let target = (
                active_child,
                repository_count,
                active_repository.is_some_and(|info| info.mark != TabMark::None),
                active_repository.is_some_and(|info| info.busy),
            );
            self.scroll_active_tab_into_view(target, window);
        }

        let mut tabs = div()
            .id("project-tabs")
            .flex()
            // Tan ancha como sus pestañas y, si no caben, encoge y se desplaza
            // (rueda) en vez de cortarse: así "+" queda pegado a la última.
            .flex_shrink(1.)
            .min_w_0()
            .h_full()
            .items_center()
            .gap(px(6.))
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll)
            .child(self.project_tab(
                ProjectTab {
                    id: "tab-home".into(),
                    icon: Lucide::House,
                    icon_active_tone: false,
                    name: "Home".into(),
                    suffix: None,
                    title: None,
                    active: home_active,
                    mark: TabMark::None,
                    busy: false,
                    closable: false,
                    repository: None,
                },
                |this, cx| this.model.update(cx, |model, cx| model.show_home_tab(cx)),
                |_, _| {},
                cx,
            ));

        for (index, info) in repositories.iter().enumerate() {
            let active = !settings_active && active_index == Some(index);
            let id = info.id.clone();
            tabs = tabs.child(self.project_tab(
                ProjectTab {
                    id: SharedString::from(format!("tab-repo-{}", info.id)),
                    icon: Lucide::FolderGit2,
                    icon_active_tone: true,
                    name: info.label.name.clone().into(),
                    suffix: info.label.suffix.clone().map(Into::into),
                    title: Some(info.path.clone().into()),
                    active,
                    mark: info.mark,
                    busy: info.busy,
                    closable: true,
                    repository: Some(RepositoryTabMenu {
                        index,
                        id: info.id.clone(),
                        path: info.path.clone(),
                        label: info.label.text().into(),
                        is_last: index + 1 == repository_count,
                        is_only: repository_count == 1,
                    }),
                },
                move |this, cx| {
                    this.model
                        .update(cx, |model, cx| model.select_repository_tab(index, cx))
                },
                move |this, cx| {
                    let id = id.clone();
                    this.model.update(cx, |model, cx| {
                        model.request_close_repositories(vec![id], cx).detach();
                    })
                },
                cx,
            ));
        }

        if settings_open {
            tabs = tabs.child(self.project_tab(
                ProjectTab {
                    id: "tab-settings".into(),
                    icon: Lucide::Settings,
                    icon_active_tone: false,
                    name: "Settings".into(),
                    suffix: None,
                    title: None,
                    active: settings_active,
                    mark: TabMark::None,
                    busy: false,
                    closable: true,
                    repository: None,
                },
                |this, cx| {
                    let section = this.model.read(cx).settings_section().to_string();
                    this.model
                        .update(cx, |model, cx| model.open_settings(&section, cx))
                },
                |this, cx| {
                    this.model
                        .update(cx, |model, cx| model.close_settings_tab(cx))
                },
                cx,
            ));
        }

        // "▾": todas las pestañas de repositorio en una lista, con su carpeta
        // y lo que piden atención, para llegar a las que no caben.
        let tab_list = (repository_count >= 2).then(|| {
            let model = self.model.clone();
            let entries: Vec<(usize, String, String, TabMark, bool)> = repositories
                .iter()
                .enumerate()
                .map(|(index, info)| {
                    (
                        index,
                        info.label.text(),
                        parent_folder(&info.path),
                        info.mark,
                        info.busy,
                    )
                })
                .collect();
            let current = active_index.filter(|_| !settings_active);
            div().occlude().flex_none().child(
                mergeon_ui::button("repository-tab-list")
                    .ghost()
                    .xs()
                    .icon(Lucide::ChevronDown)
                    .tooltip("Open repositories")
                    .dropdown_menu_items(move |_, _| {
                        entries
                            .iter()
                            .map(|(index, label, folder, mark, busy)| {
                                let model = model.clone();
                                let index = *index;
                                let icon = if *busy {
                                    Lucide::LoaderCircle
                                } else {
                                    match mark {
                                        TabMark::Conflict => Lucide::TriangleAlert,
                                        TabMark::Operation => Lucide::CircleAlert,
                                        TabMark::Dirty => Lucide::CircleDot,
                                        TabMark::None => Lucide::FolderGit2,
                                    }
                                };
                                MenuItem::new(label.clone())
                                    .icon(icon)
                                    .detail(folder.clone())
                                    .checked(current == Some(index))
                                    .on_click(move |_, cx| {
                                        model.update(cx, |model, cx| {
                                            model.select_repository_tab(index, cx)
                                        });
                                    })
                            })
                            .collect()
                    }),
            )
        });

        let open_repository_tooltip = tooltip_with_shortcut(
            "Open repository",
            ShortcutAction::OpenRepository,
            &self.shown.keyboard_shortcuts,
            cfg!(target_os = "macos"),
        );
        let open_repository = self.title_bar_button(
            TitleBarButton {
                id: "titlebar-open-repository",
                icon: Lucide::Plus,
                tooltip: open_repository_tooltip.into(),
                active: false,
                dot: None,
            },
            |this, cx| {
                this.model
                    .update(cx, |model, cx| model.show_repository_options(cx));
            },
            cx,
        );

        TitleBar::new()
            .h(px(TAB_BAR_HEIGHT))
            // En macOS se queda el margen de `TitleBar` para los semáforos.
            .when(!cfg!(target_os = "macos"), |bar| bar.pl_2())
            .bg(cx.theme().background)
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_0()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap(px(4.))
                    .child(tabs)
                    .child(open_repository)
                    // Hueco libre: arrastra la ventana.
                    .child(div().flex_1().min_w(px(24.)).h_full())
                    .children(tab_list)
                    .child(self.render_title_bar_actions(cx)),
            )
    }

    fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let show_preferences = self.shown.show_preferences;
        let settings_open = self.shown.settings_tab_open;
        let active = self
            .shown
            .active_index
            .and_then(|index| self.shown.tabs.get(index))
            .map(|info| info.id.clone());
        let restoring = self.shown.restoring;

        if settings_open && show_preferences {
            return self.settings.clone().into_any_element();
        }

        let Some(repo_id) = active else {
            return self.home.clone().into_any_element();
        };
        // Al restaurar la sesión cada repositorio pasa un momento por activo:
        // montar su espacio de trabajo lo hidrataba a medias (y le ponía o no
        // el punto de cambios según los fotogramas que se pintaran).
        if restoring && !self.workspaces.contains_key(&repo_id) {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .child(mergeon_ui::spinner().size(SpinnerSize::Lg))
                .into_any_element();
        }
        let workspace = self.workspace_for(&repo_id, window, cx);
        // La barra de estado muestra dónde está la selección del historial.
        // Se recalcula en cada frame (la cáscara se repinta con cualquier
        // cambio de sus vistas hijas) y solo la notifica si cambia.
        let history = history_position(&workspace, cx);
        self.status_bar
            .update(cx, |status_bar, cx| status_bar.set_history(history, cx));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(self.toolbar.clone())
            .child(self.operation_status.clone())
            .child(workspace)
            .child(self.status_bar.clone())
            .into_any_element()
    }
}

fn tooltip_with_shortcut(
    label: &str,
    action: ShortcutAction,
    shortcuts: &BTreeMap<String, String>,
    is_mac: bool,
) -> String {
    match effective_combo_label(action, shortcuts, is_mac) {
        Some(keys) => format!("{label} ({keys})"),
        None => label.to_string(),
    }
}

/// La carpeta que contiene un repositorio, abreviando el directorio personal
/// (`/home/ana/src/demo` → `~/src`), para la lista de "▾".
fn parent_folder(path: &str) -> String {
    let parent = std::path::Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    match parent.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with(['/', '\\'])) => {
            format!("~{rest}")
        }
        _ => parent,
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        static FIRST_FRAME: std::sync::Once = std::sync::Once::new();
        FIRST_FRAME.call_once(|| mergeon_app::startup::trace("first frame"));
        // Los espacios de trabajo de repos cerrados no deben seguir vivos.
        let tabs = &self.shown.tabs;
        self.workspaces
            .retain(|id, _| tabs.iter().any(|info| info.id == *id));

        let content = self.render_content(window, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_tab_bar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(content),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, MouseMoveEvent, PlatformInput, TestAppContext, WindowControlArea,
        WindowDecorations, WindowOptions, point,
    };

    /// La ventana real con la barra de título de la app y Settings abierto.
    fn shell_with_settings_tab(cx: &mut TestAppContext) -> AnyWindowHandle {
        cx.executor().allow_parking();
        let (backend, events) = mergeon_app::testing::TestBackend::new().build();
        let model = cx.update(|cx| {
            crate::init_bindings(cx);
            AppModel::init(backend, events, cx)
        });
        model.update(cx, |model, cx| model.open_settings("appearance", cx));
        let window = cx.update(|cx| {
            let options = WindowOptions {
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| AppShell::new(window, cx))
            })
            .expect("ventana de prueba")
            .0
        });
        cx.run_until_parked();
        window
    }

    /// Lo que la ventana le contesta a la plataforma (WM_NCHITTEST en Windows)
    /// con el ratón sobre el centro de `id`.
    fn control_area_over(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        id: &'static str,
    ) -> Option<WindowControlArea> {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.hover(id, cx);
        })
        .expect("ventana");
        cx.simulate_hit_test_window_control(window)
    }

    // Antes: las pestañas de la barra de título usaban `block_mouse_except_scroll`
    // y la consulta miraba todos los hitboxes bajo el ratón, así que encima de
    // una pestaña contestaba `Drag`. En Windows eso es HTCAPTION: el sistema se
    // quedaba con el clic y ni la pestaña Settings ni su × respondían.
    #[gpui_kit::test]
    async fn title_bar_tabs_and_their_close_buttons_are_not_drag_areas(cx: &mut TestAppContext) {
        let window = shell_with_settings_tab(cx);

        for id in [
            "tab-home",
            "tab-settings",
            "tab-settings-close",
            "titlebar-open-repository",
            "titlebar-operations-log",
            "titlebar-settings",
        ] {
            assert_eq!(control_area_over(cx, window, id), None, "{id}");
        }

        // El hueco libre de la barra, a la derecha de las pestañas, sí mueve la
        // ventana: la consulta funciona y la prueba de arriba no pasa por vacía.
        let empty = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                // Pasado "+" (que va pegado a la última pestaña).
                let plus = window.find("titlebar-open-repository").bounds();
                let empty = point(plus.right() + px(80.), plus.center().y);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: empty,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }),
                    cx,
                );
                empty
            })
            .expect("ventana");
        assert_eq!(
            cx.simulate_hit_test_window_control(window),
            Some(WindowControlArea::Drag),
            "{empty:?}"
        );
    }
}
