//! Flujos compartidos con manejo de cambios locales:
//! `src/composables/useCheckoutFlow.js` y `src/composables/useCreateBranchFlow.js`.
//!
//! Los dos ejecutan solo el núcleo y devuelven un estado (`completed` |
//! `failed` | `aborted`) con lo que quien llama necesita para decidir qué
//! releer: el single-flight y el refresco posterior son distintos en cada
//! pantalla y quedan fuera.
//!
//! En el original los composables recibían `getRepoPath` y `applyResult`; en
//! los tres sitios donde se usaban eran "el repositorio de la vista" y
//! "aplicar el resultado a ese repositorio". Aquí se pasa el repositorio.

use gpui_kit::{Context, Task};
use mergeon_backend::Outcome;
use mergeon_model::dialogs::ConfirmOptions;
use mergeon_model::repositories::RepoKey;

use crate::AppModel;
use crate::git_client::GitMutation;
use crate::tasks::flow;

/// Qué hacer con los cambios sin commitear antes de cambiar de rama.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LocalChangesMode {
    /// Dejarlos como están (git decide si el cambio de rama es posible).
    #[default]
    Keep,
    /// Descartarlos.
    Discard,
    /// Guardarlos en un stash y devolverlos después.
    Stash,
}

impl LocalChangesMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LocalChangesMode::Keep => "keep",
            LocalChangesMode::Discard => "discard",
            LocalChangesMode::Stash => "stash",
        }
    }
}

/// Cómo terminó un flujo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowStatus {
    Completed,
    Failed,
    /// No llegó a intentarse (se canceló o falló un paso previo).
    Aborted,
}

impl FlowStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FlowStatus::Completed => "completed",
            FlowStatus::Failed => "failed",
            FlowStatus::Aborted => "aborted",
        }
    }
}

/// A qué se hace checkout (el `checkoutCall` del original).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutTarget {
    /// Una rama local, un commit o cualquier ref: `checkout`.
    Reference(String),
    /// Una rama remota (`origin/feature`), con tracking:
    /// `checkoutRemoteBranch`.
    RemoteBranch(String),
    /// Un tag: `checkoutTag`.
    Tag(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRequest {
    /// Nombre visible del destino; va en el mensaje del stash.
    pub label: String,
    pub target: CheckoutTarget,
    pub local_changes_mode: LocalChangesMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckoutOutcome {
    pub status: FlowStatus,
    /// Resultado del checkout, si llegó a intentarse.
    pub result: Option<GitMutation>,
    pub stashed_changes: bool,
}

impl CheckoutOutcome {
    fn aborted() -> Self {
        Self {
            status: FlowStatus::Aborted,
            result: None,
            stashed_changes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateBranchRequest {
    pub branch_name: String,
    pub checkout_after_create: bool,
    pub local_changes_mode: LocalChangesMode,
    /// Desde dónde nace la rama; `None` es el commit actual.
    pub base_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateBranchOutcome {
    pub status: FlowStatus,
    pub branch_result: Option<GitMutation>,
    pub stashed_changes: bool,
    pub pop_result: Option<GitMutation>,
}

impl CreateBranchOutcome {
    fn aborted() -> Self {
        Self {
            status: FlowStatus::Aborted,
            branch_result: None,
            stashed_changes: false,
            pop_result: None,
        }
    }
}

/// `result?.success || result?.outcome === 'noop'`.
fn succeeded_or_noop(result: &GitMutation) -> bool {
    result.success() || result.outcome() == Some(Outcome::Noop)
}

fn stash_ref_of(result: &GitMutation) -> String {
    result
        .result()
        .and_then(|result| result.stash_ref.clone())
        .filter(|stash_ref| !stash_ref.is_empty())
        .unwrap_or_else(|| "stash@{0}".to_string())
}

impl AppModel {
    /// `runCheckout` de `useCheckoutFlow`: núcleo compartido del checkout con
    /// manejo de cambios locales: descartar o guardar en stash antes de
    /// cambiar, y devolver el stash al working tree después — también si el
    /// checkout falla, para no dejar los cambios escondidos.
    ///
    /// Cada resultado intermedio se aplica al repositorio
    /// (`apply_repository_operation_result`).
    pub fn run_checkout<'a>(
        &mut self,
        repo: impl Into<RepoKey<'a>>,
        request: CheckoutRequest,
        cx: &mut Context<Self>,
    ) -> Task<CheckoutOutcome> {
        let Some(repository) = self.repositories.repository(repo) else {
            return Task::ready(CheckoutOutcome::aborted());
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();

        flow(cx, async move |this, cx| {
            let CheckoutRequest {
                label,
                target,
                local_changes_mode,
            } = request;

            if local_changes_mode == LocalChangesMode::Discard {
                let local_changes = this
                    .update(cx, |this, cx| this.has_uncommitted_changes(&repo_path, cx))
                    .await;
                let local_changes = match local_changes {
                    Ok(local_changes) => local_changes,
                    Err(error) => {
                        this.update(cx, |this, cx| {
                            this.notify_error(error.message(), cx);
                        });
                        return CheckoutOutcome::aborted();
                    }
                };
                let files = local_changes
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect();
                let discard_result = this
                    .update(cx, |this, cx| this.discard_all(&repo_path, Some(files), cx))
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &discard_result, cx);
                });
                if !succeeded_or_noop(&discard_result) {
                    return CheckoutOutcome::aborted();
                }
            }

            let mut stashed_changes = false;
            let mut stash_ref = None;

            if local_changes_mode == LocalChangesMode::Stash {
                let message = format!("WIP: switching to {label}");
                let stash_result = this
                    .update(cx, |this, cx| {
                        this.stash_step(&repo_path, Some(&message), true, cx)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &stash_result, cx);
                });
                if !stash_result.success() {
                    return CheckoutOutcome::aborted();
                }
                stashed_changes = true;
                // Guardamos el stash concreto que creamos: el índice 0 puede correrse.
                stash_ref = Some(stash_ref_of(&stash_result));
            }

            let result = this
                .update(cx, |this, cx| match &target {
                    CheckoutTarget::Reference(reference) => {
                        this.checkout(&repo_path, reference, cx)
                    }
                    CheckoutTarget::RemoteBranch(remote_branch) => {
                        this.checkout_remote_branch(&repo_path, remote_branch, cx)
                    }
                    CheckoutTarget::Tag(tag_name) => this.checkout_tag(&repo_path, tag_name, cx),
                })
                .await;
            this.update(cx, |this, cx| {
                this.apply_repository_operation_result(repo_id.as_str(), &result, cx);
            });
            let succeeded = succeeded_or_noop(&result);

            if let Some(stash_ref) = stash_ref {
                let pop_result = this
                    .update(cx, |this, cx| {
                        this.pop_stash_step(&repo_path, &stash_ref, true, cx)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &pop_result, cx);
                    if !succeeded_or_noop(&pop_result) {
                        this.notify_warning(
                            if succeeded {
                                "Your stashed changes could not be restored automatically. They are still in the stash list."
                            } else {
                                "The checkout failed and your stashed changes could not be restored automatically. They are still in the stash list."
                            },
                            cx,
                        );
                    }
                });
            }

            CheckoutOutcome {
                status: if succeeded {
                    FlowStatus::Completed
                } else {
                    FlowStatus::Failed
                },
                result: Some(result),
                stashed_changes,
            }
        })
    }

    /// `submitCreateBranch` de `useCreateBranchFlow`: flujo compartido de
    /// "crear rama" con manejo de cambios locales.
    ///
    /// Siempre apunta al stash concreto que él mismo creó (el SHA que devuelve
    /// la operación de stash) y no a `stash@{0}` a ciegas, que puede ser otro.
    pub fn submit_create_branch<'a>(
        &mut self,
        repo: impl Into<RepoKey<'a>>,
        request: CreateBranchRequest,
        cx: &mut Context<Self>,
    ) -> Task<CreateBranchOutcome> {
        let Some(repository) = self.repositories.repository(repo) else {
            return Task::ready(CreateBranchOutcome::aborted());
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();

        flow(cx, async move |this, cx| {
            let CreateBranchRequest {
                branch_name,
                checkout_after_create,
                local_changes_mode,
                base_reference,
            } = request;

            let local_changes = this
                .update(cx, |this, cx| this.has_uncommitted_changes(&repo_path, cx))
                .await;
            let local_changes = match local_changes {
                Ok(local_changes) => local_changes,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        let message = error.message();
                        this.notify_error(
                            if message.is_empty() {
                                "Failed to inspect local changes"
                            } else {
                                message
                            },
                            cx,
                        );
                    });
                    return CreateBranchOutcome::aborted();
                }
            };

            let local_files: Vec<String> = local_changes
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect();
            let should_handle_local_changes = checkout_after_create && local_changes.has_changes;
            let mut stash_ref = None;

            if should_handle_local_changes && local_changes_mode == LocalChangesMode::Discard {
                let count = local_files.len();
                let confirmed = this
                    .update(cx, |this, cx| {
                        this.confirm(
                            ConfirmOptions {
                                title: Some("Discard local changes".to_string()),
                                description: Some(format!(
                                    "Discard {count} local change{} before creating '{branch_name}'?",
                                    if count != 1 { "s" } else { "" }
                                )),
                                confirm_text: Some("Discard".to_string()),
                                cancel_text: Some("Cancel".to_string()),
                                variant: Some("destructive".to_string()),
                                ..Default::default()
                            },
                            cx,
                        )
                    })
                    .await;

                if !confirmed {
                    return CreateBranchOutcome::aborted();
                }

                let discard_result = this
                    .update(cx, |this, cx| {
                        this.discard_all(&repo_path, Some(local_files.clone()), cx)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &discard_result, cx);
                });
                if !succeeded_or_noop(&discard_result) {
                    return CreateBranchOutcome::aborted();
                }
            }

            if should_handle_local_changes && local_changes_mode == LocalChangesMode::Stash {
                let message = format!("WIP: before creating {branch_name}");
                let stash_result = this
                    .update(cx, |this, cx| {
                        this.stash_step(&repo_path, Some(&message), true, cx)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &stash_result, cx);
                });
                if !stash_result.success() {
                    return CreateBranchOutcome::aborted();
                }
                stash_ref = Some(stash_ref_of(&stash_result));
            }
            let stashed_changes = stash_ref.is_some();

            let branch_result = this
                .update(cx, |this, cx| {
                    this.create_branch(
                        &repo_path,
                        &branch_name,
                        base_reference.as_deref(),
                        checkout_after_create,
                        cx,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.apply_repository_operation_result(repo_id.as_str(), &branch_result, cx);
            });

            let branch_succeeded = succeeded_or_noop(&branch_result);
            let mut pop_result = None;

            if let Some(stash_ref) = &stash_ref
                && (!branch_succeeded || checkout_after_create)
            {
                // Si la rama no se creó esto es el rollback: devolvemos los
                // cambios al working tree.
                let popped = this
                    .update(cx, |this, cx| {
                        this.pop_stash_step(&repo_path, stash_ref, true, cx)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &popped, cx);
                    if !succeeded_or_noop(&popped) {
                        if branch_succeeded {
                            this.notify_warning(
                                "The branch was created but your stashed changes could not be restored automatically. They are still in the stash list.",
                                cx,
                            );
                        } else {
                            this.notify_error(
                                "The branch was not created and your stashed changes could not be restored automatically. They are still in the stash list.",
                                cx,
                            );
                        }
                    }
                });
                pop_result = Some(popped);
            }

            CreateBranchOutcome {
                status: if branch_succeeded {
                    FlowStatus::Completed
                } else {
                    FlowStatus::Failed
                },
                branch_result: Some(branch_result),
                stashed_changes,
                pop_result,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_backend::{MutationBody, create_generic_operation_failure};

    // "falls back to stash@{0} when the stash operation returns no ref" de
    // `useCreateBranchFlow.test.js`.
    #[test]
    fn falls_back_to_the_top_stash_when_the_operation_returns_no_ref() {
        let without_result = GitMutation {
            capture: None,
            operation_id: None,
            body: MutationBody::Failed(create_generic_operation_failure(
                "stash",
                "Failed to stash",
                "boom",
                Default::default(),
            )),
        };

        assert_eq!(stash_ref_of(&without_result), "stash@{0}");
        assert!(!succeeded_or_noop(&without_result));
    }
}
