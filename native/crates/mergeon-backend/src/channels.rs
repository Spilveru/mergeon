//! Tabla canal IPC → método del backend.
//!
//! Sirve de índice para quien porta la capa de arriba y de red de seguridad:
//! un test contrasta la tabla con `electron/preload.js` (ningún canal sin
//! entrada) y con el código de este crate (ninguna entrada sin método).

/// Un canal de `electron/preload.js` y el método que lo sustituye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMapping {
    /// Nombre del canal IPC (`git:getStatus`).
    pub channel: &'static str,
    /// Accesor del [`crate::Backend`] que da el objeto con el método.
    pub accessor: &'static str,
    /// Nombre del método.
    pub method: &'static str,
}

const fn map(
    channel: &'static str,
    accessor: &'static str,
    method: &'static str,
) -> ChannelMapping {
    ChannelMapping {
        channel,
        accessor,
        method,
    }
}

/// Canales `git:*` → [`crate::GitApi`].
pub const GIT_CHANNELS: &[ChannelMapping] = &[
    map("git:isValidRepository", "git", "is_valid_repository"),
    map("git:inspectRepository", "git", "inspect_repository"),
    map("git:initRepository", "git", "init_repository"),
    map("git:getCommits", "git", "get_commits"),
    map("git:searchCommits", "git", "search_commits"),
    map("git:resolveRef", "git", "resolve_ref"),
    map("git:getIdentity", "git", "get_identity"),
    map("git:setIdentity", "git", "set_identity"),
    map("git:getReflog", "git", "get_reflog"),
    map("git:getBranches", "git", "get_branches"),
    map("git:getRemotes", "git", "get_remotes"),
    map("git:getSubmodules", "git", "get_submodules"),
    map("git:submoduleUpdate", "git", "submodule_update"),
    map("git:getRepositorySummary", "git", "get_repository_summary"),
    map("git:addRemote", "git", "add_remote"),
    map("git:removeRemote", "git", "remove_remote"),
    map("git:renameRemote", "git", "rename_remote"),
    map("git:setRemoteUrl", "git", "set_remote_url"),
    map("git:testRemoteConnection", "git", "test_remote_connection"),
    map("git:getStatus", "git", "get_status"),
    map("git:watchStatus", "git", "watch_status"),
    map("git:unwatchStatus", "git", "unwatch_status"),
    map(
        "git:setStatusWatchWindowFocus",
        "git",
        "set_status_watch_window_focus",
    ),
    map("git:getOperationState", "git", "get_operation_state"),
    map("git:cancelOperation", "git", "cancel_operation"),
    map("git:getOperationQueue", "git", "get_operation_queue"),
    map("git:getTags", "git", "get_tags"),
    map("git:getTagDetails", "git", "get_tag_details"),
    map("git:checkMergeConflicts", "git", "check_merge_conflicts"),
    map("git:getStashes", "git", "get_stashes"),
    map("git:getStashFiles", "git", "get_stash_files"),
    map("git:getStashDiff", "git", "get_stash_diff"),
    map("git:fetch", "git", "fetch"),
    map("git:pull", "git", "pull"),
    map("git:push", "git", "push"),
    map("git:checkout", "git", "checkout"),
    map("git:checkoutRemoteBranch", "git", "checkout_remote_branch"),
    map("git:createBranch", "git", "create_branch"),
    map("git:renameBranch", "git", "rename_branch"),
    map("git:deleteBranch", "git", "delete_branch"),
    map("git:deleteRemoteBranch", "git", "delete_remote_branch"),
    map("git:fastForwardBranch", "git", "fast_forward_branch"),
    map("git:merge", "git", "merge"),
    map("git:rebase", "git", "rebase"),
    map("git:compareBranches", "git", "compare_branches"),
    map("git:stash", "git", "stash"),
    map("git:applyStash", "git", "apply_stash"),
    map("git:popStash", "git", "pop_stash"),
    map("git:dropStash", "git", "drop_stash"),
    map("git:stashBranch", "git", "stash_branch"),
    map("git:createTag", "git", "create_tag"),
    map("git:deleteTag", "git", "delete_tag"),
    map("git:pushTag", "git", "push_tag"),
    map("git:deleteRemoteTag", "git", "delete_remote_tag"),
    map("git:checkoutTag", "git", "checkout_tag"),
    map(
        "git:hasUncommittedChanges",
        "git",
        "has_uncommitted_changes",
    ),
    map("git:setupTracking", "git", "setup_tracking"),
    map("git:reset", "git", "reset"),
    map("git:revert", "git", "revert"),
    map("git:cherryPick", "git", "cherry_pick"),
    map("git:addToIgnore", "git", "add_to_ignore"),
    map("git:blame", "git", "blame"),
    map("git:readFile", "git", "read_file"),
    map("git:resolveConflict", "git", "resolve_conflict"),
    map("git:rebaseAction", "git", "rebase_action"),
    map("git:interactiveRebase", "git", "interactive_rebase"),
    map("git:sequenceAction", "git", "sequence_action"),
    map("git:stage", "git", "stage"),
    map("git:applyPatch", "git", "apply_patch"),
    map("git:stageAll", "git", "stage_all"),
    map("git:unstage", "git", "unstage"),
    map("git:unstageAll", "git", "unstage_all"),
    map("git:discardChanges", "git", "discard_changes"),
    map("git:discardAll", "git", "discard_all"),
    map("git:commit", "git", "commit"),
    map("git:clone", "git", "clone"),
    map("git:getDiff", "git", "get_diff"),
    map("git:getCommitDiff", "git", "get_commit_diff"),
    map("git:getCommitFiles", "git", "get_commit_files"),
    map("git:getTreeEntries", "git", "get_tree_entries"),
    map("git:getFileAtCommit", "git", "get_file_at_commit"),
];

/// Canales `shell:*` → [`crate::Shell`].
pub const SHELL_CHANNELS: &[ChannelMapping] = &[
    map("shell:openExternal", "shell", "open_external"),
    map("shell:openPath", "shell", "open_path"),
    map("shell:showItemInFolder", "shell", "show_item_in_folder"),
    map("shell:openTerminal", "shell", "open_terminal"),
];

/// Canales `storage:*` → `mergeon_store::Store`.
pub const STORAGE_CHANNELS: &[ChannelMapping] = &[
    map(
        "storage:getGitHubAccounts",
        "storage",
        "get_github_accounts",
    ),
    map(
        "storage:saveGitHubAccount",
        "storage",
        "save_github_account",
    ),
    map(
        "storage:removeGitHubAccount",
        "storage",
        "remove_github_account",
    ),
    map(
        "storage:getActiveGitHubAccountId",
        "storage",
        "get_active_github_account_id",
    ),
    map(
        "storage:setActiveGitHubAccountId",
        "storage",
        "set_active_github_account_id",
    ),
    map(
        "storage:getRecentRepositories",
        "storage",
        "get_recent_repositories",
    ),
    map(
        "storage:addRecentRepository",
        "storage",
        "add_recent_repository",
    ),
    map(
        "storage:removeRecentRepository",
        "storage",
        "remove_recent_repository",
    ),
    map(
        "storage:clearRecentRepositories",
        "storage",
        "clear_recent_repositories",
    ),
    map(
        "storage:getWorkspaceSession",
        "storage",
        "get_workspace_session",
    ),
    map(
        "storage:setWorkspaceSession",
        "storage",
        "set_workspace_session",
    ),
    map(
        "storage:clearWorkspaceSession",
        "storage",
        "clear_workspace_session",
    ),
    map("storage:getSettings", "storage", "get_settings"),
    map("storage:updateSettings", "storage", "update_settings"),
    map(
        "storage:getStarredBranches",
        "storage",
        "get_starred_branches",
    ),
    map("storage:addStarredBranch", "storage", "add_starred_branch"),
    map(
        "storage:removeStarredBranch",
        "storage",
        "remove_starred_branch",
    ),
];

/// Canales `github:*` → `mergeon_github::GitHubClient`.
pub const GITHUB_CHANNELS: &[ChannelMapping] = &[
    map("github:auth:start", "github", "auth_start"),
    map("github:auth:poll", "github", "auth_poll"),
    map("github:auth:refresh", "github", "auth_refresh"),
    map("github:auth:token", "github", "auth_token"),
    map(
        "github:getUserRepositories",
        "github",
        "get_user_repositories",
    ),
    map("github:searchRepositories", "github", "search_repositories"),
];

/// Canales `updater:*` → [`crate::UpdaterApi`].
pub const UPDATER_CHANNELS: &[ChannelMapping] = &[
    map("updater:getState", "updater", "get_state"),
    map("updater:check", "updater", "check"),
    map("updater:install", "updater", "install"),
];

/// Canales que no pertenecen al backend: son de la ventana y los resuelve la
/// capa GPUI.
pub const WINDOW_CHANNELS: &[&str] = &[
    "app:set-title-bar-theme",
    "app:reload",
    "app:minimize",
    "app:quit",
    "app:toggleDevTools",
    "dialog:openDirectory",
];

/// Todos los canales con método en el backend.
pub fn all_channels() -> impl Iterator<Item = &'static ChannelMapping> {
    GIT_CHANNELS
        .iter()
        .chain(SHELL_CHANNELS)
        .chain(STORAGE_CHANNELS)
        .chain(GITHUB_CHANNELS)
        .chain(UPDATER_CHANNELS)
}

/// El método que sustituye a `channel`, si el backend lo cubre.
pub fn method_for_channel(channel: &str) -> Option<&'static ChannelMapping> {
    all_channels().find(|mapping| mapping.channel == channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Código fuente de las clases de este crate que implementan cada accesor.
    fn sources_for(accessor: &str) -> String {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files: &[&str] = match accessor {
            "git" => &[
                "git/mod.rs",
                "git/reads.rs",
                "git/mutations.rs",
                "git/handlers.rs",
            ],
            "shell" => &["shell.rs"],
            "updater" => &["updater.rs"],
            other => panic!("unknown accessor {other}"),
        };
        files
            .iter()
            .map(|file| {
                let path = source.join(file);
                std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Los nombres de los métodos de `$type`, comprobados al compilar: si uno
    /// deja de existir, el test no compila.
    macro_rules! methods_of {
        ($type:ty: $($method:ident),+ $(,)?) => {{
            $(let _ = <$type>::$method;)+
            BTreeSet::from([$(stringify!($method)),+])
        }};
    }

    #[test]
    fn every_mapped_method_of_this_crate_exists() {
        for accessor in ["git", "shell", "updater"] {
            let sources = sources_for(accessor);
            for mapping in all_channels().filter(|mapping| mapping.accessor == accessor) {
                let needle = format!("pub fn {}(", mapping.method);
                let generic = format!("pub fn {}<", mapping.method);
                assert!(
                    sources.contains(&needle) || sources.contains(&generic),
                    "{} -> {}().{}() is not implemented",
                    mapping.channel,
                    accessor,
                    mapping.method
                );
            }
        }
    }

    #[test]
    fn storage_and_github_channels_map_to_real_methods() {
        let mapped = |channels: &[ChannelMapping]| -> BTreeSet<&'static str> {
            channels.iter().map(|mapping| mapping.method).collect()
        };

        assert_eq!(
            mapped(STORAGE_CHANNELS),
            methods_of!(mergeon_store::Store:
                get_github_accounts,
                save_github_account,
                remove_github_account,
                get_active_github_account_id,
                set_active_github_account_id,
                get_recent_repositories,
                add_recent_repository,
                remove_recent_repository,
                clear_recent_repositories,
                get_workspace_session,
                set_workspace_session,
                clear_workspace_session,
                get_settings,
                update_settings,
                get_starred_branches,
                add_starred_branch,
                remove_starred_branch,
            )
        );
        assert_eq!(
            mapped(GITHUB_CHANNELS),
            methods_of!(mergeon_github::GitHubClient:
                auth_start,
                auth_poll,
                auth_refresh,
                auth_token,
                get_user_repositories,
                search_repositories,
            )
        );
    }

    #[test]
    fn channels_and_methods_are_unique() {
        let channels: BTreeSet<&str> = all_channels().map(|mapping| mapping.channel).collect();
        assert_eq!(channels.len(), all_channels().count());

        let methods: BTreeSet<(&str, &str)> = all_channels()
            .map(|mapping| (mapping.accessor, mapping.method))
            .collect();
        assert_eq!(methods.len(), all_channels().count());
    }

    #[test]
    fn method_names_are_the_channel_names_in_snake_case() {
        fn snake_case(name: &str) -> String {
            let mut result = String::new();
            let characters: Vec<char> = name.chars().collect();
            for (index, character) in characters.iter().enumerate() {
                if character.is_ascii_uppercase() {
                    // `GitHub` es una sola palabra en los nombres del store.
                    let previous_upper = index > 0 && characters[index - 1].is_ascii_uppercase();
                    let next_lower = characters
                        .get(index + 1)
                        .is_some_and(|next| next.is_ascii_lowercase());
                    if index > 0 && (!previous_upper || next_lower) {
                        result.push('_');
                    }
                    result.push(character.to_ascii_lowercase());
                } else {
                    result.push(*character);
                }
            }
            result.replace("git_hub", "github")
        }

        for mapping in all_channels() {
            let (_, name) = mapping.channel.split_once(':').unwrap();
            // `github:auth:start` → `auth_start`.
            let expected = snake_case(&name.replace(':', "_"));
            assert_eq!(mapping.method, expected, "{}", mapping.channel);
        }
    }

    #[test]
    fn looks_up_a_channel() {
        let mapping = method_for_channel("git:setStatusWatchWindowFocus").unwrap();
        assert_eq!(mapping.accessor, "git");
        assert_eq!(mapping.method, "set_status_watch_window_focus");
        assert_eq!(method_for_channel("app:quit"), None);
    }
}
