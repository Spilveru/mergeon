//! Los flujos del sidebar que no necesitan ventana: confirmar o pedir un dato,
//! lanzar la operación git, aplicar su resultado al repositorio y releer solo
//! las secciones que pudo cambiar. Son los `handle*` de `Sidebar.vue`.
//!
//! Corren sobre el modelo, no sobre la vista: como las promesas del original,
//! llegan al final aunque el sidebar se desmonte a mitad.
//!
//! A diferencia del original, todo flujo trabaja sobre el repositorio con el
//! que empezó: allí `applyOperationResult` y `refreshSections` leían
//! `activeRepo` en cada paso, y un cambio de pestaña a mitad de una operación
//! aplicaba el resultado al repositorio equivocado.

use gpui_kit::{App, AsyncApp, Entity, Task};
use mergeon_app::{
    AddRepository, AppModel, CheckoutRequest, CheckoutTarget, CreateBranchRequest, FlowStatus,
    GitMutation, LocalChangesMode, StructuredOutcome, SyncRequest,
};
use mergeon_backend::{
    CleanupBranchesOptions, Outcome, PullOptions, PushOptions, RemoteTagOptions,
    SubmoduleUpdateOptions,
};
use mergeon_model::dialogs::{ConfirmAnswer, ConfirmOptions, PromptOptions};

use crate::logic::UpstreamReference;

pub(crate) type Model = Entity<AppModel>;

/// El repositorio sobre el que trabaja un flujo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoRef {
    pub id: String,
    pub path: String,
}

/// `runBranchOperation`: una operación por repositorio y nombre; mientras
/// corre, pedirla otra vez espera a la misma.
pub(crate) fn single_flight<F>(
    model: &Model,
    repo: &RepoRef,
    operation: &str,
    cx: &mut App,
    work: F,
) -> Task<()>
where
    F: AsyncFnOnce(Model, &mut AsyncApp) + 'static,
{
    let handle = model.clone();
    model.update(cx, |model, cx| {
        model.run_repo_operation_single_flight(&repo.id, operation, cx, move |_, cx| {
            cx.spawn(async move |_, cx| work(handle, cx).await)
        })
    })
}

/// `applyOperationResult`.
pub(crate) fn apply(
    model: &Model,
    repo: &RepoRef,
    result: &impl StructuredOutcome,
    cx: &mut AsyncApp,
) {
    model.update(cx, |model, cx| {
        model.apply_repository_operation_result(repo.id.as_str(), result, cx);
    });
}

/// `{ result, operation: result?.operation }` sobre lo que se pide releer.
pub(crate) fn with_result(request: SyncRequest, result: &impl StructuredOutcome) -> SyncRequest {
    let summary = result.to_operation_result();
    let operation = summary.operation.clone().filter(|name| !name.is_empty());
    let request = request.result(summary);
    match operation {
        Some(operation) => request.operation(operation),
        None => request,
    }
}

/// `refreshSections`.
pub(crate) async fn refresh(
    model: &Model,
    repo: &RepoRef,
    request: SyncRequest,
    cx: &mut AsyncApp,
) {
    model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(repo.id.as_str(), request, cx)
        })
        .await;
}

/// `ensureSidebarDataLoaded`.
pub(crate) fn ensure_loaded(model: &Model, repo_id: &str, request: SyncRequest, cx: &mut App) {
    model
        .update(cx, |model, cx| {
            model.ensure_repository_sections_loaded(repo_id, request, cx)
        })
        .detach();
}

fn auth_token(model: &Model, cx: &mut AsyncApp) -> Option<String> {
    model.read_with(cx, |model, _| model.git_auth_token())
}

fn succeeded_or_noop(result: &GitMutation) -> bool {
    result.success() || result.outcome() == Some(Outcome::Noop)
}

fn succeeded_or_conflicted(result: &GitMutation) -> bool {
    result.success() || result.outcome() == Some(Outcome::Conflicted)
}

pub(crate) fn confirm_options(
    title: &str,
    description: String,
    confirm_text: &str,
    cancel_text: &str,
    destructive: bool,
) -> ConfirmOptions {
    ConfirmOptions {
        title: Some(title.to_string()),
        description: Some(description),
        confirm_text: Some(confirm_text.to_string()),
        cancel_text: Some(cancel_text.to_string()),
        variant: destructive.then(|| "destructive".to_string()),
        ..Default::default()
    }
}

async fn confirm(model: &Model, options: ConfirmOptions, cx: &mut AsyncApp) -> bool {
    model
        .update(cx, |model, cx| model.confirm(options, cx))
        .await
}

/// `prompt(...)`: el texto confirmado, ya sin espacios alrededor. `None` si se
/// canceló o quedó vacío.
async fn prompt(model: &Model, options: PromptOptions, cx: &mut AsyncApp) -> Option<String> {
    let answer = model
        .update(cx, |model, cx| model.prompt(options, cx))
        .await;
    if !answer.confirmed {
        return None;
    }
    answer
        .value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `/\S+/` de los prompts de nombre.
const NOT_BLANK: &str = r"\S+";

// ---------------------------------------------------------------------------
// Checkout
// ---------------------------------------------------------------------------

/// `executeCheckoutFlow`: el núcleo (descartar o guardar en stash, checkout y
/// devolver el stash) vive en `AppModel::run_checkout`; aquí queda el
/// single-flight y qué secciones hay que releer.
pub(crate) fn checkout(
    model: &Model,
    repo: RepoRef,
    label: String,
    target: CheckoutTarget,
    local_changes_mode: LocalChangesMode,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "checkout",
        cx,
        async move |model, cx| {
            let outcome = model
                .update(cx, |model, cx| {
                    model.run_checkout(
                        repo.id.as_str(),
                        CheckoutRequest {
                            label,
                            target,
                            local_changes_mode,
                        },
                        cx,
                    )
                })
                .await;
            if outcome.status == FlowStatus::Aborted {
                return;
            }

            let needs_status = outcome
                .result
                .as_ref()
                .is_none_or(|result| result.status().is_none())
                || local_changes_mode != LocalChangesMode::Keep;
            let mut request = SyncRequest::new()
                .branches(true)
                .status(needs_status)
                .changes_count(needs_status)
                .stashes(outcome.stashed_changes);
            if let Some(result) = &outcome.result {
                request = with_result(request, result);
            }
            refresh(&model, &repo, request, cx).await;
        },
    )
}

// ---------------------------------------------------------------------------
// Ramas destacadas
// ---------------------------------------------------------------------------

/// `toggleStarBranch`.
pub(crate) async fn toggle_star_branch(
    model: Model,
    repo: RepoRef,
    branch_name: String,
    cx: &mut AsyncApp,
) {
    let starred = model.read_with(cx, |model, _| {
        model
            .repository(repo.id.as_str())
            .is_some_and(|repository| repository.starred_branches.contains(&branch_name))
    });
    let written = model
        .update(cx, |model, cx| {
            if starred {
                model.remove_starred_branch(&repo.path, &branch_name, cx)
            } else {
                model.add_starred_branch(&repo.path, &branch_name, cx)
            }
        })
        .await;

    // Escribir ya deja la lista nueva en el modelo: invalidar y releer la
    // sección la vaciaba un instante ("No starred branches").
    match written {
        Ok(_) => {}
        Err(error) => {
            model.update(cx, |model, cx| {
                model.notify_error_with("Failed to update starred branches", &error, cx);
            });
        }
    }
}

fn is_starred(model: &Model, repo: &RepoRef, branch_name: &str, cx: &mut AsyncApp) -> bool {
    model.read_with(cx, |model, _| {
        model
            .repository(repo.id.as_str())
            .is_some_and(|repository| {
                repository
                    .starred_branches
                    .iter()
                    .any(|starred| starred == branch_name)
            })
    })
}

/// `syncStarredBranchRename`: la marca de destacada sigue a la rama renombrada.
async fn sync_starred_branch_rename(
    model: &Model,
    repo: &RepoRef,
    old_name: &str,
    next_name: &str,
    cx: &mut AsyncApp,
) {
    if !is_starred(model, repo, old_name, cx) {
        return;
    }
    let _ = model
        .update(cx, |model, cx| {
            model.remove_starred_branch(&repo.path, old_name, cx)
        })
        .await;
    let _ = model
        .update(cx, |model, cx| {
            model.add_starred_branch(&repo.path, next_name, cx)
        })
        .await;
}

/// `syncStarredBranchDeletion`.
async fn sync_starred_branch_deletion(
    model: &Model,
    repo: &RepoRef,
    branch_name: &str,
    cx: &mut AsyncApp,
) {
    if !is_starred(model, repo, branch_name, cx) {
        return;
    }
    let _ = model
        .update(cx, |model, cx| {
            model.remove_starred_branch(&repo.path, branch_name, cx)
        })
        .await;
}

// ---------------------------------------------------------------------------
// Ramas locales
// ---------------------------------------------------------------------------

/// `handleSetupTracking`.
pub(crate) fn setup_tracking(
    model: &Model,
    repo: RepoRef,
    branch_name: String,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "setup-tracking",
        cx,
        async move |model, cx| {
            let result = model
                .update(cx, |model, cx| {
                    model.setup_tracking(&repo.path, Some(&branch_name), cx)
                })
                .await;
            apply(&model, &repo, &result, cx);
            if result.success() && result.outcome() == Some(Outcome::Completed) {
                let request = with_result(SyncRequest::new().branches(true).remotes(true), &result);
                refresh(&model, &repo, request, cx).await;
            }
        },
    )
}

/// `handleRenameBranch`.
pub(crate) fn rename_branch(
    model: &Model,
    repo: RepoRef,
    branch_name: String,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "rename-branch",
        cx,
        async move |model, cx| {
            let Some(next_branch_name) = prompt(
                &model,
                PromptOptions {
                    title: Some("Rename branch".to_string()),
                    description: Some(format!("Rename '{branch_name}' to:")),
                    confirm_text: Some("Rename".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    default_value: Some(branch_name.clone()),
                    pattern: Some(NOT_BLANK.to_string()),
                    input_error_message: Some("Branch name is required.".to_string()),
                    ..Default::default()
                },
                cx,
            )
            .await
            else {
                return;
            };

            let result = model
                .update(cx, |model, cx| {
                    model.rename_branch(&repo.path, &branch_name, &next_branch_name, cx)
                })
                .await;
            apply(&model, &repo, &result, cx);
            if succeeded_or_noop(&result) {
                sync_starred_branch_rename(&model, &repo, &branch_name, &next_branch_name, cx)
                    .await;
                let needs_status = result.status().is_none();
                let request = with_result(
                    SyncRequest::new()
                        .branches(true)
                        .status(needs_status)
                        .changes_count(needs_status)
                        .starred_branches(true),
                    &result,
                );
                refresh(&model, &repo, request, cx).await;
            }
        },
    )
}

/// `handleDeleteBranch`. Si la rama tiene upstream se ofrece borrarlo de paso.
pub(crate) async fn delete_branch(
    model: Model,
    repo: RepoRef,
    branch_name: String,
    upstream: Option<UpstreamReference>,
    cx: &mut AsyncApp,
) {
    let description = format!("Delete '{branch_name}'? Git will refuse if it is not fully merged.");
    let options = confirm_options("Delete branch", description, "Delete", "Cancel", true);

    let delete_upstream = match &upstream {
        Some(upstream) => {
            // Apagada: borrar en el remoto lo notan los demás y no se deshace solo.
            let answer = model
                .update(cx, |model, cx| {
                    model.confirm_with_option(
                        ConfirmOptions {
                            option_label: Some(format!(
                                "Also delete '{}' on {}",
                                upstream.full_name, upstream.remote_name
                            )),
                            ..options
                        },
                        cx,
                    )
                })
                .await;
            match answer {
                ConfirmAnswer::WithOption {
                    confirmed: true,
                    checked,
                } => checked,
                _ => return,
            }
        }
        None => {
            if !confirm(&model, options, cx).await {
                return;
            }
            false
        }
    };

    let flight = cx.update(|cx| {
        single_flight(
            &model,
            &repo.clone(),
            "delete-branch",
            cx,
            async move |model, cx| {
                let result = model
                    .update(cx, |model, cx| {
                        model.delete_branch(&repo.path, &branch_name, cx)
                    })
                    .await;
                apply(&model, &repo, &result, cx);
                if !result.success() {
                    return;
                }

                // Solo se toca el remoto si la local se borró: al revés dejaría la copia
                // local apuntando a algo que ya no existe.
                if let Some(upstream) = upstream.filter(|_| delete_upstream) {
                    let remote_result = model
                        .update(cx, |model, cx| {
                            model.delete_remote_branch(
                                &repo.path,
                                &upstream.remote_name,
                                &upstream.branch_name,
                                cx,
                            )
                        })
                        .await;
                    apply(&model, &repo, &remote_result, cx);
                }

                sync_starred_branch_deletion(&model, &repo, &branch_name, cx).await;
                let needs_status = result.status().is_none();
                let request = with_result(
                    SyncRequest::new()
                        .branches(true)
                        .remotes(delete_upstream)
                        .status(needs_status)
                        .changes_count(needs_status)
                        .starred_branches(true),
                    &result,
                );
                refresh(&model, &repo, request, cx).await;
            },
        )
    });
    flight.await;
}

/// "Clean up merged branches…": borra en lote lo elegido en el diálogo
/// (una sola mutación en la cola, con su aviso y su entrada en el log), quita
/// la marca de destacada de las ramas que ya no existen y relee ramas (y
/// remotos, si se borraron ramas remotas).
pub(crate) fn cleanup_branches(
    model: &Model,
    repo: RepoRef,
    options: CleanupBranchesOptions,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "cleanup-branches",
        cx,
        async move |model, cx| {
            let requested: Vec<String> = options
                .branches
                .iter()
                .map(|branch| branch.name.clone())
                .collect();
            let delete_remote = options.delete_remote;
            let result = model
                .update(cx, |model, cx| {
                    model.cleanup_branches(&repo.path, options, cx)
                })
                .await;
            apply(&model, &repo, &result, cx);

            let needs_status = result.status().is_none();
            let request = with_result(
                SyncRequest::new()
                    .branches(true)
                    .remotes(delete_remote)
                    .status(needs_status)
                    .changes_count(needs_status)
                    .starred_branches(true),
                &result,
            );
            refresh(&model, &repo, request, cx).await;

            // Las destacadas que ya no existen dejan de estarlo.
            let gone: Vec<String> = model.read_with(cx, |model, _| {
                let Some(repository) = model.repository(repo.id.as_str()) else {
                    return Vec::new();
                };
                requested
                    .into_iter()
                    .filter(|name| {
                        repository.starred_branches.contains(name)
                            && !repository
                                .branches
                                .iter()
                                .any(|branch| !branch.is_remote && &branch.name == name)
                    })
                    .collect()
            });
            for name in gone {
                sync_starred_branch_deletion(&model, &repo, &name, cx).await;
            }
        },
    )
}

/// `handleFastForwardBranch`.
pub(crate) fn fast_forward_branch(
    model: &Model,
    repo: RepoRef,
    branch_name: String,
    tracking_reference: String,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "fast-forward-branch",
        cx,
        async move |model, cx| {
            let result = model
                .update(cx, |model, cx| {
                    model.fast_forward_branch(&repo.path, &branch_name, &tracking_reference, cx)
                })
                .await;
            apply(&model, &repo, &result, cx);
            if succeeded_or_noop(&result) {
                let needs_status = result.status().is_none();
                let request = with_result(
                    SyncRequest::new()
                        .branches(true)
                        .status(needs_status)
                        .changes_count(needs_status),
                    &result,
                );
                refresh(&model, &repo, request, cx).await;
            }
        },
    )
}

/// `handlePullBranch` y `handlePullRemoteBranch`.
pub(crate) fn pull(
    model: &Model,
    repo: RepoRef,
    remote_name: String,
    branch_name: String,
    cx: &mut App,
) -> Task<()> {
    single_flight(model, &repo.clone(), "pull", cx, async move |model, cx| {
        let auth_token = auth_token(&model, cx);
        let result = model
            .update(cx, |model, cx| {
                model.pull(
                    &repo.path,
                    PullOptions {
                        remote: Some(remote_name),
                        branch: Some(branch_name),
                        auth_token,
                        ..PullOptions::default()
                    },
                    cx,
                )
            })
            .await;
        // Al registro de operaciones y al estado del repositorio, como el
        // resto de mutaciones (también si falla o se queda con conflictos).
        apply(&model, &repo, &result, cx);
        if succeeded_or_conflicted(&result) || result.outcome() == Some(Outcome::Noop) {
            let request = with_result(
                SyncRequest::new()
                    .branches(true)
                    .remotes(true)
                    .status(true)
                    .changes_count(true),
                &result,
            )
            .operation("pull");
            refresh(&model, &repo, request, cx).await;
        }
        // Un fallo sin estado adjunto ya lo relee `apply`.
    })
}

/// `handlePushBranch`. `target_branch` es la rama del remoto cuando no se
/// llama como la local (`local` sigue a `origin/otro-nombre`).
pub(crate) fn push_branch(
    model: &Model,
    repo: RepoRef,
    remote_name: String,
    branch_name: String,
    target_branch: Option<String>,
    cx: &mut App,
) -> Task<()> {
    single_flight(model, &repo.clone(), "push", cx, async move |model, cx| {
        let auth_token = auth_token(&model, cx);
        let target_branch = target_branch.filter(|target| *target != branch_name);
        let result = model
            .update(cx, |model, cx| {
                model.push(
                    &repo.path,
                    PushOptions {
                        remote: remote_name,
                        branch: Some(branch_name),
                        target_branch,
                        auth_token,
                        ..PushOptions::default()
                    },
                    false,
                    cx,
                )
            })
            .await;
        apply(&model, &repo, &result, cx);
        if succeeded_or_noop(&result) {
            // El push mueve `remoto/rama`: también cambia la sección Remotes.
            let request = with_result(
                SyncRequest::new()
                    .branches(true)
                    .remotes(true)
                    .status(true)
                    .changes_count(true),
                &result,
            );
            refresh(&model, &repo, request, cx).await;
        }
    })
}

/// `handleCreateBranchSubmit`.
pub(crate) fn create_branch(
    model: &Model,
    repo: RepoRef,
    request: CreateBranchRequest,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "create-branch",
        cx,
        async move |model, cx| {
            let checkout_after_create = request.checkout_after_create;
            let local_changes_mode = request.local_changes_mode;
            let flow = model
                .update(cx, |model, cx| {
                    model.submit_create_branch(repo.id.as_str(), request, cx)
                })
                .await;
            if flow.status != FlowStatus::Completed {
                return;
            }

            let reads_status =
                checkout_after_create || local_changes_mode != LocalChangesMode::Keep;
            let mut sync = SyncRequest::new()
                .branches(true)
                .status(reads_status)
                .changes_count(reads_status)
                .stashes(flow.stashed_changes);
            if let Some(result) = &flow.branch_result {
                sync = with_result(sync, result);
            }
            refresh(&model, &repo, sync, cx).await;
        },
    )
}

// ---------------------------------------------------------------------------
// Ramas remotas
// ---------------------------------------------------------------------------

/// `handleDeleteRemoteBranch`.
pub(crate) async fn delete_remote_branch(
    model: Model,
    repo: RepoRef,
    remote_name: String,
    branch_name: String,
    cx: &mut AsyncApp,
) {
    let confirmed = confirm(
        &model,
        confirm_options(
            "Delete remote branch",
            format!("Delete '{remote_name}/{branch_name}' from '{remote_name}'?"),
            "Delete",
            "Cancel",
            true,
        ),
        cx,
    )
    .await;
    if !confirmed {
        return;
    }

    let flight = cx.update(|cx| {
        single_flight(
            &model,
            &repo.clone(),
            "delete-remote-branch",
            cx,
            async move |model, cx| {
                let result = model
                    .update(cx, |model, cx| {
                        model.delete_remote_branch(&repo.path, &remote_name, &branch_name, cx)
                    })
                    .await;
                apply(&model, &repo, &result, cx);
                if result.success() {
                    let request =
                        with_result(SyncRequest::new().branches(true).remotes(true), &result);
                    refresh(&model, &repo, request, cx).await;
                }
            },
        )
    });
    flight.await;
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// El prompt opcional de la anotación; `None` crea un tag ligero.
async fn prompt_tag_annotation(
    model: &Model,
    description: String,
    cx: &mut AsyncApp,
) -> Option<String> {
    prompt(
        model,
        PromptOptions {
            title: Some("Annotate tag (optional)".to_string()),
            description: Some(description),
            confirm_text: Some("Create tag".to_string()),
            cancel_text: Some("Skip".to_string()),
            textarea: true,
            rows: Some(4),
            placeholder: Some("Release notes, context, or why this tag matters".to_string()),
            ..Default::default()
        },
        cx,
    )
    .await
}

/// Crea el tag, relee la sección y ofrece publicarlo.
async fn create_tag_at(
    model: &Model,
    repo: &RepoRef,
    tag_name: &str,
    commit: &str,
    message: Option<String>,
    cx: &mut AsyncApp,
) {
    let result = model
        .update(cx, |model, cx| {
            model.create_tag(&repo.path, tag_name, Some(commit), message.as_deref(), cx)
        })
        .await;
    apply(model, repo, &result, cx);

    if result.success() {
        let request = with_result(SyncRequest::new().tags(true), &result);
        refresh(model, repo, request, cx).await;
        if let Some(remote) = tag_remote_name(model, repo, cx) {
            offer_to_publish_tag(model, repo, tag_name, &remote, cx).await;
        }
    }
}

/// `handleCreateTag`: nombre, anotación opcional y tag sobre `HEAD`.
pub(crate) async fn create_tag(model: Model, repo: RepoRef, cx: &mut AsyncApp) {
    let Some(tag_name) = prompt(
        &model,
        PromptOptions {
            title: Some("Create tag".to_string()),
            description: Some("Enter the new tag name.".to_string()),
            confirm_text: Some("Next".to_string()),
            cancel_text: Some("Cancel".to_string()),
            pattern: Some(NOT_BLANK.to_string()),
            input_error_message: Some("Tag name is required.".to_string()),
            ..Default::default()
        },
        cx,
    )
    .await
    else {
        return;
    };

    let message = prompt_tag_annotation(
        &model,
        format!(
            "Add a message for '{tag_name}' to create an annotated tag. Leave it blank for a lightweight tag."
        ),
        cx,
    )
    .await;
    create_tag_at(&model, &repo, &tag_name, "HEAD", message, cx).await;
}

/// `handleCreateTagFromContext` sobre la rama con checkout.
pub(crate) fn create_tag_from_branch(model: &Model, repo: RepoRef, cx: &mut App) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "create-tag",
        cx,
        async move |model, cx| {
            create_tag(model, repo, cx).await;
        },
    )
}

/// `handleCreateTagFromRemoteContext`.
pub(crate) async fn create_tag_from_remote_branch(
    model: Model,
    repo: RepoRef,
    remote_name: String,
    branch_name: String,
    cx: &mut AsyncApp,
) {
    let reference = format!("{remote_name}/{branch_name}");
    let Some(tag_name) = prompt(
        &model,
        PromptOptions {
            title: Some("Create tag".to_string()),
            description: Some(format!("Create a tag at '{reference}'.")),
            confirm_text: Some("Create".to_string()),
            cancel_text: Some("Cancel".to_string()),
            pattern: Some(NOT_BLANK.to_string()),
            input_error_message: Some("Tag name is required.".to_string()),
            ..Default::default()
        },
        cx,
    )
    .await
    else {
        return;
    };

    let flight = cx.update(|cx| {
        single_flight(&model, &repo.clone(), "create-tag", cx, async move |model, cx| {
            let message = prompt_tag_annotation(
                &model,
                format!(
                    "Add a message for '{tag_name}' to create an annotated tag at '{reference}'. Leave it blank for a lightweight tag."
                ),
                cx,
            )
            .await;
            create_tag_at(&model, &repo, &tag_name, &reference, message, cx).await;
        })
    });
    flight.await;
}

/// Un tag recién creado solo existe aquí. Se ofrece publicarlo en el mismo
/// gesto porque descubrir que nadie lo ve suele pasar mucho después.
async fn offer_to_publish_tag(
    model: &Model,
    repo: &RepoRef,
    tag_name: &str,
    remote: &str,
    cx: &mut AsyncApp,
) {
    let confirmed = confirm(
        model,
        confirm_options(
            "Tag created",
            format!(
                "'{tag_name}' only exists on this machine. Push it to {remote} so others can see it?"
            ),
            &format!("Push to {remote}"),
            "Not now",
            false,
        ),
        cx,
    )
    .await;
    if confirmed {
        push_tag(model.clone(), repo.clone(), tag_name.to_string(), cx).await;
    }
}

/// El remoto de las acciones de tags del repositorio (`origin`, o el único /
/// primero que haya). `None` si no tiene remotos.
fn tag_remote_name(model: &Model, repo: &RepoRef, cx: &mut AsyncApp) -> Option<String> {
    model.read_with(cx, |model, _| {
        model
            .repository(repo.id.as_str())
            .and_then(|repository| crate::logic::tag_remote(&repository.remotes))
            .map(str::to_string)
    })
}

fn remote_tag_options(model: &Model, remote: String, cx: &mut AsyncApp) -> RemoteTagOptions {
    RemoteTagOptions {
        remote,
        auth_token: auth_token(model, cx),
    }
}

/// `handlePushTag`.
pub(crate) async fn push_tag(model: Model, repo: RepoRef, tag_name: String, cx: &mut AsyncApp) {
    let Some(remote) = tag_remote_name(&model, &repo, cx) else {
        return;
    };
    let options = remote_tag_options(&model, remote, cx);
    let result = model
        .update(cx, |model, cx| {
            model.push_tag(&repo.path, &tag_name, options, cx)
        })
        .await;
    apply(&model, &repo, &result, cx);
}

/// `handleDeleteRemoteTag`.
pub(crate) async fn delete_remote_tag(
    model: Model,
    repo: RepoRef,
    tag_name: String,
    cx: &mut AsyncApp,
) {
    let Some(remote) = tag_remote_name(&model, &repo, cx) else {
        return;
    };
    let confirmed = confirm(
        &model,
        confirm_options(
            &format!("Delete tag on {remote}"),
            format!("Delete '{tag_name}' on {remote}? Your local tag stays."),
            &format!("Delete on {remote}"),
            "Cancel",
            true,
        ),
        cx,
    )
    .await;
    if !confirmed {
        return;
    }

    let options = remote_tag_options(&model, remote, cx);
    let result = model
        .update(cx, |model, cx| {
            model.delete_remote_tag(&repo.path, &tag_name, options, cx)
        })
        .await;
    apply(&model, &repo, &result, cx);
}

/// `handleDeleteTag`. Un tag creado aquí puede estar publicado o no, y desde el
/// cliente no se sabe sin preguntar al remoto: por eso se ofrece borrarlo
/// también allí y la casilla empieza apagada.
pub(crate) async fn delete_tag(model: Model, repo: RepoRef, tag_name: String, cx: &mut AsyncApp) {
    let options = confirm_options(
        "Delete tag",
        format!("Delete tag '{tag_name}'?"),
        "Delete",
        "Cancel",
        true,
    );
    // Sin remotos no hay dónde borrarlo también: confirmación simple.
    let remote = tag_remote_name(&model, &repo, cx);
    let checked = match &remote {
        Some(remote) => {
            let answer = model
                .update(cx, |model, cx| {
                    model.confirm_with_option(
                        ConfirmOptions {
                            option_label: Some(format!("Also delete it on {remote}")),
                            ..options
                        },
                        cx,
                    )
                })
                .await;
            let ConfirmAnswer::WithOption {
                confirmed: true,
                checked,
            } = answer
            else {
                return;
            };
            checked
        }
        None => {
            if !confirm(&model, options, cx).await {
                return;
            }
            false
        }
    };

    let result = model
        .update(cx, |model, cx| model.delete_tag(&repo.path, &tag_name, cx))
        .await;
    apply(&model, &repo, &result, cx);
    if !result.success() {
        return;
    }

    if let Some(remote) = remote.filter(|_| checked) {
        let options = remote_tag_options(&model, remote, cx);
        let remote_result = model
            .update(cx, |model, cx| {
                model.delete_remote_tag(&repo.path, &tag_name, options, cx)
            })
            .await;
        apply(&model, &repo, &remote_result, cx);
    }

    let request = with_result(SyncRequest::new().tags(true), &result);
    refresh(&model, &repo, request, cx).await;
}

/// `handleCheckoutTag`.
pub(crate) async fn checkout_tag(model: Model, repo: RepoRef, tag_name: String, cx: &mut AsyncApp) {
    let confirmed = confirm(
        &model,
        confirm_options(
            "Checkout tag",
            format!(
                "Checkout tag '{tag_name}' in detached HEAD state? You will not be on a branch until you create or checkout one again."
            ),
            "Checkout",
            "Cancel",
            false,
        ),
        cx,
    )
    .await;
    if !confirmed {
        return;
    }

    let result = model
        .update(cx, |model, cx| {
            model.checkout_tag(&repo.path, &tag_name, cx)
        })
        .await;
    apply(&model, &repo, &result, cx);

    if result.success() {
        let request = with_result(
            SyncRequest::new()
                .branches(true)
                .changes_count(result.status().is_none()),
            &result,
        );
        refresh(&model, &repo, request, cx).await;
        model.update(cx, |model, cx| {
            model.notify_info(
                &format!(
                    "Detached HEAD at tag '{tag_name}'. Create a branch if you want to keep working from here."
                ),
                cx,
            );
        });
    }
}

// ---------------------------------------------------------------------------
// Remotos
// ---------------------------------------------------------------------------

/// `runRemoteMutation`.
fn remote_mutation(
    model: &Model,
    repo: RepoRef,
    operation: &'static str,
    cx: &mut App,
    task: impl FnOnce(&mut AppModel, &str, &mut gpui_kit::Context<AppModel>) -> Task<GitMutation>
    + 'static,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        operation,
        cx,
        async move |model, cx| {
            let result = model
                .update(cx, |model, cx| task(model, &repo.path, cx))
                .await;
            apply(&model, &repo, &result, cx);
            if succeeded_or_noop(&result) {
                let request = with_result(SyncRequest::new().remotes(true).branches(true), &result);
                refresh(&model, &repo, request, cx).await;
            }
        },
    )
}

/// `handleEditRemoteUrl`.
pub(crate) async fn edit_remote_url(
    model: Model,
    repo: RepoRef,
    remote_name: String,
    cx: &mut AsyncApp,
) {
    let current_url = model.read_with(cx, |model, _| {
        model
            .repository(repo.id.as_str())
            .and_then(|repository| {
                repository
                    .remotes
                    .iter()
                    .find(|remote| remote.name == remote_name)
            })
            .map(|remote| remote.refs.fetch.clone())
            .unwrap_or_default()
    });
    let Some(new_url) = prompt(
        &model,
        PromptOptions {
            title: Some(format!("Edit URL of '{remote_name}'")),
            description: Some("The remote will fetch and push against this URL.".to_string()),
            confirm_text: Some("Save".to_string()),
            cancel_text: Some("Cancel".to_string()),
            placeholder: Some("https://github.com/username/repository.git".to_string()),
            default_value: Some(current_url.clone()),
            ..Default::default()
        },
        cx,
    )
    .await
    else {
        return;
    };
    if new_url == current_url {
        return;
    }

    cx.update(|cx| {
        remote_mutation(
            &model,
            repo,
            "set-remote-url",
            cx,
            move |model, path, cx| model.set_remote_url(path, &remote_name, &new_url, cx),
        )
    })
    .await;
}

/// `handleRenameRemote`.
pub(crate) async fn rename_remote(
    model: Model,
    repo: RepoRef,
    remote_name: String,
    cx: &mut AsyncApp,
) {
    let Some(new_name) = prompt(
        &model,
        PromptOptions {
            title: Some(format!("Rename '{remote_name}'")),
            confirm_text: Some("Rename".to_string()),
            cancel_text: Some("Cancel".to_string()),
            placeholder: Some("New remote name".to_string()),
            default_value: Some(remote_name.clone()),
            pattern: Some("^[A-Za-z0-9._-]+$".to_string()),
            input_error_message: Some(
                "Remote names can only contain letters, numbers, dots, dashes and underscores."
                    .to_string(),
            ),
            ..Default::default()
        },
        cx,
    )
    .await
    else {
        return;
    };
    if new_name == remote_name {
        return;
    }

    cx.update(|cx| {
        remote_mutation(&model, repo, "rename-remote", cx, move |model, path, cx| {
            model.rename_remote(path, &remote_name, &new_name, cx)
        })
    })
    .await;
}

/// `handleRemoveRemote`.
pub(crate) async fn remove_remote(
    model: Model,
    repo: RepoRef,
    remote_name: String,
    cx: &mut AsyncApp,
) {
    let confirmed = confirm(
        &model,
        confirm_options(
            "Remove remote",
            format!(
                "Remove '{remote_name}'? Its remote-tracking branches will be deleted locally. The remote repository is not affected."
            ),
            "Remove",
            "Cancel",
            true,
        ),
        cx,
    )
    .await;
    if !confirmed {
        return;
    }

    cx.update(|cx| {
        remote_mutation(&model, repo, "remove-remote", cx, move |model, path, cx| {
            model.remove_remote(path, &remote_name, cx)
        })
    })
    .await;
}

// ---------------------------------------------------------------------------
// Stashes
// ---------------------------------------------------------------------------

/// `handleBranchFromStash`.
pub(crate) async fn branch_from_stash(
    model: Model,
    repo: RepoRef,
    stash_ref: String,
    cx: &mut AsyncApp,
) {
    let Some(branch_name) = prompt(
        &model,
        PromptOptions {
            title: Some("Create branch from stash".to_string()),
            description: Some(format!("Create a new branch from {stash_ref}.")),
            confirm_text: Some("Create".to_string()),
            cancel_text: Some("Cancel".to_string()),
            pattern: Some(NOT_BLANK.to_string()),
            input_error_message: Some("Branch name is required.".to_string()),
            ..Default::default()
        },
        cx,
    )
    .await
    else {
        return;
    };

    let result = model
        .update(cx, |model, cx| {
            model.stash_branch(&repo.path, &stash_ref, &branch_name, cx)
        })
        .await;
    apply(&model, &repo, &result, cx);

    if succeeded_or_conflicted(&result) {
        let needs_status = result.status().is_none();
        let request = with_result(
            SyncRequest::new()
                .branches(true)
                .stashes(true)
                .status(needs_status)
                .changes_count(needs_status),
            &result,
        );
        refresh(&model, &repo, request, cx).await;
    }
}

/// `handleApplyStash`.
pub(crate) async fn apply_stash(model: Model, repo: RepoRef, stash_ref: String, cx: &mut AsyncApp) {
    let result = model
        .update(cx, |model, cx| {
            model.apply_stash(&repo.path, &stash_ref, cx)
        })
        .await;
    apply(&model, &repo, &result, cx);

    if succeeded_or_conflicted(&result) {
        let request = with_result(
            SyncRequest::new()
                .stashes(true)
                .changes_count(result.status().is_none()),
            &result,
        );
        refresh(&model, &repo, request, cx).await;
    }
}

/// `handlePopStash`.
pub(crate) async fn pop_stash(model: Model, repo: RepoRef, stash_ref: String, cx: &mut AsyncApp) {
    let confirmed = confirm(
        &model,
        confirm_options(
            "Pop stash",
            format!("Pop {stash_ref}?"),
            "Pop",
            "Cancel",
            false,
        ),
        cx,
    )
    .await;
    if !confirmed {
        return;
    }

    let result = model
        .update(cx, |model, cx| model.pop_stash(&repo.path, &stash_ref, cx))
        .await;
    apply(&model, &repo, &result, cx);

    if succeeded_or_conflicted(&result) {
        let request = with_result(
            SyncRequest::new()
                .stashes(true)
                .changes_count(result.status().is_none()),
            &result,
        );
        refresh(&model, &repo, request, cx).await;
    }
}

/// `handleDropStash`.
pub(crate) async fn drop_stash(model: Model, repo: RepoRef, stash_ref: String, cx: &mut AsyncApp) {
    let confirmed = confirm(
        &model,
        confirm_options(
            "Drop stash",
            format!("Drop {stash_ref}? This cannot be undone."),
            "Drop",
            "Cancel",
            true,
        ),
        cx,
    )
    .await;
    if !confirmed {
        return;
    }

    let result = model
        .update(cx, |model, cx| model.drop_stash(&repo.path, &stash_ref, cx))
        .await;
    apply(&model, &repo, &result, cx);

    if result.success() {
        let request = with_result(SyncRequest::new().stashes(true), &result);
        refresh(&model, &repo, request, cx).await;
    }
}

// ---------------------------------------------------------------------------
// Submódulos
// ---------------------------------------------------------------------------

/// `runSubmoduleUpdate`: sin rutas actualiza todos.
pub(crate) fn update_submodules(
    model: &Model,
    repo: RepoRef,
    paths: Vec<String>,
    cx: &mut App,
) -> Task<()> {
    single_flight(
        model,
        &repo.clone(),
        "submodule-update",
        cx,
        async move |model, cx| {
            let result = model
                .update(cx, |model, cx| {
                    model.update_submodules(&repo.path, SubmoduleUpdateOptions { paths }, cx)
                })
                .await;
            apply(&model, &repo, &result, cx);
            if succeeded_or_noop(&result) {
                let request = with_result(
                    SyncRequest::new()
                        .submodules(true)
                        .status(true)
                        .changes_count(true),
                    &result,
                );
                refresh(&model, &repo, request, cx).await;
            }
        },
    )
}

/// `handleOpenSubmodule`: el submódulo es un repositorio git normal, se abre
/// como otra pestaña.
pub(crate) fn open_submodule(model: &Model, repo: &RepoRef, submodule_path: &str, cx: &mut App) {
    let path = format!("{}/{submodule_path}", repo.path);
    model
        .update(cx, |model, cx| {
            model.add_repository(&path, AddRepository::default(), cx)
        })
        .detach();
}
