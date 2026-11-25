//! Canales de mutación cuya operación vive en `mergeon-git-ops`: cada uno es
//! un `registerRepoMutationHandler` del original, con su nombre de operación,
//! su prefijo de mensaje, su clave de dedupe, su timeout y el `extra` que
//! acompaña a un fallo del handler.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mergeon_git_ops::{
    ApplyPatchOptions, CleanupBranchesOptions, CommitOptions, ConflictResolution, FetchOptions,
    FetchPullRequestOptions, InteractiveRebaseOptions, MergeOptions, MutationResult,
    OperationResult, PullOptions, PullRequestHead, PushOptions, RebaseActionParams, RebaseOptions,
    RemoteTagOptions, ResolveConflictResult, SequenceActionResult, SetIdentityOptions,
    SetIdentityResult, StashOptions, SubmoduleUpdateOptions,
};
use mergeon_git_query::normalize_repository_file_path;
use mergeon_gitcmd::NETWORK_TIMEOUT;
use serde::Serialize;
use serde_json::{Value, json};

use super::exec::path_text;
use super::{GitApi, MutationConfig};
use crate::mutation::{Mutation, StructuredResult, Thrown};
use mergeon_git_ops::undo::{DiscardProbe, RefProbe, UndoStep};

/// Rebase interactivo completo: puede tocar muchos commits, timeout amplio.
const INTERACTIVE_REBASE_TIMEOUT: Duration = Duration::from_millis(120_000);

/// `value || null`.
pub(super) fn or_null(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

/// Un argumento opcional del canal, como viajaba por IPC.
pub(super) fn optional(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_string()))
}

/// Las opciones del canal como el objeto que viajaba por IPC.
fn options_json<T: Serialize>(options: &T) -> Value {
    serde_json::to_value(options).unwrap_or(Value::Null)
}

/// `extra` de los canales que actúan sobre un archivo.
fn file_extra(file_path: &str) -> Value {
    json!({
        "filePath": or_null(file_path),
        "filesAffected": if file_path.is_empty() { json!([]) } else { json!([file_path]) },
    })
}

/// `extra` de merge, rebase, revert y cherry-pick.
fn sequence_extra(branch: Value) -> Value {
    json!({
        "conflictFiles": [],
        "operationState": null,
        "branch": branch,
        "remote": null,
        "upstreamSet": false,
    })
}

/// Lo que se captura en merge / rebase: HEAD y, si cambian antes a otra rama,
/// dónde estaba esa rama (para poder deshacerlo).
fn sequence_probe(target_branch: Option<&str>) -> RefProbe {
    match target_branch.filter(|branch| !branch.is_empty()) {
        Some(branch) => RefProbe::head().with_ref(format!("refs/heads/{branch}")),
        None => RefProbe::head(),
    }
}

/// Configuración del canal `git:push`.
pub(crate) fn push_config(repo_path: &Path, options: &PushOptions) -> MutationConfig {
    let remote = if options.remote.is_empty() {
        "origin"
    } else {
        &options.remote
    };
    MutationConfig::new(
        "push",
        "Failed to push",
        json!([path_text(repo_path), options_json(options)]),
    )
    .structured_failure()
    .extra(json!({
        "conflictFiles": [],
        "operationState": null,
        "branch": or_null(options.branch.as_deref().unwrap_or_default()),
        "remote": remote,
        "upstreamSet": false,
    }))
}

impl GitApi {
    /// Encola `run` para `repo_path` con la configuración del canal.
    pub(super) fn mutate<T, F>(
        &self,
        repo_path: &Path,
        config: MutationConfig,
        run: F,
    ) -> Mutation<T>
    where
        T: StructuredResult + Clone + Send + 'static,
        F: FnOnce(&Path) -> Result<T, Thrown> + Send + 'static,
    {
        let repo = repo_path.to_path_buf();
        self.run_repo_mutation(repo_path, config, move || run(&repo))
    }

    /// Canal `git:setIdentity`. Escritura de identidad: no toca índice ni
    /// working tree, fuera de la cola.
    pub fn set_identity(
        &self,
        repo_path: impl AsRef<Path>,
        options: &SetIdentityOptions,
    ) -> SetIdentityResult {
        mergeon_git_ops::set_git_identity_operation(repo_path.as_ref(), options)
    }

    // Gestión de remotes: agregar, quitar, renombrar y cambiar URL.

    /// Canal `git:addRemote`.
    pub fn add_remote(
        &self,
        repo_path: impl AsRef<Path>,
        name: &str,
        url: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (name_arg, url_arg) = (name.to_string(), url.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "add-remote",
                "Failed to add remote",
                json!([path_text(repo), name, url]),
            )
            .extra(json!({ "remote": or_null(name) })),
            move |repo| {
                Ok(mergeon_git_ops::add_remote_operation(
                    repo, &name_arg, &url_arg,
                ))
            },
        )
    }

    /// Canal `git:removeRemote`.
    pub fn remove_remote(
        &self,
        repo_path: impl AsRef<Path>,
        name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let name_arg = name.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "remove-remote",
                "Failed to remove remote",
                json!([path_text(repo), name]),
            )
            .extra(json!({ "remote": or_null(name) })),
            move |repo| Ok(mergeon_git_ops::remove_remote_operation(repo, &name_arg)),
        )
    }

    /// Canal `git:renameRemote`.
    pub fn rename_remote(
        &self,
        repo_path: impl AsRef<Path>,
        name: &str,
        new_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (name_arg, new_name_arg) = (name.to_string(), new_name.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "rename-remote",
                "Failed to rename remote",
                json!([path_text(repo), name, new_name]),
            )
            .extra(json!({ "remote": or_null(name) })),
            move |repo| {
                Ok(mergeon_git_ops::rename_remote_operation(
                    repo,
                    &name_arg,
                    &new_name_arg,
                ))
            },
        )
    }

    /// Canal `git:setRemoteUrl`.
    pub fn set_remote_url(
        &self,
        repo_path: impl AsRef<Path>,
        name: &str,
        url: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (name_arg, url_arg) = (name.to_string(), url.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "set-remote-url",
                "Failed to update the remote URL",
                json!([path_text(repo), name, url]),
            )
            .extra(json!({ "remote": or_null(name) })),
            move |repo| {
                Ok(mergeon_git_ops::set_remote_url_operation(
                    repo, &name_arg, &url_arg,
                ))
            },
        )
    }

    /// Canal `git:submoduleUpdate`: actualiza submódulos (init incluido).
    pub fn submodule_update(
        &self,
        repo_path: impl AsRef<Path>,
        options: &SubmoduleUpdateOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "submodule-update",
                "Failed to update submodules",
                json!([path_text(repo), options_json(options)]),
            )
            .timeout(NETWORK_TIMEOUT),
            move |repo| {
                Ok(mergeon_git_ops::submodule_update_operation(
                    repo,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:fetch`. Fetch actualiza refs locales y por eso también pasa
    /// por la cola.
    pub fn fetch(
        &self,
        repo_path: impl AsRef<Path>,
        options: &FetchOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new("fetch", "Failed to fetch", Value::Null)
                .request_key(json!([options_json(options)])),
            move |repo| Ok(mergeon_git_ops::fetch_operation(repo, &options_arg)),
        )
    }

    /// Trae la rama de un pull request de GitHub para hacerle checkout
    /// (`mergeon_git_ops::fetch_pull_request_operation`): sin canal en el
    /// original. Va por la cola y se anota como un `fetch`.
    pub fn fetch_pull_request(
        &self,
        repo_path: impl AsRef<Path>,
        options: &FetchPullRequestOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let head = match &options.head {
            PullRequestHead::SameRepository { branch } => json!({ "branch": branch }),
            PullRequestHead::Fork { number } => json!({ "pullRequest": number }),
        };
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new("fetch", "Failed to fetch", Value::Null).request_key(json!([
                "pull-request",
                options.remote,
                head
            ])),
            move |repo| {
                Ok(mergeon_git_ops::fetch_pull_request_operation(
                    repo,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:pull`. Pull actualiza refs y worktree/HEAD; comparte el
    /// mismo contrato estructurado.
    pub fn pull(
        &self,
        repo_path: impl AsRef<Path>,
        options: &PullOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new("pull", "Failed to pull", Value::Null)
                .capture(RefProbe::head())
                .request_key(json!([options_json(options)])),
            move |repo| Ok(mergeon_git_ops::pull_operation(repo, &options_arg)),
        )
    }

    /// Canal `git:push`.
    pub fn push(
        &self,
        repo_path: impl AsRef<Path>,
        options: &PushOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(repo, push_config(repo, options), move |repo| {
            Ok(mergeon_git_ops::push_operation(repo, &options_arg))
        })
    }

    /// Canal `git:checkout`.
    pub fn checkout(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let branch_arg = branch_name.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "checkout",
                "Failed to checkout branch",
                json!([path_text(repo), branch_name]),
            )
            .capture(RefProbe::head())
            .extra(json!({ "branch": or_null(branch_name) })),
            move |repo| Ok(mergeon_git_ops::checkout_operation(repo, &branch_arg)),
        )
    }

    /// Canal `git:checkoutRemoteBranch`: checkout de una rama remota con tracking.
    pub fn checkout_remote_branch(
        &self,
        repo_path: impl AsRef<Path>,
        remote_branch: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let remote_branch_arg = remote_branch.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "checkout-remote-branch",
                "Failed to checkout remote branch",
                json!([path_text(repo), remote_branch]),
            )
            .capture(
                RefProbe::head().with_branch(
                    remote_branch
                        .split_once('/')
                        .map_or("", |(_, branch)| branch),
                ),
            )
            .extra(json!({ "remote": or_null(remote_branch) })),
            move |repo| {
                Ok(mergeon_git_ops::checkout_remote_branch_operation(
                    repo,
                    &remote_branch_arg,
                ))
            },
        )
    }

    /// Canal `git:createBranch`. En el original `start_point` es `null` y
    /// `checkout_after_create` es `true` por defecto.
    pub fn create_branch(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
        start_point: Option<&str>,
        checkout_after_create: bool,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let branch_arg = branch_name.to_string();
        let start_point_arg = start_point.map(str::to_string);
        self.mutate(
            repo,
            MutationConfig::new(
                "create-branch",
                "Failed to create branch",
                json!([
                    path_text(repo),
                    branch_name,
                    optional(start_point),
                    checkout_after_create
                ]),
            )
            .capture(RefProbe::head().with_branch(branch_name))
            .extra(json!({ "branch": or_null(branch_name) })),
            move |repo| {
                Ok(mergeon_git_ops::create_branch_operation(
                    repo,
                    &branch_arg,
                    start_point_arg.as_deref(),
                    checkout_after_create,
                ))
            },
        )
    }

    /// Canal `git:renameBranch`.
    pub fn rename_branch(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
        next_branch_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (branch_arg, next_arg) = (branch_name.to_string(), next_branch_name.to_string());
        let reported_branch = if next_branch_name.is_empty() {
            branch_name
        } else {
            next_branch_name
        };
        self.mutate(
            repo,
            MutationConfig::new(
                "rename-branch",
                "Failed to rename branch",
                json!([path_text(repo), branch_name, next_branch_name]),
            )
            .extra(json!({ "branch": or_null(reported_branch) })),
            move |repo| {
                Ok(mergeon_git_ops::rename_branch_operation(
                    repo,
                    &branch_arg,
                    &next_arg,
                ))
            },
        )
    }

    /// Canal `git:deleteBranch`.
    pub fn delete_branch(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let branch_arg = branch_name.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "delete-branch",
                "Failed to delete branch",
                json!([path_text(repo), branch_name]),
            )
            .capture(RefProbe::head().with_branch(branch_name))
            .extra(json!({ "branch": or_null(branch_name) })),
            move |repo| Ok(mergeon_git_ops::delete_branch_operation(repo, &branch_arg)),
        )
    }

    /// Canal `git:cleanupBranches` (sin original): borra en lote las ramas
    /// elegidas en el diálogo de limpieza y, si se pide, sus ramas remotas.
    /// El token no viaja en los argumentos que se registran.
    pub fn cleanup_branches(
        &self,
        repo_path: impl AsRef<Path>,
        options: &CleanupBranchesOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        let mut config = MutationConfig::new(
            "cleanup-branches",
            "Failed to clean up branches",
            json!([path_text(repo), options_json(options)]),
        );
        // Con borrado remoto, un push por rama: el tiempo de las de red.
        if options.delete_remote {
            config = config.timeout(NETWORK_TIMEOUT);
        }
        self.mutate(repo, config, move |repo| {
            Ok(mergeon_git_ops::cleanup_branches_operation(
                repo,
                &options_arg,
            ))
        })
    }

    /// Canal `git:deleteRemoteBranch`, sin token de autenticación como en la
    /// app Electron. Ver [`Self::delete_remote_branch_with_auth`].
    pub fn delete_remote_branch(
        &self,
        repo_path: impl AsRef<Path>,
        remote_name: &str,
        branch_name: &str,
    ) -> Mutation<MutationResult> {
        self.delete_remote_branch_with_auth(repo_path, remote_name, branch_name, None)
    }

    /// `git:deleteRemoteBranch` autenticando con el token de GitHub
    /// (`auth_token`), como push, fetch, pull y los tags remotos. El token no
    /// viaja en los argumentos que se registran de la operación.
    pub fn delete_remote_branch_with_auth(
        &self,
        repo_path: impl AsRef<Path>,
        remote_name: &str,
        branch_name: &str,
        auth_token: Option<&str>,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (remote_arg, branch_arg) = (remote_name.to_string(), branch_name.to_string());
        let token_arg = auth_token.map(str::to_string);
        self.mutate(
            repo,
            MutationConfig::new(
                "delete-remote-branch",
                "Failed to delete remote branch",
                json!([path_text(repo), remote_name, branch_name]),
            )
            .extra(json!({
                "branch": or_null(branch_name),
                "remote": or_null(remote_name),
            })),
            move |repo| {
                Ok(mergeon_git_ops::delete_remote_branch_with_auth_operation(
                    repo,
                    &remote_arg,
                    &branch_arg,
                    token_arg.as_deref(),
                ))
            },
        )
    }

    /// Canal `git:fastForwardBranch`.
    pub fn fast_forward_branch(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
        upstream_branch: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (branch_arg, upstream_arg) = (branch_name.to_string(), upstream_branch.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "fast-forward-branch",
                "Failed to fast-forward branch",
                json!([path_text(repo), branch_name, upstream_branch]),
            )
            .capture(RefProbe::head())
            .extra(json!({
                "branch": or_null(branch_name),
                "remote": or_null(upstream_branch),
            })),
            move |repo| {
                Ok(mergeon_git_ops::fast_forward_branch_operation(
                    repo,
                    &branch_arg,
                    &upstream_arg,
                ))
            },
        )
    }

    /// Canal `git:applyPatch`. El parche llega ya recortado desde la interfaz,
    /// que es quien tiene el diff.
    pub fn apply_patch(
        &self,
        repo_path: impl AsRef<Path>,
        patch: &str,
        options: &ApplyPatchOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let patch_arg = patch.to_string();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "apply-patch",
                "Failed to apply the selected block",
                json!([path_text(repo), patch, options_json(options)]),
            ),
            move |repo| {
                let file_path = match options_arg.file_path.as_deref() {
                    Some(file_path) if !file_path.is_empty() => {
                        Some(normalize_repository_file_path(repo, file_path)?)
                    }
                    _ => None,
                };
                Ok(mergeon_git_ops::apply_patch_operation(
                    repo,
                    &patch_arg,
                    &ApplyPatchOptions {
                        file_path,
                        ..options_arg
                    },
                ))
            },
        )
    }

    /// Canal `git:stage`.
    pub fn stage(&self, repo_path: impl AsRef<Path>, file_path: &str) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let file_arg = file_path.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "stage",
                "Failed to stage file",
                json!([path_text(repo), file_path]),
            )
            .extra(file_extra(file_path)),
            move |repo| {
                Ok(mergeon_git_ops::stage_operation(
                    repo,
                    &normalize_repository_file_path(repo, &file_arg)?,
                ))
            },
        )
    }

    /// Canal `git:stageAll`.
    pub fn stage_all(&self, repo_path: impl AsRef<Path>) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        self.mutate(
            repo,
            MutationConfig::new(
                "stage-all",
                "Failed to stage files",
                json!([path_text(repo)]),
            ),
            |repo| Ok(mergeon_git_ops::stage_all_operation(repo)),
        )
    }

    /// Canal `git:unstage`.
    pub fn unstage(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let file_arg = file_path.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "unstage",
                "Failed to unstage file",
                json!([path_text(repo), file_path]),
            )
            .extra(file_extra(file_path)),
            move |repo| {
                Ok(mergeon_git_ops::unstage_operation(
                    repo,
                    &normalize_repository_file_path(repo, &file_arg)?,
                ))
            },
        )
    }

    /// Canal `git:unstageAll`.
    pub fn unstage_all(&self, repo_path: impl AsRef<Path>) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        self.mutate(
            repo,
            MutationConfig::new(
                "unstage-all",
                "Failed to unstage files",
                json!([path_text(repo)]),
            ),
            |repo| Ok(mergeon_git_ops::unstage_all_operation(repo)),
        )
    }

    /// Canal `git:discardChanges`.
    pub fn discard_changes(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
    ) -> Mutation<MutationResult> {
        self.discard_changes_backed_up(repo_path, file_path, None)
    }

    /// `git:discardChanges` guardando antes, en el mismo turno de la cola,
    /// una copia recuperable de lo que se descarta en `backup.directory`
    /// (`Mutation::capture` → `discard`). Sin `backup`, como
    /// [`Self::discard_changes`].
    pub fn discard_changes_backed_up(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
        backup: Option<(PathBuf, u64)>,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let file_arg = file_path.to_string();
        let mut config = MutationConfig::new(
            "discard",
            "Failed to discard changes",
            json!([path_text(repo), file_path]),
        )
        .extra(file_extra(file_path));
        if let Some((directory, limit)) = backup
            && let Ok(normalized) = normalize_repository_file_path(repo, file_path)
        {
            config = config.capture(RefProbe::head().with_discard(DiscardProbe {
                paths: Some(vec![normalized]),
                directory,
                limit,
            }));
        }
        self.mutate(repo, config, move |repo| {
            Ok(mergeon_git_ops::discard_changes_operation(
                repo,
                &normalize_repository_file_path(repo, &file_arg)?,
            ))
        })
    }

    /// Canal `git:discardAll`. Sin `file_paths` descarta todos los cambios.
    pub fn discard_all(
        &self,
        repo_path: impl AsRef<Path>,
        file_paths: Option<&[String]>,
    ) -> Mutation<MutationResult> {
        self.discard_all_backed_up(repo_path, file_paths, None)
    }

    /// `git:discardAll` con copia recuperable (ver
    /// [`Self::discard_changes_backed_up`]).
    pub fn discard_all_backed_up(
        &self,
        repo_path: impl AsRef<Path>,
        file_paths: Option<&[String]>,
        backup: Option<(PathBuf, u64)>,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let file_paths_arg = file_paths.map(<[String]>::to_vec);
        let mut config = MutationConfig::new(
            "discard-all",
            "Failed to discard changes",
            json!([path_text(repo), file_paths]),
        );
        if let Some((directory, limit)) = backup {
            let paths = file_paths.map(|paths| {
                paths
                    .iter()
                    .filter_map(|path| normalize_repository_file_path(repo, path).ok())
                    .collect()
            });
            config = config.capture(RefProbe::head().with_discard(DiscardProbe {
                paths,
                directory,
                limit,
            }));
        }
        self.mutate(repo, config, move |repo| {
            let normalized = match file_paths_arg {
                Some(file_paths) => Some(
                    file_paths
                        .iter()
                        .map(|file_path| normalize_repository_file_path(repo, file_path))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                None => None,
            };
            Ok(mergeon_git_ops::discard_all_operation(
                repo,
                normalized.as_deref(),
            ))
        })
    }

    /// Deshace (o, con `redo`, rehace) una operación con sus pasos
    /// (`mergeon_git_ops::undo::undo_operation`), por la cola como cualquier
    /// mutación: sale en el log de operaciones como `undo` / `redo`.
    pub fn undo(
        &self,
        repo_path: impl AsRef<Path>,
        label: &str,
        steps: &[UndoStep],
        redo: bool,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (operation, prefix) = if redo {
            ("redo", "Failed to redo")
        } else {
            ("undo", "Failed to undo")
        };
        let label_arg = label.to_string();
        let steps_arg = steps.to_vec();
        self.mutate(
            repo,
            MutationConfig::new(operation, prefix, json!([path_text(repo), label]))
                .capture(RefProbe::head()),
            move |repo| {
                Ok(mergeon_git_ops::undo::undo_operation(
                    repo, &label_arg, &steps_arg, redo,
                ))
            },
        )
    }

    /// Canal `git:commit`.
    pub fn commit(
        &self,
        repo_path: impl AsRef<Path>,
        message: &str,
        options: &CommitOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let message_arg = message.to_string();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "commit",
                "Failed to commit",
                json!([path_text(repo), message, options_json(options)]),
            )
            .capture(
                RefProbe::head()
                    .with_ref("MERGE_HEAD")
                    .with_ref("CHERRY_PICK_HEAD")
                    .with_ref("REVERT_HEAD"),
            ),
            move |repo| {
                Ok(mergeon_git_ops::commit_operation(
                    repo,
                    &message_arg,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:merge`.
    pub fn merge(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
        options: &MergeOptions,
    ) -> Mutation<OperationResult> {
        let repo = repo_path.as_ref();
        let branch_arg = branch_name.to_string();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "merge",
                "Failed to merge",
                json!([path_text(repo), branch_name, options_json(options)]),
            )
            .capture(sequence_probe(options.target_branch.as_deref()))
            .structured_failure()
            .extra(sequence_extra(or_null(
                options.target_branch.as_deref().unwrap_or_default(),
            ))),
            move |repo| {
                Ok(mergeon_git_ops::merge_operation(
                    repo,
                    &branch_arg,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:rebase`.
    pub fn rebase(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: &str,
        options: &RebaseOptions,
    ) -> Mutation<OperationResult> {
        let repo = repo_path.as_ref();
        let branch_arg = branch_name.to_string();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "rebase",
                "Failed to rebase",
                json!([path_text(repo), branch_name, options_json(options)]),
            )
            .capture(sequence_probe(options.target_branch.as_deref()))
            .structured_failure()
            .extra(sequence_extra(or_null(
                options.target_branch.as_deref().unwrap_or_default(),
            ))),
            move |repo| {
                Ok(mergeon_git_ops::rebase_operation(
                    repo,
                    &branch_arg,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:stash`.
    pub fn stash(
        &self,
        repo_path: impl AsRef<Path>,
        message: Option<&str>,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let message_arg = message.map(str::to_string);
        self.mutate(
            repo,
            MutationConfig::new(
                "stash",
                "Failed to stash changes",
                json!([path_text(repo), optional(message)]),
            )
            .capture(RefProbe::head().with_ref("refs/stash")),
            move |repo| {
                Ok(mergeon_git_ops::stash_operation(
                    repo,
                    message_arg.as_deref(),
                ))
            },
        )
    }

    /// `git stash push` con opciones (rutas, lo que no se sigue, conservar el
    /// índice, solo lo preparado). Va por el mismo canal `git:stash`: la
    /// mutación se identifica por sus opciones.
    pub fn stash_with_options(
        &self,
        repo_path: impl AsRef<Path>,
        options: &StashOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "stash",
                "Failed to stash changes",
                json!([path_text(repo), options_json(options)]),
            )
            .capture(RefProbe::head().with_ref("refs/stash")),
            move |repo| Ok(mergeon_git_ops::stash_push_operation(repo, &options_arg)),
        )
    }

    /// Canal `git:applyStash`.
    pub fn apply_stash(
        &self,
        repo_path: impl AsRef<Path>,
        stash_ref: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let stash_arg = stash_ref.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "apply-stash",
                "Failed to apply stash",
                json!([path_text(repo), stash_ref]),
            )
            .capture(RefProbe::head().with_ref(stash_ref))
            .extra(json!({ "stashRef": or_null(stash_ref) })),
            move |repo| Ok(mergeon_git_ops::apply_stash_operation(repo, &stash_arg)),
        )
    }

    /// Canal `git:popStash`.
    pub fn pop_stash(
        &self,
        repo_path: impl AsRef<Path>,
        stash_ref: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let stash_arg = stash_ref.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "pop-stash",
                "Failed to pop stash",
                json!([path_text(repo), stash_ref]),
            )
            .capture(RefProbe::head().with_ref(stash_ref))
            .extra(json!({ "stashRef": or_null(stash_ref) })),
            move |repo| Ok(mergeon_git_ops::pop_stash_operation(repo, &stash_arg)),
        )
    }

    /// Canal `git:dropStash`.
    pub fn drop_stash(
        &self,
        repo_path: impl AsRef<Path>,
        stash_ref: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let stash_arg = stash_ref.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "drop-stash",
                "Failed to drop stash",
                json!([path_text(repo), stash_ref]),
            )
            .capture(RefProbe::head().with_ref(stash_ref))
            .extra(json!({ "stashRef": or_null(stash_ref) })),
            move |repo| Ok(mergeon_git_ops::drop_stash_operation(repo, &stash_arg)),
        )
    }

    /// Canal `git:stashBranch`.
    pub fn stash_branch(
        &self,
        repo_path: impl AsRef<Path>,
        stash_ref: &str,
        branch_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (stash_arg, branch_arg) = (stash_ref.to_string(), branch_name.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "stash-branch",
                "Failed to create a branch from stash",
                json!([path_text(repo), stash_ref, branch_name]),
            )
            .extra(json!({
                "stashRef": or_null(stash_ref),
                "branch": or_null(branch_name),
            })),
            move |repo| {
                Ok(mergeon_git_ops::stash_branch_operation(
                    repo,
                    &stash_arg,
                    &branch_arg,
                ))
            },
        )
    }

    /// Canal `git:createTag`. Sin `commit` se etiqueta `HEAD`; con `message`
    /// el tag es anotado.
    pub fn create_tag(
        &self,
        repo_path: impl AsRef<Path>,
        tag_name: &str,
        commit: Option<&str>,
        message: Option<&str>,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let tag_arg = tag_name.to_string();
        let commit_arg = commit.map(str::to_string);
        let message_arg = message.map(str::to_string);
        self.mutate(
            repo,
            MutationConfig::new(
                "create-tag",
                "Failed to create tag",
                json!([
                    path_text(repo),
                    tag_name,
                    optional(commit),
                    optional(message)
                ]),
            )
            .capture(RefProbe::head().with_ref(format!("refs/tags/{tag_name}")))
            .extra(json!({ "tag": or_null(tag_name) })),
            move |repo| {
                Ok(mergeon_git_ops::create_tag_operation(
                    repo,
                    &tag_arg,
                    commit_arg.as_deref(),
                    message_arg.as_deref(),
                ))
            },
        )
    }

    /// Canal `git:deleteTag`.
    pub fn delete_tag(
        &self,
        repo_path: impl AsRef<Path>,
        tag_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let tag_arg = tag_name.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "delete-tag",
                "Failed to delete tag",
                json!([path_text(repo), tag_name]),
            )
            .capture(RefProbe::head().with_ref(format!("refs/tags/{tag_name}")))
            .extra(json!({ "tag": or_null(tag_name) })),
            move |repo| Ok(mergeon_git_ops::delete_tag_operation(repo, &tag_arg)),
        )
    }

    // Publicar un tag y quitarlo del remoto: hasta ahora un tag creado aquí no
    // salía nunca de la máquina.

    /// Canal `git:pushTag`.
    pub fn push_tag(
        &self,
        repo_path: impl AsRef<Path>,
        tag_name: &str,
        options: &RemoteTagOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let tag_arg = tag_name.to_string();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "push-tag",
                "Failed to push tag",
                json!([path_text(repo), tag_name, options_json(options)]),
            )
            .timeout(NETWORK_TIMEOUT)
            .extra(json!({ "tag": or_null(tag_name) })),
            move |repo| {
                Ok(mergeon_git_ops::push_tag_operation(
                    repo,
                    &tag_arg,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:deleteRemoteTag`.
    pub fn delete_remote_tag(
        &self,
        repo_path: impl AsRef<Path>,
        tag_name: &str,
        options: &RemoteTagOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let tag_arg = tag_name.to_string();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "delete-remote-tag",
                "Failed to delete the tag on the remote",
                json!([path_text(repo), tag_name, options_json(options)]),
            )
            .timeout(NETWORK_TIMEOUT)
            .extra(json!({ "tag": or_null(tag_name) })),
            move |repo| {
                Ok(mergeon_git_ops::delete_remote_tag_operation(
                    repo,
                    &tag_arg,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:checkoutTag`.
    pub fn checkout_tag(
        &self,
        repo_path: impl AsRef<Path>,
        tag_name: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let tag_arg = tag_name.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "checkout-tag",
                "Failed to checkout tag",
                json!([path_text(repo), tag_name]),
            )
            .capture(RefProbe::head())
            .extra(json!({ "tag": or_null(tag_name) })),
            move |repo| Ok(mergeon_git_ops::checkout_tag_operation(repo, &tag_arg)),
        )
    }

    /// Canal `git:reset`. `mode` es `soft`, `mixed` (el valor por defecto del
    /// original) o `hard`.
    pub fn reset(
        &self,
        repo_path: impl AsRef<Path>,
        commit: &str,
        mode: &str,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (commit_arg, mode_arg) = (commit.to_string(), mode.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "reset",
                "Failed to reset",
                json!([path_text(repo), commit, mode]),
            )
            .capture(RefProbe::head())
            .structured_failure()
            .extra(json!({
                "status": null,
                "operationState": null,
                "conflictState": null,
                "branch": null,
                "remote": null,
                "tag": null,
                "stashRef": null,
                "commit": or_null(commit),
                "mode": mode,
                "filePath": null,
                "filesAffected": [],
            })),
            move |repo| {
                Ok(mergeon_git_ops::reset_operation(
                    repo,
                    &commit_arg,
                    &mode_arg,
                ))
            },
        )
    }

    /// Canal `git:revert`.
    pub fn revert(&self, repo_path: impl AsRef<Path>, commit: &str) -> Mutation<OperationResult> {
        let repo = repo_path.as_ref();
        let commit_arg = commit.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "revert",
                "Failed to revert",
                json!([path_text(repo), commit]),
            )
            .capture(RefProbe::head())
            .structured_failure()
            .extra(sequence_extra(Value::Null)),
            move |repo| Ok(mergeon_git_ops::revert_operation(repo, &commit_arg)),
        )
    }

    /// Canal `git:cherryPick`.
    pub fn cherry_pick(
        &self,
        repo_path: impl AsRef<Path>,
        commit: &str,
    ) -> Mutation<OperationResult> {
        let repo = repo_path.as_ref();
        let commit_arg = commit.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "cherry-pick",
                "Failed to cherry-pick",
                json!([path_text(repo), commit]),
            )
            .capture(RefProbe::head())
            .structured_failure()
            .extra(sequence_extra(Value::Null)),
            move |repo| Ok(mergeon_git_ops::cherry_pick_operation(repo, &commit_arg)),
        )
    }

    /// Cherry-pick de varios commits (del más viejo al más nuevo) con un solo
    /// `git cherry-pick a b c`. Va por el canal `git:cherryPick`: la mutación
    /// se identifica por sus commits.
    pub fn cherry_pick_commits(
        &self,
        repo_path: impl AsRef<Path>,
        commits: &[String],
    ) -> Mutation<OperationResult> {
        let repo = repo_path.as_ref();
        let commits_arg = commits.to_vec();
        self.mutate(
            repo,
            MutationConfig::new(
                "cherry-pick",
                "Failed to cherry-pick",
                json!([path_text(repo), commits]),
            )
            .capture(RefProbe::head())
            .structured_failure()
            .extra(sequence_extra(Value::Null)),
            move |repo| {
                Ok(mergeon_git_ops::cherry_pick_commits_operation(
                    repo,
                    &commits_arg,
                ))
            },
        )
    }

    /// Revert de varios commits (del más nuevo al más viejo); con
    /// `single_commit`, en un único commit de revert. Canal `git:revert`.
    pub fn revert_commits(
        &self,
        repo_path: impl AsRef<Path>,
        commits: &[String],
        single_commit: bool,
    ) -> Mutation<OperationResult> {
        let repo = repo_path.as_ref();
        let commits_arg = commits.to_vec();
        self.mutate(
            repo,
            MutationConfig::new(
                "revert",
                "Failed to revert",
                json!([path_text(repo), commits, { "singleCommit": single_commit }]),
            )
            .capture(RefProbe::head())
            .structured_failure()
            .extra(sequence_extra(Value::Null)),
            move |repo| {
                Ok(mergeon_git_ops::revert_commits_operation(
                    repo,
                    &commits_arg,
                    single_commit,
                ))
            },
        )
    }

    // Worktrees (sin canal en el original).

    /// `git worktree add` con una rama existente o nueva.
    pub fn add_worktree(
        &self,
        repo_path: impl AsRef<Path>,
        options: &mergeon_git_ops::AddWorktreeOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "worktree-add",
                "Failed to create the worktree",
                json!([path_text(repo), options]),
            )
            .extra(json!({ "filePath": or_null(&options.path) })),
            move |repo| Ok(mergeon_git_ops::add_worktree_operation(repo, &options_arg)),
        )
    }

    /// `git worktree remove [--force]`.
    pub fn remove_worktree(
        &self,
        repo_path: impl AsRef<Path>,
        path: &str,
        force: bool,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let path_arg = path.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "worktree-remove",
                "Failed to remove the worktree",
                json!([path_text(repo), path, { "force": force }]),
            )
            .extra(json!({ "filePath": or_null(path) })),
            move |repo| {
                Ok(mergeon_git_ops::remove_worktree_operation(
                    repo, &path_arg, force,
                ))
            },
        )
    }

    /// `git worktree lock` (`lock = true`, con motivo opcional) o `unlock`.
    pub fn set_worktree_locked(
        &self,
        repo_path: impl AsRef<Path>,
        path: &str,
        lock: bool,
        reason: Option<&str>,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let path_arg = path.to_string();
        let reason_arg = reason.map(str::to_string);
        let (channel, failure) = if lock {
            ("worktree-lock", "Failed to lock the worktree")
        } else {
            ("worktree-unlock", "Failed to unlock the worktree")
        };
        self.mutate(
            repo,
            MutationConfig::new(channel, failure, json!([path_text(repo), path, reason]))
                .extra(json!({ "filePath": or_null(path) })),
            move |repo| {
                Ok(if lock {
                    mergeon_git_ops::lock_worktree_operation(repo, &path_arg, reason_arg.as_deref())
                } else {
                    mergeon_git_ops::unlock_worktree_operation(repo, &path_arg)
                })
            },
        )
    }

    /// `git worktree prune`.
    pub fn prune_worktrees(&self, repo_path: impl AsRef<Path>) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        self.mutate(
            repo,
            MutationConfig::new(
                "worktree-prune",
                "Failed to prune worktrees",
                json!([path_text(repo)]),
            ),
            move |repo| Ok(mergeon_git_ops::prune_worktrees_operation(repo)),
        )
    }

    /// Canal `git:resolveConflict`: escribe el contenido resuelto del archivo
    /// y lo marca como resuelto.
    pub fn resolve_conflict(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
        content: &str,
    ) -> Mutation<ResolveConflictResult> {
        let repo = repo_path.as_ref();
        let (file_arg, content_arg) = (file_path.to_string(), content.to_string());
        self.mutate(
            repo,
            MutationConfig::new(
                "resolve-conflict",
                "Failed to resolve conflict",
                json!([path_text(repo), file_path, content]),
            )
            .structured_failure()
            .extra(json!({
                "filePath": file_path,
                "status": null,
                "operationState": null,
                "conflictState": null,
            })),
            move |repo| {
                Ok(mergeon_git_ops::resolve_conflict_operation(
                    repo,
                    &normalize_repository_file_path(repo, &file_arg)?,
                    &content_arg,
                ))
            },
        )
    }

    /// [`Self::resolve_conflict`] con cualquier forma de resolución: el
    /// contenido, una de las versiones entera (binarios, borrado/modificación)
    /// o borrar el archivo. En el registro de operaciones queda la forma, no
    /// el contenido (puede pesar megas).
    pub fn resolve_conflict_with(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
        resolution: &ConflictResolution,
    ) -> Mutation<ResolveConflictResult> {
        let repo = repo_path.as_ref();
        let summary = match resolution {
            ConflictResolution::Content(content) => {
                json!({ "kind": "content", "bytes": content.len() })
            }
            ConflictResolution::Keep(side) => json!({ "kind": "keep", "value": side }),
            ConflictResolution::Delete => json!({ "kind": "delete" }),
        };
        let (file_arg, resolution_arg) = (file_path.to_string(), resolution.clone());
        self.mutate(
            repo,
            MutationConfig::new(
                "resolve-conflict",
                "Failed to resolve conflict",
                json!([path_text(repo), file_path, summary]),
            )
            .structured_failure()
            .extra(json!({
                "filePath": file_path,
                "status": null,
                "operationState": null,
                "conflictState": null,
            })),
            move |repo| {
                Ok(mergeon_git_ops::resolve_conflict_with_operation(
                    repo,
                    &normalize_repository_file_path(repo, &file_arg)?,
                    &resolution_arg,
                ))
            },
        )
    }

    /// Canal `git:interactiveRebase`.
    pub fn interactive_rebase(
        &self,
        repo_path: impl AsRef<Path>,
        options: &InteractiveRebaseOptions,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let options_arg = options.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "interactive-rebase",
                "Interactive rebase failed",
                json!([path_text(repo), options_json(options)]),
            )
            .capture(RefProbe::head())
            .timeout(INTERACTIVE_REBASE_TIMEOUT),
            move |repo| {
                Ok(mergeon_git_ops::interactive_rebase_operation(
                    repo,
                    &options_arg,
                ))
            },
        )
    }

    /// Canal `git:rebaseAction`: reword, drop o squash de un commit.
    pub fn rebase_action(
        &self,
        repo_path: impl AsRef<Path>,
        action: &str,
        commit_hash: &str,
        params: &RebaseActionParams,
    ) -> Mutation<MutationResult> {
        let repo = repo_path.as_ref();
        let (action_arg, commit_arg) = (action.to_string(), commit_hash.to_string());
        let params_arg = params.clone();
        self.mutate(
            repo,
            MutationConfig::new(
                "rebase-action",
                "Rebase action failed",
                json!([path_text(repo), action, commit_hash, options_json(params)]),
            )
            .capture(RefProbe::head())
            .extra(json!({
                "action": action,
                "commit": or_null(commit_hash),
            })),
            move |repo| {
                Ok(mergeon_git_ops::rebase_action_operation(
                    repo,
                    &action_arg,
                    &commit_arg,
                    &params_arg,
                ))
            },
        )
    }

    /// Canal `git:sequenceAction`: `continue`, `abort` o `skip` de la
    /// operación en curso.
    pub fn sequence_action(
        &self,
        repo_path: impl AsRef<Path>,
        action: &str,
    ) -> Mutation<SequenceActionResult> {
        let repo = repo_path.as_ref();
        let action_arg = action.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "sequence-action",
                format!("Failed to {action} Git operation"),
                json!([path_text(repo), action]),
            )
            .capture(RefProbe::head())
            .structured_failure()
            .extra(json!({
                "action": action,
                "status": null,
                "operationState": null,
                "conflictState": null,
            })),
            move |repo| {
                Ok(mergeon_git_ops::sequence_action_operation(
                    repo,
                    &action_arg,
                ))
            },
        )
    }
}
