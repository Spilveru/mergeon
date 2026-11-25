//! Menús contextuales del sidebar (`branchContextMenuItems`,
//! `buildRemoteBranchMenuItems`, `tagContextMenuItems` y
//! `remoteContextMenuItems` de `Sidebar.vue`).
//!
//! Cada menú se describe como datos ([`MenuEntry`]) a partir del estado del
//! repositorio: así la misma política decide la etiqueta, el ícono y si la
//! entrada está habilitada, y los tests la comprueban sin abrir el menú. La
//! acción de cada entrada es un [`SidebarCommand`], que ejecuta
//! [`crate::Sidebar::run_command`].

use mergeon_app::OpenRepository;
use mergeon_model::repository_read_model::{BranchItem, RemoteItem};
use mergeon_ui::Lucide;

use crate::logic::{
    branch_tracking_reference, build_pull_request_url, parse_upstream_reference, tag_remote,
};

/// Pestaña con la que se abre el diálogo Branch Actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchAction {
    Merge,
    Rebase,
    Compare,
}

impl BranchAction {
    pub fn as_str(self) -> &'static str {
        match self {
            BranchAction::Merge => "merge",
            BranchAction::Rebase => "rebase",
            BranchAction::Compare => "compare",
        }
    }
}

/// Una orden del sidebar: lo que hace una entrada de menú, un botón de fila o
/// un doble clic. Todas entran por [`crate::Sidebar::run_command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarCommand {
    // Ramas locales
    CheckoutBranch(String),
    FastForwardBranch(String),
    PullBranch(String),
    PushBranch(String),
    /// Branch Actions con una rama local como origen.
    BranchAction {
        branch: String,
        action: BranchAction,
    },
    /// Comparación navegable de esta ref (rama local o `remoto/rama`) con la
    /// rama actual (o `HEAD` si está desanclado).
    CompareWithCurrent(String),
    NewBranchFromBranch(String),
    NewTagFromBranch(String),
    SetupTracking(String),
    RenameBranch(String),
    DeleteBranch(String),
    CopyBranchName(String),
    ToggleStarBranch(String),
    /// "Clean up merged branches…" (menú de la sección Branches).
    CleanupMergedBranches,

    // Ramas remotas
    CheckoutRemoteBranch {
        remote: String,
        branch: String,
    },
    PullRemoteBranch {
        remote: String,
        branch: String,
    },
    OpenPullRequest(String),
    RemoteBranchAction {
        remote: String,
        branch: String,
        action: BranchAction,
    },
    NewBranchFromRemoteBranch {
        remote: String,
        branch: String,
    },
    NewTagFromRemoteBranch {
        remote: String,
        branch: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },

    // Tags
    CreateTag,
    CheckoutTag(String),
    ViewTagDetails(String),
    CopyTagName(String),
    PushTag(String),
    DeleteRemoteTag(String),
    DeleteTag(String),

    // Remotos
    AddRemote,
    EditRemoteUrl(String),
    RenameRemote(String),
    RemoveRemote(String),

    // Stashes (por hash, que es su ref)
    SelectStash(String),
    BranchFromStash(String),
    ApplyStash(String),
    PopStash(String),
    DropStash(String),

    // Submódulos (por ruta)
    UpdateAllSubmodules,
    UpdateSubmodule(String),
    OpenSubmodule(String),

    // Worktrees (por ruta)
    /// "New worktree…" de la cabecera de la sección.
    NewWorktree,
    /// "Check out in new worktree…" del menú de una rama local.
    CheckoutInNewWorktree(String),
    /// Abrir el worktree como repositorio en una pestaña.
    OpenWorktree(String),
    RemoveWorktree(String),
    LockWorktree(String),
    UnlockWorktree(String),
    CopyWorktreePath(String),
    PruneWorktrees,
}

/// Una entrada de menú contextual, o un separador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Separator,
    Item {
        label: String,
        icon: Lucide,
        disabled: bool,
        danger: bool,
        /// `None` en las entradas puramente informativas (siempre deshabilitadas).
        command: Option<SidebarCommand>,
    },
}

impl MenuEntry {
    pub(crate) fn item(label: impl Into<String>, icon: Lucide, command: SidebarCommand) -> Self {
        MenuEntry::Item {
            label: label.into(),
            icon,
            disabled: false,
            danger: false,
            command: Some(command),
        }
    }

    pub(crate) fn info(label: impl Into<String>, icon: Lucide) -> Self {
        MenuEntry::Item {
            label: label.into(),
            icon,
            disabled: true,
            danger: false,
            command: None,
        }
    }

    pub(crate) fn disabled(mut self, value: bool) -> Self {
        if let MenuEntry::Item { disabled, .. } = &mut self {
            *disabled = value;
        }
        self
    }

    pub(crate) fn danger(mut self) -> Self {
        if let MenuEntry::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    /// Texto de la entrada; vacío en un separador.
    pub fn label(&self) -> &str {
        match self {
            MenuEntry::Separator => "",
            MenuEntry::Item { label, .. } => label,
        }
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self, MenuEntry::Item { disabled: true, .. })
    }

    pub fn is_danger(&self) -> bool {
        matches!(self, MenuEntry::Item { danger: true, .. })
    }

    pub fn command(&self) -> Option<&SidebarCommand> {
        match self {
            MenuEntry::Item { command, .. } => command.as_ref(),
            MenuEntry::Separator => None,
        }
    }
}

/// Menú de la cabecera de la sección Branches.
pub fn branches_section_menu(repo: &OpenRepository) -> Vec<MenuEntry> {
    let has_local = repo.branches.iter().any(|branch| !branch.is_remote);
    vec![
        MenuEntry::item(
            "Clean up merged branches…",
            Lucide::BrushCleaning,
            SidebarCommand::CleanupMergedBranches,
        )
        .disabled(!has_local || !can_mutate(repo)),
    ]
}

/// `!activeRepo.operationState?.inProgress`.
fn can_mutate(repo: &OpenRepository) -> bool {
    !repo
        .operation_state
        .as_ref()
        .is_some_and(|state| state.in_progress)
}

/// "Compare with '<rama actual>'...", o con `HEAD` si está desanclado.
fn compare_label(current_branch: &str) -> String {
    if current_branch.is_empty() {
        "Compare with HEAD...".to_string()
    } else {
        format!("Compare with '{current_branch}'...")
    }
}

/// "Merge into '<rama actual>'..." y "Rebase '<rama actual>' onto '<ref>'...":
/// Branch Actions integra `source` en la rama actual o la rebasa sobre él.
/// Con HEAD desanclado no hay rama en la que integrar: etiquetas genéricas.
fn integration_labels(current_branch: &str, source: &str) -> [(String, Lucide, BranchAction); 2] {
    let (merge, rebase) = if current_branch.is_empty() {
        (
            "Merge into current branch...".to_string(),
            format!("Rebase current branch onto '{source}'..."),
        )
    } else {
        (
            format!("Merge into '{current_branch}'..."),
            format!("Rebase '{current_branch}' onto '{source}'..."),
        )
    };
    [
        (merge, Lucide::GitMerge, BranchAction::Merge),
        (rebase, Lucide::Workflow, BranchAction::Rebase),
    ]
}

/// Menú de una rama local. `is_running(operation)` dice si esa operación del
/// repositorio está en curso (`isBranchOperationRunning`).
pub fn local_branch_menu(
    repo: &OpenRepository,
    branch: &BranchItem,
    is_running: &dyn Fn(&str) -> bool,
) -> Vec<MenuEntry> {
    if branch.is_remote {
        return Vec::new();
    }

    let name = branch.name.clone();
    let current_branch = repo.current_branch.clone().unwrap_or_default();
    let tracking_reference = branch_tracking_reference(branch);
    let tracking_parts = parse_upstream_reference(tracking_reference.as_deref(), &repo.remotes);
    let can_run_tracking_actions = branch.active && tracking_parts.is_some();
    let can_fast_forward = branch.active && tracking_reference.is_some() && branch.behind > 0;
    let can_mutate_branch = can_mutate(repo);

    let mut items = vec![
        MenuEntry::item(
            format!("Checkout '{name}'"),
            Lucide::Download,
            SidebarCommand::CheckoutBranch(name.clone()),
        )
        .disabled(branch.active || !can_mutate_branch || is_running("checkout")),
    ];

    if let Some(tracking_reference) = &tracking_reference {
        items.push(
            MenuEntry::item(
                format!("Fast-Forward to '{tracking_reference}'"),
                Lucide::ChevronsRight,
                SidebarCommand::FastForwardBranch(name.clone()),
            )
            .disabled(!can_fast_forward || !can_mutate_branch || is_running("fast-forward-branch")),
        );
        items.push(
            MenuEntry::item(
                format!("Pull '{tracking_reference}'"),
                Lucide::Download,
                SidebarCommand::PullBranch(name.clone()),
            )
            .disabled(!can_run_tracking_actions || !can_mutate_branch || is_running("pull")),
        );
        let remote_name = tracking_parts
            .as_ref()
            .map_or("origin", |parts| parts.remote_name.as_str());
        items.push(
            MenuEntry::item(
                format!("Push '{name}' to '{remote_name}'"),
                Lucide::Upload,
                SidebarCommand::PushBranch(name.clone()),
            )
            .disabled(!can_run_tracking_actions || !can_mutate_branch || is_running("push")),
        );
    }

    items.push(MenuEntry::Separator);
    // Solo lee: no depende de que se pueda mutar el repositorio.
    items.push(
        MenuEntry::item(
            compare_label(&current_branch),
            Lucide::GitCompareArrows,
            SidebarCommand::CompareWithCurrent(name.clone()),
        )
        .disabled(branch.active),
    );
    for (label, icon, action) in integration_labels(&current_branch, &name) {
        items.push(
            MenuEntry::item(
                label,
                icon,
                SidebarCommand::BranchAction {
                    branch: name.clone(),
                    action,
                },
            )
            .disabled(branch.active || current_branch.is_empty() || !can_mutate_branch),
        );
    }

    items.push(MenuEntry::Separator);
    items.push(
        MenuEntry::item(
            "New Branch...",
            Lucide::GitBranchPlus,
            SidebarCommand::NewBranchFromBranch(name.clone()),
        )
        .disabled(!can_mutate_branch || is_running("create-branch")),
    );
    // La rama activa ya está desprotegida aquí: git no la deja en dos sitios.
    items.push(
        MenuEntry::item(
            "Check out in new worktree...",
            Lucide::FolderGit2,
            SidebarCommand::CheckoutInNewWorktree(name.clone()),
        )
        .disabled(branch.active || is_running("worktree-add")),
    );
    items.push(
        MenuEntry::item(
            "New Tag...",
            Lucide::Tag,
            SidebarCommand::NewTagFromBranch(name.clone()),
        )
        .disabled(!branch.active || !can_mutate_branch || is_running("create-tag")),
    );

    items.push(MenuEntry::Separator);
    match &tracking_reference {
        Some(tracking_reference) if branch.upstream_set => {
            items.push(MenuEntry::info(
                format!("Tracking: {tracking_reference}"),
                Lucide::Link2,
            ));
        }
        _ => {
            let target = tracking_reference
                .clone()
                .unwrap_or_else(|| format!("origin/{name}"));
            items.push(
                MenuEntry::item(
                    format!("Set Upstream to {target}"),
                    Lucide::Link2,
                    SidebarCommand::SetupTracking(name.clone()),
                )
                .disabled(!can_mutate_branch || is_running("setup-tracking")),
            );
        }
    }
    items.push(
        MenuEntry::item(
            format!("Rename '{name}'..."),
            Lucide::Pencil,
            SidebarCommand::RenameBranch(name.clone()),
        )
        .disabled(!can_mutate_branch || is_running("rename-branch")),
    );
    items.push(
        MenuEntry::item(
            format!("Delete '{name}'..."),
            Lucide::Trash,
            SidebarCommand::DeleteBranch(name.clone()),
        )
        .danger()
        .disabled(branch.active || !can_mutate_branch || is_running("delete-branch")),
    );

    items.push(MenuEntry::Separator);
    items.push(MenuEntry::item(
        "Copy Branch Name",
        Lucide::Copy,
        SidebarCommand::CopyBranchName(name),
    ));

    items
}

/// Menú de una rama remota (`buildRemoteBranchMenuItems`).
pub fn remote_branch_menu(
    repo: &OpenRepository,
    remote: &RemoteItem,
    branch_name: &str,
    is_running: &dyn Fn(&str) -> bool,
) -> Vec<MenuEntry> {
    if branch_name.is_empty() {
        return Vec::new();
    }

    let remote_name = remote.name.clone();
    let branch = branch_name.to_string();
    let full_remote_branch = format!("{remote_name}/{branch_name}");
    let current_branch = repo.current_branch.clone().unwrap_or_default();
    let current = if current_branch.is_empty() {
        "HEAD".to_string()
    } else {
        current_branch.clone()
    };
    let local_branch_exists = repo.branches.iter().any(|local| local.name == branch_name);
    let can_mutate_branch = can_mutate(repo);
    let matches_current_branch = branch_name == current_branch;
    let pull_request_url = build_pull_request_url(remote, branch_name, &current_branch);

    let mut items = vec![
        // Con la rama ya en local no se crea otra: se cambia a la que tienes, y la
        // etiqueta lo dice para que no parezca que va a duplicarla.
        MenuEntry::item(
            if local_branch_exists {
                format!("Switch to '{branch_name}'")
            } else {
                format!("Checkout '{full_remote_branch}'")
            },
            Lucide::Download,
            SidebarCommand::CheckoutRemoteBranch {
                remote: remote_name.clone(),
                branch: branch.clone(),
            },
        )
        .disabled(
            (local_branch_exists && matches_current_branch)
                || !can_mutate_branch
                || is_running("checkout-remote-branch")
                || is_running("checkout"),
        ),
        MenuEntry::item(
            format!("Pull '{full_remote_branch}' into '{current}'..."),
            Lucide::Download,
            SidebarCommand::PullRemoteBranch {
                remote: remote_name.clone(),
                branch: branch.clone(),
            },
        )
        .disabled(!can_mutate_branch || is_running("pull")),
        MenuEntry::Separator,
    ];

    if let Some(url) = pull_request_url {
        items.push(MenuEntry::item(
            format!("Create Pull Request on {remote_name}..."),
            Lucide::GitPullRequestArrow,
            SidebarCommand::OpenPullRequest(url),
        ));
        items.push(MenuEntry::Separator);
    }

    items.push(MenuEntry::item(
        compare_label(&current),
        Lucide::GitCompareArrows,
        SidebarCommand::CompareWithCurrent(full_remote_branch.clone()),
    ));
    for (label, icon, action) in integration_labels(&current_branch, &full_remote_branch) {
        items.push(
            MenuEntry::item(
                label,
                icon,
                SidebarCommand::RemoteBranchAction {
                    remote: remote_name.clone(),
                    branch: branch.clone(),
                    action,
                },
            )
            .disabled(matches_current_branch || current_branch.is_empty() || !can_mutate_branch),
        );
    }

    items.push(MenuEntry::Separator);
    items.push(
        MenuEntry::item(
            "New Branch...",
            Lucide::GitBranchPlus,
            SidebarCommand::NewBranchFromRemoteBranch {
                remote: remote_name.clone(),
                branch: branch.clone(),
            },
        )
        .disabled(!can_mutate_branch || is_running("create-branch")),
    );
    items.push(
        MenuEntry::item(
            "New Tag...",
            Lucide::Tag,
            SidebarCommand::NewTagFromRemoteBranch {
                remote: remote_name.clone(),
                branch: branch.clone(),
            },
        )
        .disabled(!can_mutate_branch || is_running("create-tag")),
    );

    items.push(MenuEntry::Separator);
    items.push(
        MenuEntry::item(
            format!("Delete '{full_remote_branch}'..."),
            Lucide::Trash,
            SidebarCommand::DeleteRemoteBranch {
                remote: remote_name,
                branch,
            },
        )
        .danger()
        .disabled(!can_mutate_branch || is_running("delete-remote-branch")),
    );

    items.push(MenuEntry::Separator);
    items.push(MenuEntry::item(
        "Copy Branch Name",
        Lucide::Copy,
        SidebarCommand::CopyBranchName(full_remote_branch),
    ));

    items
}

/// Menú de un tag (`tagContextMenuItems`).
/// Menú contextual de un stash: lo mismo que sus botones de la fila, más
/// inspeccionarlo (hallazgo del QA de 0.12.0: el clic derecho no hacía nada).
pub fn stash_menu(repo: &OpenRepository, hash: &str) -> Vec<MenuEntry> {
    if hash.is_empty() {
        return Vec::new();
    }
    let can_mutate = can_mutate(repo);
    let hash = hash.to_string();
    vec![
        MenuEntry::item(
            "Inspect",
            Lucide::Eye,
            SidebarCommand::SelectStash(hash.clone()),
        ),
        MenuEntry::Separator,
        MenuEntry::item(
            "Apply stash",
            Lucide::ArrowDownToLine,
            SidebarCommand::ApplyStash(hash.clone()),
        )
        .disabled(!can_mutate),
        MenuEntry::item(
            "Pop stash",
            Lucide::PackageOpen,
            SidebarCommand::PopStash(hash.clone()),
        )
        .disabled(!can_mutate),
        MenuEntry::item(
            "Create branch from stash…",
            Lucide::GitBranchPlus,
            SidebarCommand::BranchFromStash(hash.clone()),
        )
        .disabled(!can_mutate),
        MenuEntry::Separator,
        MenuEntry::item(
            "Drop stash…",
            Lucide::Trash,
            SidebarCommand::DropStash(hash),
        )
        .danger()
        .disabled(!can_mutate),
    ]
}

pub fn tag_menu(repo: &OpenRepository, tag: &str) -> Vec<MenuEntry> {
    if tag.is_empty() {
        return Vec::new();
    }

    let can_mutate = can_mutate(repo);
    let tag_name = tag.to_string();

    let mut items = vec![
        MenuEntry::item(
            format!("Checkout '{tag}'"),
            Lucide::GitBranch,
            SidebarCommand::CheckoutTag(tag_name.clone()),
        )
        .disabled(!can_mutate),
        MenuEntry::item(
            "View details…",
            Lucide::Eye,
            SidebarCommand::ViewTagDetails(tag_name.clone()),
        ),
        MenuEntry::item(
            "Copy tag name",
            Lucide::Copy,
            SidebarCommand::CopyTagName(tag_name.clone()),
        ),
        MenuEntry::Separator,
    ];
    // Sin remotos no hay dónde publicar ni borrar: no se ofrece.
    if let Some(remote) = tag_remote(&repo.remotes) {
        items.push(
            MenuEntry::item(
                format!("Push '{tag}' to {remote}"),
                Lucide::Upload,
                SidebarCommand::PushTag(tag_name.clone()),
            )
            .disabled(!can_mutate),
        );
        items.push(
            MenuEntry::item(
                format!("Delete '{tag}' on {remote}…"),
                Lucide::Cloud,
                SidebarCommand::DeleteRemoteTag(tag_name.clone()),
            )
            .danger()
            .disabled(!can_mutate),
        );
    }
    items.push(
        MenuEntry::item(
            format!("Delete '{tag}'…"),
            Lucide::Trash,
            SidebarCommand::DeleteTag(tag_name),
        )
        .danger()
        .disabled(!can_mutate),
    );
    items
}

/// Menú de un remoto (`remoteContextMenuItems`).
pub fn remote_menu(repo: &OpenRepository, remote_name: &str) -> Vec<MenuEntry> {
    if remote_name.is_empty() {
        return Vec::new();
    }

    let can_mutate = can_mutate(repo);
    let name = remote_name.to_string();

    vec![
        MenuEntry::item(
            "Edit URL…",
            Lucide::Link2,
            SidebarCommand::EditRemoteUrl(name.clone()),
        )
        .disabled(!can_mutate),
        MenuEntry::item(
            format!("Rename '{remote_name}'…"),
            Lucide::Pencil,
            SidebarCommand::RenameRemote(name.clone()),
        )
        .disabled(!can_mutate),
        MenuEntry::Separator,
        MenuEntry::item(
            format!("Remove '{remote_name}'…"),
            Lucide::Trash,
            SidebarCommand::RemoveRemote(name),
        )
        .danger()
        .disabled(!can_mutate),
    ]
}

/// Posición de la entrada con esa etiqueta, contando separadores (es el índice
/// con el que el menú la identifica).
pub fn entry_index(entries: &[MenuEntry], label: &str) -> Option<usize> {
    entries.iter().position(|entry| entry.label() == label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_backend::OperationState;
    use mergeon_model::repository_read_model::RemoteRefs;

    fn branch(name: &str, active: bool) -> BranchItem {
        BranchItem {
            name: name.to_string(),
            active,
            is_remote: false,
            upstream: None,
            has_tracking: false,
            upstream_set: false,
            ahead: 0,
            behind: 0,
        }
    }

    fn repo() -> OpenRepository {
        let mut repo = OpenRepository::new("/tmp/demo", None).expect("repo");
        repo.current_branch = Some("main".to_string());
        repo.branches = vec![
            BranchItem {
                upstream: Some("origin/main".to_string()),
                has_tracking: true,
                upstream_set: true,
                ..branch("main", true)
            },
            branch("feature", false),
        ];
        repo.remotes = vec![RemoteItem {
            name: "origin".to_string(),
            refs: RemoteRefs {
                fetch: String::new(),
                push: "https://github.com/demo/repo.git".to_string(),
            },
            branches: vec!["main".to_string(), "remote-feature".to_string()],
        }];
        repo
    }

    fn labels(entries: &[MenuEntry]) -> Vec<&str> {
        entries.iter().map(MenuEntry::label).collect()
    }

    fn entry<'a>(entries: &'a [MenuEntry], label: &str) -> &'a MenuEntry {
        entries
            .iter()
            .find(|entry| entry.label() == label)
            .unwrap_or_else(|| panic!("no entry `{label}` in {:?}", labels(entries)))
    }

    const IDLE: &dyn Fn(&str) -> bool = &|_| false;

    #[test]
    fn local_branch_menu_without_upstream_offers_to_set_it() {
        let repo = repo();
        let menu = local_branch_menu(&repo, &repo.branches[1], IDLE);

        assert_eq!(
            labels(&menu),
            [
                "Checkout 'feature'",
                "",
                "Compare with 'main'...",
                "Merge into 'main'...",
                "Rebase 'main' onto 'feature'...",
                "",
                "New Branch...",
                "Check out in new worktree...",
                "New Tag...",
                "",
                "Set Upstream to origin/feature",
                "Rename 'feature'...",
                "Delete 'feature'...",
                "",
                "Copy Branch Name",
            ]
        );
        assert!(!entry(&menu, "Checkout 'feature'").is_disabled());
        // Los tags nuevos solo salen de la rama con checkout.
        assert!(entry(&menu, "New Tag...").is_disabled());
        assert!(entry(&menu, "Delete 'feature'...").is_danger());
        assert_eq!(
            entry(&menu, "Set Upstream to origin/feature").command(),
            Some(&SidebarCommand::SetupTracking("feature".to_string()))
        );
    }

    /// `Sidebar.test.js`: "disables destructive local branch actions for the
    /// active branch".
    #[test]
    fn disables_destructive_local_branch_actions_for_the_active_branch() {
        let repo = repo();
        let menu = local_branch_menu(&repo, &repo.branches[0], IDLE);

        assert!(entry(&menu, "Checkout 'main'").is_disabled());
        assert!(entry(&menu, "Delete 'main'...").is_disabled());
        assert!(entry(&menu, "Merge into 'main'...").is_disabled());
        // Con upstream: fast-forward (sin nada que traer, apagado), pull, push
        // y la línea informativa de tracking.
        assert!(entry(&menu, "Fast-Forward to 'origin/main'").is_disabled());
        assert!(!entry(&menu, "Pull 'origin/main'").is_disabled());
        assert!(!entry(&menu, "Push 'main' to 'origin'").is_disabled());
        assert!(entry(&menu, "Tracking: origin/main").is_disabled());
        assert!(!entry(&menu, "New Tag...").is_disabled());
    }

    #[test]
    fn fast_forward_needs_the_active_branch_to_be_behind() {
        let mut repo = repo();
        repo.branches[0].behind = 2;
        let menu = local_branch_menu(&repo, &repo.branches[0], IDLE);

        assert!(!entry(&menu, "Fast-Forward to 'origin/main'").is_disabled());

        let busy = |operation: &str| operation == "fast-forward-branch";
        let menu = local_branch_menu(&repo, &repo.branches[0], &busy);
        assert!(entry(&menu, "Fast-Forward to 'origin/main'").is_disabled());
    }

    #[test]
    fn an_operation_in_progress_disables_every_mutation() {
        let mut repo = repo();
        repo.operation_state = Some(OperationState {
            in_progress: true,
            ..OperationState::default()
        });
        let menu = local_branch_menu(&repo, &repo.branches[1], IDLE);

        for label in [
            "Checkout 'feature'",
            "Merge into 'main'...",
            "New Branch...",
            "Set Upstream to origin/feature",
            "Rename 'feature'...",
            "Delete 'feature'...",
        ] {
            assert!(entry(&menu, label).is_disabled(), "{label}");
        }
        assert!(!entry(&menu, "Copy Branch Name").is_disabled());

        let tags = tag_menu(&repo, "v1.0.0");
        assert!(entry(&tags, "Checkout 'v1.0.0'").is_disabled());
        assert!(!entry(&tags, "View details…").is_disabled());
        let remotes = remote_menu(&repo, "origin");
        assert!(entry(&remotes, "Remove 'origin'…").is_disabled());
    }

    /// `Sidebar.test.js`: "disables merge and rebase for a remote branch that
    /// matches the current branch".
    #[test]
    fn disables_merge_and_rebase_for_a_remote_branch_that_matches_the_current_branch() {
        let repo = repo();
        let menu = remote_branch_menu(&repo, &repo.remotes[0], "main", IDLE);

        assert!(entry(&menu, "Merge into 'main'...").is_disabled());
        assert!(entry(&menu, "Rebase 'main' onto 'origin/main'...").is_disabled());
        // Ya estás en ella: tampoco hay a dónde cambiar.
        assert!(entry(&menu, "Switch to 'main'").is_disabled());
    }

    #[test]
    fn remote_branch_menu_lists_every_action() {
        let repo = repo();
        let menu = remote_branch_menu(&repo, &repo.remotes[0], "remote-feature", IDLE);

        assert_eq!(
            labels(&menu),
            [
                "Checkout 'origin/remote-feature'",
                "Pull 'origin/remote-feature' into 'main'...",
                "",
                "Create Pull Request on origin...",
                "",
                "Compare with 'main'...",
                "Merge into 'main'...",
                "Rebase 'main' onto 'origin/remote-feature'...",
                "",
                "New Branch...",
                "New Tag...",
                "",
                "Delete 'origin/remote-feature'...",
                "",
                "Copy Branch Name",
            ]
        );
        assert_eq!(
            entry(&menu, "Create Pull Request on origin...").command(),
            Some(&SidebarCommand::OpenPullRequest(
                "https://github.com/demo/repo/compare/main...remote-feature?expand=1".to_string()
            ))
        );
        assert_eq!(
            entry_index(&menu, "Delete 'origin/remote-feature'..."),
            Some(12)
        );
    }

    #[test]
    fn a_remote_that_is_not_on_github_has_no_pull_request_entry() {
        let mut repo = repo();
        repo.remotes[0].refs.push = "/tmp/origin.git".to_string();
        let menu = remote_branch_menu(&repo, &repo.remotes[0], "remote-feature", IDLE);

        assert!(
            !labels(&menu)
                .iter()
                .any(|label| label.starts_with("Create Pull Request"))
        );
    }

    #[test]
    fn tag_and_remote_menus() {
        let repo = repo();

        assert_eq!(
            labels(&tag_menu(&repo, "v1.0.0")),
            [
                "Checkout 'v1.0.0'",
                "View details…",
                "Copy tag name",
                "",
                "Push 'v1.0.0' to origin",
                "Delete 'v1.0.0' on origin…",
                "Delete 'v1.0.0'…",
            ]
        );
        assert_eq!(
            labels(&remote_menu(&repo, "origin")),
            ["Edit URL…", "Rename 'origin'…", "", "Remove 'origin'…"]
        );
    }

    #[test]
    fn a_detached_head_disables_merge_and_rebase_without_empty_names() {
        let mut repo = repo();
        repo.current_branch = None;
        repo.branches[0].active = false;
        let menu = local_branch_menu(&repo, &repo.branches[1], IDLE);
        assert!(entry(&menu, "Merge into current branch...").is_disabled());
        assert!(entry(&menu, "Rebase current branch onto 'feature'...").is_disabled());
        assert!(!labels(&menu).iter().any(|label| label.contains("''")));

        let menu = remote_branch_menu(&repo, &repo.remotes[0], "remote-feature", IDLE);
        assert!(entry(&menu, "Merge into current branch...").is_disabled());
        assert!(
            entry(
                &menu,
                "Rebase current branch onto 'origin/remote-feature'..."
            )
            .is_disabled()
        );
    }

    #[test]
    fn rebase_label_names_the_branch_that_is_rewritten() {
        let repo = repo();
        let menu = local_branch_menu(&repo, &repo.branches[1], IDLE);
        // Branch Actions con `feature` de origen y `main` de destino rebasa
        // `main` sobre `feature`: la etiqueta lo dice así.
        assert_eq!(
            entry(&menu, "Rebase 'main' onto 'feature'...").command(),
            Some(&SidebarCommand::BranchAction {
                branch: "feature".to_string(),
                action: BranchAction::Rebase,
            })
        );
    }

    #[test]
    fn tag_menu_uses_the_repository_remote() {
        let mut repo = repo();
        repo.remotes[0].name = "upstream".to_string();
        let menu = tag_menu(&repo, "v1.0.0");
        assert!(labels(&menu).contains(&"Push 'v1.0.0' to upstream"));
        assert!(labels(&menu).contains(&"Delete 'v1.0.0' on upstream…"));

        repo.remotes.clear();
        let menu = tag_menu(&repo, "v1.0.0");
        assert_eq!(
            labels(&menu),
            [
                "Checkout 'v1.0.0'",
                "View details…",
                "Copy tag name",
                "",
                "Delete 'v1.0.0'…",
            ]
        );
    }
}
