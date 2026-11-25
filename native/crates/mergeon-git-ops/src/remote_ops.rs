//! Operaciones de red: push, fetch y pull.

use std::path::Path;

use crate::exec::{
    OperationError, OperationOutcome, authenticated_git, get_remotes_verbose, git, raw,
};
use crate::messages::{
    clean_message, has_auth_error, has_conflict_output, is_already_up_to_date,
    is_transient_fetch_error, js_trim, normalize_branch_name, normalize_success_message,
};
use crate::results::{
    MutationExtra, MutationPayload, build_remote_mutation_error_result, compute_refs_changed,
    finalize_mutation_result, read_refs_fingerprint,
};
use crate::types::{
    Code, FetchOptions, MutationResult, Operation, Outcome, PullOptions, PushOptions,
    RemoteFetchResult,
};

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn push_inner(
    repo_path: &Path,
    options: &PushOptions,
    source_branch: &mut Option<String>,
) -> OperationOutcome<MutationResult> {
    let remote = options.remote.as_str();
    let base_git = git(repo_path);
    let destination_branch = normalize_branch_name(options.target_branch.as_deref());

    if source_branch.is_none() {
        let status = base_git.status()?;
        *source_branch = normalize_branch_name(status.current.as_deref());
    }

    let Some(source) = source_branch.clone() else {
        return Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                Operation::Push,
                Outcome::Blocked,
                Code::NoCurrentBranch,
                "No current branch is checked out. Switch to a branch before pushing.",
            )
            .with(MutationExtra {
                remote: Some(remote.to_string()),
                ..Default::default()
            }),
        ));
    };

    let blocked = |code: Code, message: String| {
        finalize_mutation_result(
            repo_path,
            MutationPayload::failure(Operation::Push, Outcome::Blocked, code, message).with(
                MutationExtra {
                    branch: Some(source.clone()),
                    remote: Some(remote.to_string()),
                    ..Default::default()
                },
            ),
        )
    };

    let remotes = get_remotes_verbose(&base_git)?;
    let Some(remote_config) = remotes.iter().find(|candidate| candidate.name == remote) else {
        return Ok(blocked(
            Code::RemoteNotFound,
            format!("Remote '{remote}' not found. Please configure the remote first."),
        ));
    };

    if remote_config.push.is_empty() && remote_config.fetch.is_empty() {
        return Ok(blocked(
            Code::RemotePushUrlMissing,
            format!("Remote '{remote}' has no push URL configured."),
        ));
    }

    // `--progress` + `with_progress`: el avance llega a la cola de mutaciones.
    let git = authenticated_git(repo_path, options.auth_token.as_deref()).with_progress();
    /*
     * El original preguntaba aquí por `branches.branches[rama].tracking`, pero
     * `git.branch()` de simple-git no expone ese campo: `hasUpstream` era
     * siempre false, todo push a la rama del mismo nombre llevaba `-u` (y
     * movía el upstream de una rama que ya lo tenía) y se anunciaba como
     * UPSTREAM_CREATED. Aquí se lee el upstream configurado de la rama.
     */
    let upstream = raw(
        &base_git,
        &[
            "for-each-ref".to_string(),
            "--format=%(upstream)".to_string(),
            format!("refs/heads/{source}"),
        ],
    )?;
    let has_upstream = !js_trim(&upstream).is_empty();
    let resolved_target_branch = destination_branch.unwrap_or_else(|| source.clone());
    let same_branch = resolved_target_branch == source;
    let ref_spec = if same_branch {
        source.clone()
    } else {
        format!("{source}:{resolved_target_branch}")
    };

    let mut push_args = vec!["push", "--progress"];
    if options.force {
        push_args.push("--force-with-lease");
    }
    if !has_upstream && same_branch {
        push_args.push("-u");
    }
    push_args.push(remote);
    push_args.push(&ref_spec);

    let raw_output = raw(&git, &push_args)?;

    if options.push_tags {
        raw(&git, &["push", "--progress", remote, "--tags"])?;
    }

    let noop = is_already_up_to_date(&raw_output);
    let upstream_set = !has_upstream && same_branch;

    let (code, message) = if upstream_set {
        (
            Code::UpstreamCreated,
            format!("Push completed and upstream set to {remote}/{source}."),
        )
    } else if noop {
        (
            Code::AlreadyUpToDate,
            "Everything is already up to date.".to_string(),
        )
    } else if options.push_tags {
        (
            Code::Ok,
            "Push completed successfully, including tags.".to_string(),
        )
    } else {
        (
            Code::Ok,
            normalize_success_message(&raw_output, "Push completed successfully."),
        )
    };

    Ok(finalize_mutation_result(
        repo_path,
        MutationPayload::new(
            true,
            Operation::Push,
            if noop {
                Outcome::Noop
            } else {
                Outcome::Completed
            },
            code,
            message,
        )
        .with(MutationExtra {
            branch: Some(source),
            remote: Some(remote.to_string()),
            upstream_set,
            ..Default::default()
        }),
    ))
}

/// `pushOperation`: empuja una rama (la actual por defecto) al remoto.
pub fn push_operation(repo_path: &Path, options: &PushOptions) -> MutationResult {
    let mut source_branch = normalize_branch_name(options.branch.as_deref());

    match push_inner(repo_path, options, &mut source_branch) {
        Ok(result) => result,
        Err(error) => build_remote_mutation_error_result(
            repo_path,
            Operation::Push,
            &error,
            MutationExtra {
                branch: source_branch,
                remote: Some(options.remote.clone()),
                ..Default::default()
            },
        ),
    }
}

/// Fetch de un remoto, con un reintento si el fallo es de red y transitorio.
fn fetch_remote(
    repo_path: &Path,
    auth_token: Option<&str>,
    remote_name: &str,
) -> OperationOutcome<RemoteFetchResult> {
    let git = authenticated_git(repo_path, auth_token).with_progress();

    let mut attempt = 0;
    loop {
        /*
         * `git.fetch(remoto)` de simple-git solo pasa el remoto al comando si
         * también recibe una rama; con el remoto solo ejecuta `git fetch` a
         * secas, que trae el remoto por defecto de la rama actual. En la app
         * Electron eso hacía que elegir otro remoto trajera el de siempre y
         * que "Fetch all" trajera N veces el mismo. Aquí va el remoto pedido.
         *
         * `--prune` borra las ramas de seguimiento (`origin/x`) de las ramas
         * que ya no existen en el remoto. Sin él una rama borrada en GitHub
         * seguía para siempre en el sidebar y, como el historial se lee con
         * `--all`, mantenía su carril abierto en el grafo. Las ramas locales
         * no se tocan.
         *
         * `--progress` hace que git informe del avance aunque stderr no sea
         * una terminal; el runner lo separa del stderr del resultado.
         */
        match raw(&git, &["fetch", "--prune", "--progress", remote_name]) {
            Ok(_) => break,
            Err(error) => {
                let error = OperationError::from(error);
                if attempt >= 1
                    || error.control_code().is_some()
                    || !is_transient_fetch_error(&error.message)
                {
                    return Err(error);
                }
                attempt += 1;
            }
        }
    }

    Ok(RemoteFetchResult {
        remote: remote_name.to_string(),
        success: true,
        summary: None,
        code: None,
        message: None,
    })
}

/// `fetchOperation`: trae un remoto, o todos con `all_remotes`.
pub fn fetch_operation(repo_path: &Path, options: &FetchOptions) -> MutationResult {
    let remote = options.remote.as_str();
    let auth_token = options.auth_token.as_deref();
    let base_git = git(repo_path);
    let refs_before = read_refs_fingerprint(repo_path);

    /*
     * Un fetch que no movió ningún ref se anuncia como noop: así la UI puede
     * saltarse la recarga del historial en vez de reconstruir la lista entera.
     */
    let finalize_fetch = |mut payload: MutationPayload| {
        let refs_changed = compute_refs_changed(
            refs_before.as_deref(),
            read_refs_fingerprint(repo_path).as_deref(),
        );
        payload.extra.refs_changed = refs_changed;

        if refs_changed == Some(false) && payload.success {
            payload.outcome = Outcome::Noop;
            payload.code = Code::AlreadyUpToDate;
            payload.message = "Already up to date.".to_string();
        }

        finalize_mutation_result(repo_path, payload)
    };

    if !options.all_remotes {
        let run = || -> OperationOutcome<MutationResult> {
            let remotes = get_remotes_verbose(&base_git)?;
            if !remotes.iter().any(|candidate| candidate.name == remote) {
                return Ok(finalize_mutation_result(
                    repo_path,
                    MutationPayload::failure(
                        Operation::Fetch,
                        Outcome::Blocked,
                        Code::RemoteNotFound,
                        format!("Remote '{remote}' not found. Please configure the remote first."),
                    )
                    .with(MutationExtra {
                        remote: Some(remote.to_string()),
                        refs_changed: Some(false),
                        ..Default::default()
                    }),
                ));
            }

            let remote_result = fetch_remote(repo_path, auth_token, remote)?;
            Ok(finalize_fetch(
                MutationPayload::completed(
                    Operation::Fetch,
                    format!("Fetch completed successfully for '{remote}'."),
                )
                .with(MutationExtra {
                    remote: Some(remote.to_string()),
                    summary: remote_result.summary.clone(),
                    remote_results: vec![remote_result],
                    ..Default::default()
                }),
            ))
        };

        return run().unwrap_or_else(|error| {
            build_remote_mutation_error_result(
                repo_path,
                Operation::Fetch,
                &error,
                MutationExtra {
                    remote: Some(remote.to_string()),
                    ..Default::default()
                },
            )
        });
    }

    let remotes = match get_remotes_verbose(&base_git) {
        Ok(remotes) => remotes,
        Err(error) => {
            return build_remote_mutation_error_result(
                repo_path,
                Operation::Fetch,
                &error.into(),
                MutationExtra {
                    all_remotes: true,
                    ..Default::default()
                },
            );
        }
    };

    if remotes.is_empty() {
        return finalize_mutation_result(
            repo_path,
            MutationPayload::new(
                true,
                Operation::Fetch,
                Outcome::Noop,
                Code::NoRemotes,
                "No Git remotes are configured.",
            )
            .with(MutationExtra {
                all_remotes: true,
                refs_changed: Some(false),
                ..Default::default()
            }),
        );
    }

    let mut remote_results = Vec::new();
    for remote_config in &remotes {
        match fetch_remote(repo_path, auth_token, &remote_config.name) {
            Ok(result) => remote_results.push(result),
            Err(error) => {
                let message = clean_message(
                    &error.message,
                    &format!("Git fetch failed for '{}'.", remote_config.name),
                );
                let control_code = error.control_code();
                let code = match control_code {
                    Some(code) => code.into(),
                    None if has_auth_error(&message) => Code::AuthRequired,
                    None if has_conflict_output(&message) => Code::Conflict,
                    None => Code::GitError,
                };
                remote_results.push(RemoteFetchResult {
                    remote: remote_config.name.clone(),
                    success: false,
                    summary: None,
                    code: Some(code),
                    message: Some(message),
                });
                if control_code.is_some() {
                    break;
                }
            }
        }
    }

    let failures: Vec<&RemoteFetchResult> = remote_results
        .iter()
        .filter(|result| !result.success)
        .collect();
    let succeeded = remote_results.len() - failures.len();

    let Some(first_failure) = failures.first() else {
        // Con un solo remoto se nombra, como en el fetch de un remoto elegido.
        let message = match remote_results.as_slice() {
            [only] => format!("Fetch completed successfully for '{}'.", only.remote),
            _ => format!(
                "Fetch completed successfully for {} remote{}.",
                remote_results.len(),
                plural(remote_results.len())
            ),
        };
        return finalize_fetch(MutationPayload::completed(Operation::Fetch, message).with(
            MutationExtra {
                all_remotes: true,
                remote_results,
                ..Default::default()
            },
        ));
    };

    let partial = succeeded > 0;
    let control_failure = failures
        .iter()
        .find(|failure| matches!(failure.code, Some(Code::Cancelled | Code::Timeout)));

    let (outcome, code, message) = match control_failure {
        Some(failure) => (
            if failure.code == Some(Code::Timeout) {
                Outcome::Timeout
            } else {
                Outcome::Cancelled
            },
            failure.code.unwrap_or(Code::Cancelled),
            failure.message.clone().unwrap_or_default(),
        ),
        None if partial => (
            Outcome::Partial,
            Code::PartialFailure,
            format!(
                "Fetch completed for {succeeded} remote{} but failed for {}.\n{}",
                plural(succeeded),
                failures.len(),
                fetch_failure_reasons(&failures)
            ),
        ),
        None => (
            Outcome::Failed,
            first_failure.code.unwrap_or(Code::GitError),
            match failures.as_slice() {
                [only] => format!(
                    "Fetch failed for '{}'.\n{}",
                    only.remote,
                    only.message.as_deref().unwrap_or_default()
                ),
                _ => format!(
                    "Fetch failed for all configured remotes.\n{}",
                    fetch_failure_reasons(&failures)
                ),
            },
        ),
    };

    finalize_fetch(
        MutationPayload::failure(Operation::Fetch, outcome, code, message).with(MutationExtra {
            all_remotes: true,
            remote_results,
            ..Default::default()
        }),
    )
}

/// Por qué falló cada remoto, uno por línea. Sin esto el aviso solo decía
/// que el fetch había fallado y el motivo se perdía.
fn fetch_failure_reasons(failures: &[&RemoteFetchResult]) -> String {
    failures
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                failure.remote,
                failure.message.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `pullOperation`: trae e integra la rama remota, con merge o con rebase.
pub fn pull_operation(repo_path: &Path, options: &PullOptions) -> MutationResult {
    let refs_before = read_refs_fingerprint(repo_path);
    let extra = MutationExtra {
        remote: options.remote.clone(),
        branch: options.branch.clone(),
        rebase: options.rebase,
        autostash: options.autostash,
        ..Default::default()
    };

    let run = || -> OperationOutcome<MutationResult> {
        let git = authenticated_git(repo_path, options.auth_token.as_deref()).with_progress();

        let mut args = vec!["pull", "--progress"];

        /*
         * Git moderno aborta un pull divergente si nadie le dice cómo reconciliar
         * ("Need to specify how to reconcile divergent branches") salvo que el
         * usuario tenga pull.rebase configurado. La estrategia ya se eligió en el
         * diálogo, así que se pasa explícita para no depender de la config global.
         */
        if options.rebase {
            args.push("--rebase");
        } else {
            args.push("--no-rebase");
        }

        if options.autostash {
            args.push("--autostash");
        }

        if let Some(remote) = options
            .remote
            .as_deref()
            .filter(|remote| !remote.is_empty())
        {
            args.push(remote);
        }

        if let Some(branch) = options
            .branch
            .as_deref()
            .filter(|branch| !branch.is_empty())
        {
            args.push(branch);
        }

        let output = raw(&git, &args)?;
        let noop = is_already_up_to_date(&output);

        /*
         * `noop` mira solo la rama local: el fetch que hace el pull por debajo pudo
         * mover otras ramas de seguimiento, y esas sí cambian el historial. La
         * huella de refs es la que decide si hay que recargar.
         */
        let refs_changed = compute_refs_changed(
            refs_before.as_deref(),
            read_refs_fingerprint(repo_path).as_deref(),
        );

        let payload = if noop {
            MutationPayload::new(
                true,
                Operation::Pull,
                Outcome::Noop,
                Code::AlreadyUpToDate,
                "Already up to date.",
            )
        } else {
            MutationPayload::completed(
                Operation::Pull,
                normalize_success_message(&output, "Pull completed successfully."),
            )
        };

        Ok(finalize_mutation_result(
            repo_path,
            payload.with(MutationExtra {
                summary: Some(output),
                refs_changed,
                ..extra.clone()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_remote_mutation_error_result(repo_path, Operation::Pull, &error, extra.clone())
    })
}
