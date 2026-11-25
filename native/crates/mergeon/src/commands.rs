//! Registro único de comandos de la paleta: id, etiqueta, contexto (grupo),
//! ícono, cuándo está disponible y qué ejecuta.
//!
//! No duplica lógica: casi todo es un [`ShortcutAction`] (lo mismo que hace
//! su atajo, `AppModel::handle_shortcut`) y el resto llama al mismo método
//! del modelo que el botón de la barra o de los ajustes (stash, terminal,
//! carpeta, tema, inicio). El atajo que se muestra sale de
//! `mergeon_model::keyboard_shortcuts`, así que no puede desfasarse.
//!
//! Disponibilidad: lo que necesita un repositorio no está disponible sin
//! uno; lo que lo cambia (fetch, pull, push, stash, nueva rama) tampoco
//! mientras hay una operación de secuencia a medias (como la barra). En la
//! paleta se ven atenuados y al final.

use gpui_kit::{App, Entity};
use mergeon_app::{AppModel, PullRequestsAvailability};
use mergeon_model::command_palette::PaletteScope;
use mergeon_model::keyboard_shortcuts::{
    APP_SHORTCUTS, ShortcutAction, WORKSPACE_SHORTCUTS, combo_label, is_mac_platform,
};
use mergeon_model::repositories::RepoLifecycleState;
use mergeon_ui::Lucide;

/// Lo que hace un comando.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Lo mismo que su atajo.
    Shortcut(ShortcutAction),
    /// "Stash" de la barra.
    Stash,
    /// "Open Terminal" de la barra.
    OpenTerminal,
    /// "Show in Folder" de la barra.
    ShowInFolder,
    /// Claro ↔ oscuro (Settings → Appearance).
    ToggleTheme,
    /// La pestaña Home.
    GoHome,
    /// No ejecuta nada: deja la paleta buscando en un ámbito ("Switch to
    /// branch…" escribe `@`).
    Scope(PaletteScope),
    /// "Stash selected files…" de Local Changes sin menú: el diálogo de stash
    /// con los archivos marcados (lo preparado).
    StashSelected,
    /// Vuelve a abrir la paleta como selector de lo que pide el comando (lo
    /// resuelve la cáscara, ver [`Picker`]).
    Pick(Picker),
    /// Lo mismo que su entrada de menú en el sidebar (lo resuelve la
    /// cáscara con `Sidebar::run_command`).
    Sidebar(SidebarTask),
    /// El PR de la rama actual en el navegador (como el indicador de la
    /// barra de estado).
    OpenCurrentPullRequest,
    /// Crea un nuevo workspace (grupo de repositorios).
    CreateWorkspace,
    /// Renombra el workspace activo.
    RenameWorkspace,
    /// Borra el workspace activo (sin cerrar repositorios).
    DeleteWorkspace,
}

/// Los comandos de la paleta que son una entrada de menú del sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidebarTask {
    /// "Clean up merged branches…" de la cabecera Branches.
    CleanupMergedBranches,
    /// "New worktree…" de la cabecera Worktrees.
    NewWorktree,
}

impl SidebarTask {
    pub fn command(self) -> mergeon_view_sidebar::SidebarCommand {
        use mergeon_view_sidebar::SidebarCommand;
        match self {
            SidebarTask::CleanupMergedBranches => SidebarCommand::CleanupMergedBranches,
            SidebarTask::NewWorktree => SidebarCommand::NewWorktree,
        }
    }
}

/// Los comandos que necesitan elegir algo antes de ejecutarse: la paleta se
/// reabre con esa lista.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Picker {
    /// "File history…": un archivo del repositorio → su pestaña de historial
    /// desde `HEAD`.
    FileHistory,
    /// "Compare with current branch…": una rama → la comparación navegable
    /// con la actual (`SidebarCommand::CompareWithCurrent`).
    CompareBranch,
    /// "Fixup into…": uno de los commits de la caja de commit → un
    /// `fixup!` con lo preparado (`ChangesView::commit_fixup`).
    FixupTarget,
    /// "Checkout pull request…": uno de los PRs abiertos del repositorio →
    /// trae su rama y la desprotege (como "Checkout PR branch" del sidebar).
    PullRequest,
    /// "Switch workspace…": elige "All" o uno de los grupos de repositorios.
    Workspace,
}

/// Una entrada del registro.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// Estable (tests y documentación).
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: &'static str,
    pub label: &'static str,
    /// El grupo, que se pinta atenuado como contexto.
    pub group: &'static str,
    pub icon: Lucide,
    pub command: Command,
}

const fn spec(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    icon: Lucide,
    command: Command,
) -> CommandSpec {
    CommandSpec {
        id,
        label,
        group,
        icon,
        command,
    }
}

const fn shortcut(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    icon: Lucide,
    action: ShortcutAction,
) -> CommandSpec {
    spec(id, label, group, icon, Command::Shortcut(action))
}

const REPOSITORY: &str = "Repository";
const NAVIGATION: &str = "Navigation";
const APPLICATION: &str = "Application";
const GO_TO: &str = "Go to";

/// Todos los comandos, en el orden en que se listan con la consulta vacía.
pub const COMMANDS: &[CommandSpec] = &[
    spec(
        "switch-branch",
        "Switch to branch…",
        GO_TO,
        Lucide::GitBranch,
        Command::Scope(PaletteScope::Branches),
    ),
    spec(
        "go-to-repository",
        "Go to repository…",
        GO_TO,
        Lucide::FolderGit2,
        Command::Scope(PaletteScope::Repositories),
    ),
    spec(
        "go-to-file",
        "Go to file…",
        GO_TO,
        Lucide::FileSearch,
        Command::Scope(PaletteScope::Files),
    ),
    spec(
        "file-history",
        "File history…",
        GO_TO,
        Lucide::FileClock,
        Command::Pick(Picker::FileHistory),
    ),
    spec(
        "compare-branch",
        "Compare with current branch…",
        GO_TO,
        Lucide::GitCompareArrows,
        Command::Pick(Picker::CompareBranch),
    ),
    shortcut(
        "fetch",
        "Fetch…",
        REPOSITORY,
        Lucide::ArrowDownToLine,
        ShortcutAction::Fetch,
    ),
    shortcut(
        "pull",
        "Pull…",
        REPOSITORY,
        Lucide::Download,
        ShortcutAction::Pull,
    ),
    shortcut(
        "push",
        "Push…",
        REPOSITORY,
        Lucide::Upload,
        ShortcutAction::Push,
    ),
    spec(
        "stash",
        "Stash changes…",
        REPOSITORY,
        Lucide::Archive,
        Command::Stash,
    ),
    spec(
        "stash-selected",
        "Stash selected files…",
        REPOSITORY,
        Lucide::Archive,
        Command::StashSelected,
    ),
    spec(
        "fixup",
        "Fixup into…",
        REPOSITORY,
        Lucide::Wrench,
        Command::Pick(Picker::FixupTarget),
    ),
    shortcut(
        "undo",
        "Undo last Git operation",
        REPOSITORY,
        Lucide::Undo2,
        ShortcutAction::Undo,
    ),
    shortcut(
        "redo",
        "Redo Git operation",
        REPOSITORY,
        Lucide::Redo2,
        ShortcutAction::Redo,
    ),
    shortcut(
        "new-branch",
        "New branch…",
        REPOSITORY,
        Lucide::GitBranchPlus,
        ShortcutAction::NewBranch,
    ),
    spec(
        "cleanup-branches",
        "Clean up merged branches…",
        REPOSITORY,
        Lucide::BrushCleaning,
        Command::Sidebar(SidebarTask::CleanupMergedBranches),
    ),
    spec(
        "new-worktree",
        "New worktree…",
        REPOSITORY,
        Lucide::FolderGit2,
        Command::Sidebar(SidebarTask::NewWorktree),
    ),
    spec(
        "open-pull-request",
        "Open pull request of current branch",
        REPOSITORY,
        Lucide::GitPullRequest,
        Command::OpenCurrentPullRequest,
    ),
    spec(
        "checkout-pull-request",
        "Checkout pull request…",
        REPOSITORY,
        Lucide::GitPullRequestArrow,
        Command::Pick(Picker::PullRequest),
    ),
    shortcut(
        "commits",
        "Show Commits",
        NAVIGATION,
        Lucide::GitCommitHorizontal,
        ShortcutAction::ShowCommitsView,
    ),
    shortcut(
        "local-changes",
        "Show Local Changes",
        NAVIGATION,
        Lucide::FilePen,
        ShortcutAction::ShowChangesView,
    ),
    shortcut(
        "search-commits",
        "Search commits",
        NAVIGATION,
        Lucide::Search,
        ShortcutAction::SearchCommits,
    ),
    shortcut(
        "reflog",
        "Reflog",
        REPOSITORY,
        Lucide::RotateCcwClock,
        ShortcutAction::ShowReflog,
    ),
    shortcut(
        "operation-log",
        "Operation log",
        APPLICATION,
        Lucide::ScrollText,
        ShortcutAction::ShowOperationsLog,
    ),
    shortcut(
        "open-in-editor",
        "Open in editor",
        REPOSITORY,
        Lucide::CodeXml,
        ShortcutAction::OpenInEditor,
    ),
    spec(
        "open-terminal",
        "Open terminal",
        REPOSITORY,
        Lucide::SquareTerminal,
        Command::OpenTerminal,
    ),
    spec(
        "show-in-folder",
        "Show in folder",
        REPOSITORY,
        Lucide::FolderSearch2,
        Command::ShowInFolder,
    ),
    shortcut(
        "open-repository",
        "Open repository…",
        APPLICATION,
        Lucide::FolderOpen,
        ShortcutAction::OpenRepository,
    ),
    shortcut(
        "clone",
        "Clone from GitHub…",
        APPLICATION,
        Lucide::Github,
        ShortcutAction::CloneFromGitHub,
    ),
    shortcut(
        "clone-url",
        "Clone from URL…",
        APPLICATION,
        Lucide::Link,
        ShortcutAction::CloneFromUrl,
    ),
    shortcut(
        "close-tab",
        "Close tab",
        NAVIGATION,
        Lucide::X,
        ShortcutAction::CloseTab,
    ),
    shortcut(
        "close-repository",
        "Close repository",
        NAVIGATION,
        Lucide::FolderX,
        ShortcutAction::CloseRepository,
    ),
    shortcut(
        "reopen-repository",
        "Reopen closed repository",
        NAVIGATION,
        Lucide::RotateCcw,
        ShortcutAction::ReopenClosedRepository,
    ),
    shortcut(
        "next-repository",
        "Next repository tab",
        NAVIGATION,
        Lucide::ChevronRight,
        ShortcutAction::NextRepositoryTab,
    ),
    shortcut(
        "previous-repository",
        "Previous repository tab",
        NAVIGATION,
        Lucide::ChevronLeft,
        ShortcutAction::PreviousRepositoryTab,
    ),
    shortcut(
        "next-document",
        "Next document",
        NAVIGATION,
        Lucide::ChevronsRight,
        ShortcutAction::NextDocumentTab,
    ),
    shortcut(
        "previous-document",
        "Previous document",
        NAVIGATION,
        Lucide::ChevronsLeft,
        ShortcutAction::PreviousDocumentTab,
    ),
    spec(
        "home",
        "Go to Home",
        NAVIGATION,
        Lucide::House,
        Command::GoHome,
    ),
    spec(
        "switch-workspace",
        "Switch workspace…",
        NAVIGATION,
        Lucide::Folders,
        Command::Pick(Picker::Workspace),
    ),
    spec(
        "create-workspace",
        "Create workspace…",
        APPLICATION,
        Lucide::SquarePlus,
        Command::CreateWorkspace,
    ),
    spec(
        "rename-workspace",
        "Rename current workspace…",
        APPLICATION,
        Lucide::Pencil,
        Command::RenameWorkspace,
    ),
    spec(
        "delete-workspace",
        "Delete current workspace",
        APPLICATION,
        Lucide::Trash,
        Command::DeleteWorkspace,
    ),
    shortcut(
        "settings",
        "Settings",
        APPLICATION,
        Lucide::Settings,
        ShortcutAction::OpenPreferences,
    ),
    shortcut(
        "shortcuts",
        "Keyboard shortcuts",
        APPLICATION,
        Lucide::Keyboard,
        ShortcutAction::OpenShortcutsHelp,
    ),
    spec(
        "toggle-theme",
        "Toggle light / dark theme",
        APPLICATION,
        Lucide::SunMoon,
        Command::ToggleTheme,
    ),
    shortcut(
        "reload",
        "Reload application",
        APPLICATION,
        Lucide::RotateCw,
        ShortcutAction::ReloadApplication,
    ),
    shortcut(
        "minimize",
        "Minimize window",
        APPLICATION,
        Lucide::Minimize2,
        ShortcutAction::MinimizeWindow,
    ),
    shortcut(
        "quit",
        "Quit Mergeon",
        APPLICATION,
        Lucide::LogOut,
        ShortcutAction::Quit,
    ),
];

/// Lo que decide qué comandos están disponibles, leído del modelo al abrir
/// la paleta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommandContext {
    pub has_repository: bool,
    /// Sin operación de secuencia a medias (merge, rebase…).
    pub repository_idle: bool,
    pub repository_count: usize,
    pub can_reopen: bool,
    /// Hay algo preparado (las casillas marcadas de Local Changes).
    pub has_staged: bool,
    /// Se conoce el PR abierto de la rama actual.
    pub has_branch_pull_request: bool,
    /// Se conocen PRs abiertos del repositorio.
    pub has_pull_requests: bool,
    /// Hay algo que deshacer / rehacer en el repositorio activo
    /// (`AppModel::can_undo`).
    pub can_undo: bool,
    pub can_redo: bool,
    /// Número de grupos de repositorios definidos.
    pub workspace_count: usize,
    /// Hay un workspace personalizado activo (no "All").
    pub has_active_workspace: bool,
}

impl CommandContext {
    pub fn read(model: &AppModel) -> Self {
        let active = model.active_repository();
        Self {
            has_repository: active.is_some(),
            repository_idle: active
                .is_some_and(|repo| repo.repo_lifecycle_state == RepoLifecycleState::Idle),
            repository_count: model.visible_repositories().len(),
            can_reopen: !model.closed_repositories().is_empty(),
            has_staged: active.is_some_and(|repo| !staged_paths(repo).is_empty()),
            has_branch_pull_request: model.active_branch_pull_request().is_some(),
            can_undo: model.can_undo(false),
            can_redo: model.can_undo(true),
            has_pull_requests: model.active_pull_requests().is_some_and(|state| {
                state.availability == PullRequestsAvailability::Available && !state.pulls.is_empty()
            }),
            workspace_count: model.workspaces().len(),
            has_active_workspace: model.active_workspace_id().is_some(),
        }
    }
}

/// Lo marcado en Local Changes (con algo preparado), como lo toma "Stash
/// selected files…" de su menú (`mergeon_view_changes::stash_selection`).
pub fn staged_paths(repo: &mergeon_app::OpenRepository) -> Vec<String> {
    let Some(status) = repo.status.as_ref() else {
        return Vec::new();
    };
    let mut paths: Vec<String> = Vec::new();
    for file in &status.files {
        let staged =
            !matches!(file.index.as_str(), "" | " " | "?" | "!" | "U") && file.working_dir != "U";
        if staged && !paths.contains(&file.path) {
            paths.push(file.path.clone());
        }
    }
    paths
}

impl CommandSpec {
    /// Si se puede ejecutar ahora.
    pub fn available(&self, context: CommandContext) -> bool {
        use ShortcutAction as A;
        let needs_repository = || context.has_repository;
        let mutates = || context.has_repository && context.repository_idle;
        match self.command {
            Command::Shortcut(A::Fetch | A::Pull | A::Push | A::NewBranch)
            | Command::Stash
            | Command::Sidebar(SidebarTask::CleanupMergedBranches | SidebarTask::NewWorktree) => {
                mutates()
            }
            Command::OpenCurrentPullRequest => {
                needs_repository() && context.has_branch_pull_request
            }
            Command::Shortcut(A::Undo) => needs_repository() && context.can_undo,
            Command::Shortcut(A::Redo) => needs_repository() && context.can_redo,
            Command::Pick(Picker::PullRequest) => mutates() && context.has_pull_requests,
            Command::Pick(Picker::Workspace) => context.workspace_count > 0,
            Command::CreateWorkspace => true,
            Command::RenameWorkspace | Command::DeleteWorkspace => context.has_active_workspace,
            // Sin nada marcado no hay qué guardar ni con qué hacer el fixup.
            Command::StashSelected | Command::Pick(Picker::FixupTarget) => {
                mutates() && context.has_staged
            }
            Command::Pick(Picker::FileHistory | Picker::CompareBranch) => needs_repository(),
            Command::Shortcut(
                A::ShowCommitsView
                | A::ShowChangesView
                | A::SearchCommits
                | A::ShowReflog
                | A::OpenInEditor
                | A::CloseTab
                | A::CloseRepository
                | A::NextDocumentTab
                | A::PreviousDocumentTab,
            )
            | Command::OpenTerminal
            | Command::ShowInFolder
            | Command::Scope(PaletteScope::Files) => needs_repository(),
            Command::Shortcut(A::NextRepositoryTab | A::PreviousRepositoryTab) => {
                context.repository_count > 0
            }
            Command::Shortcut(A::ReopenClosedRepository) => context.can_reopen,
            Command::Scope(PaletteScope::Branches) => needs_repository(),
            _ => true,
        }
    }

    /// El atajo tal como se pinta (`Ctrl+Shift+F`, `⌘⇧F`).
    pub fn shortcut_label(&self) -> Option<String> {
        let Command::Shortcut(action) = self.command else {
            return None;
        };
        APP_SHORTCUTS
            .iter()
            .chain(WORKSPACE_SHORTCUTS)
            .find(|binding| binding.action == action)
            .map(|binding| combo_label(binding.combo, is_mac_platform()))
    }
}

/// Ejecuta un comando sobre el repositorio activo (quien llama ya dejó
/// activo el que tocaba). Los `Scope` los resuelve la paleta y los `Pick` y
/// `Sidebar`, la cáscara.
pub fn run(command: Command, model: &Entity<AppModel>, cx: &mut App) {
    model.update(cx, |model, cx| match command {
        Command::Shortcut(action) => model.handle_shortcut(action, cx),
        Command::Stash => model.handle_stash(cx).detach(),
        Command::StashSelected => {
            let paths = model
                .active_repository()
                .map(staged_paths)
                .unwrap_or_default();
            if paths.is_empty() {
                model.notify_info("Check the files to stash in Local Changes first.", cx);
            } else {
                model.handle_stash_paths(paths, cx);
            }
        }
        Command::OpenTerminal => model.handle_open_terminal(cx).detach(),
        Command::ShowInFolder => model.handle_open_explorer(cx).detach(),
        Command::ToggleTheme => model.toggle_theme(cx).detach(),
        Command::GoHome => model.show_home_tab(cx),
        Command::OpenCurrentPullRequest => {
            match model
                .active_branch_pull_request()
                .map(|item| item.pull.html_url.clone())
            {
                Some(url) => model.open_external(&url, cx).detach(),
                None => {
                    model.notify_info("The current branch has no open pull request.", cx);
                }
            }
        }
        Command::CreateWorkspace => {
            let initial = model
                .active_repository()
                .map(|repo| vec![repo.path.clone()])
                .unwrap_or_default();
            model.prompt_create_workspace(initial, cx).detach();
        }
        Command::RenameWorkspace => {
            if let Some(id) = model.active_workspace_id().map(str::to_string) {
                model.prompt_rename_workspace(&id, cx).detach();
            }
        }
        Command::DeleteWorkspace => {
            if let Some(id) = model.active_workspace_id().map(str::to_string) {
                model.confirm_delete_workspace(&id, cx).detach();
            }
        }
        Command::Scope(_) | Command::Pick(_) | Command::Sidebar(_) => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_labels_are_unique() {
        let ids: HashSet<&str> = COMMANDS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids.len(), COMMANDS.len());
        let labels: HashSet<&str> = COMMANDS.iter().map(|spec| spec.label).collect();
        assert_eq!(labels.len(), COMMANDS.len());
    }

    /// Todo atajo global con sentido en la paleta tiene su comando.
    #[test]
    fn every_application_shortcut_has_a_command() {
        let covered: HashSet<ShortcutAction> = COMMANDS
            .iter()
            .filter_map(|spec| match spec.command {
                Command::Shortcut(action) => Some(action),
                _ => None,
            })
            .collect();
        for binding in APP_SHORTCUTS.iter().chain(WORKSPACE_SHORTCUTS) {
            let skipped = matches!(
                binding.action,
                ShortcutAction::SelectRepositoryTab(_)
                    | ShortcutAction::OpenCommandPalette
                    | ShortcutAction::ToggleDevTools
            );
            assert!(
                skipped || covered.contains(&binding.action),
                "{:?}",
                binding.action
            );
        }
    }

    #[test]
    fn availability_follows_the_repository() {
        let find = |id: &str| COMMANDS.iter().find(|spec| spec.id == id).unwrap();
        let none = CommandContext::default();
        let busy = CommandContext {
            has_repository: true,
            repository_idle: false,
            repository_count: 1,
            can_reopen: false,
            has_staged: true,
            has_branch_pull_request: false,
            has_pull_requests: false,
            can_undo: false,
            can_redo: false,
            workspace_count: 0,
            has_active_workspace: false,
        };
        let idle = CommandContext {
            repository_idle: true,
            ..busy
        };
        assert!(!find("fetch").available(none));
        assert!(!find("fetch").available(busy));
        assert!(find("fetch").available(idle));
        assert!(find("open-terminal").available(busy));
        assert!(!find("switch-branch").available(none));
        assert!(find("open-repository").available(none));
        assert!(find("go-to-repository").available(none));
        assert!(!find("reopen-repository").available(idle));
        assert!(find("reopen-repository").available(CommandContext {
            can_reopen: true,
            ..none
        }));
        // Las acciones nuevas de los otros paneles.
        assert!(!find("file-history").available(none));
        assert!(find("file-history").available(busy));
        assert!(find("compare-branch").available(busy));
        assert!(!find("stash-selected").available(busy));
        assert!(find("stash-selected").available(idle));
        assert!(!find("fixup").available(CommandContext {
            has_staged: false,
            ..idle
        }));
        assert!(find("fixup").available(idle));
        // Las de la ronda 2: limpieza y worktree cambian el repositorio; los
        // PRs, solo si se conocen.
        assert!(!find("cleanup-branches").available(busy));
        assert!(find("cleanup-branches").available(idle));
        assert!(!find("new-worktree").available(none));
        assert!(find("new-worktree").available(idle));
        assert!(!find("open-pull-request").available(idle));
        assert!(find("open-pull-request").available(CommandContext {
            has_branch_pull_request: true,
            ..busy
        }));
        assert!(!find("checkout-pull-request").available(idle));
        assert!(find("checkout-pull-request").available(CommandContext {
            has_pull_requests: true,
            ..idle
        }));
        assert!(!find("checkout-pull-request").available(CommandContext {
            has_pull_requests: true,
            ..busy
        }));
        // Deshacer y rehacer, solo con algo en la pila.
        assert!(!find("undo").available(idle));
        assert!(find("undo").available(CommandContext {
            can_undo: true,
            ..idle
        }));
        assert!(!find("undo").available(CommandContext {
            can_undo: true,
            ..none
        }));
        assert!(!find("redo").available(CommandContext {
            can_undo: true,
            ..idle
        }));
        assert!(find("redo").available(CommandContext {
            can_redo: true,
            ..idle
        }));
        // Workspaces: crear siempre, cambiar si existe alguno, renombrar/borrar si hay uno activo.
        assert!(find("create-workspace").available(none));
        assert!(!find("switch-workspace").available(none));
        assert!(find("switch-workspace").available(CommandContext {
            workspace_count: 1,
            ..none
        }));
        assert!(!find("rename-workspace").available(CommandContext {
            workspace_count: 1,
            ..none
        }));
        assert!(!find("delete-workspace").available(CommandContext {
            workspace_count: 1,
            ..none
        }));
        assert!(find("rename-workspace").available(CommandContext {
            workspace_count: 1,
            has_active_workspace: true,
            ..none
        }));
        assert!(find("delete-workspace").available(CommandContext {
            workspace_count: 1,
            has_active_workspace: true,
            ..none
        }));
    }

    #[test]
    fn shows_the_registered_shortcut() {
        let find = |id: &str| COMMANDS.iter().find(|spec| spec.id == id).unwrap();
        let mac = is_mac_platform();
        assert_eq!(
            find("fetch").shortcut_label(),
            Some(combo_label("mod+shift+f", mac))
        );
        assert_eq!(
            find("pull").shortcut_label(),
            Some(combo_label("mod+alt+p", mac))
        );
        assert_eq!(find("stash").shortcut_label(), None);
        assert_eq!(
            find("undo").shortcut_label(),
            Some(combo_label("mod+z", mac))
        );
        assert_eq!(
            find("redo").shortcut_label(),
            Some(combo_label("mod+shift+z", mac))
        );
    }
}
