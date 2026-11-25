//! Operaciones que pueden detenerse a medias (merge, rebase, revert,
//! cherry-pick), la resolución de conflictos y continuar / abortar / saltar.

use mergeon_gitcmd::{OperationType, detect_operation_state, get_structured_status};
use std::path::Path;

use crate::exec::{OperationOutcome, exec_checked, git, raw};
use crate::messages::{
    capitalize, clean_message, has_conflict_output, is_already_up_to_date, is_continue_blocked,
    normalize_branch_name, normalize_success_message,
};
use crate::results::{
    OperationPayload, build_conflict_result, build_generic_error_result,
    create_resolve_conflict_result, create_sequence_action_result, finalize_operation_result,
    safe_get_status,
};
use crate::types::{
    Code, ConflictResolution, MergeOptions, Operation, OperationResult, Outcome, RebaseOptions,
    ResolveConflictResult, SequenceActionResult,
};

fn merge_inner(
    repo_path: &Path,
    branch_name: &str,
    options: &MergeOptions,
    target_branch: &mut Option<String>,
) -> OperationOutcome<OperationResult> {
    let git = git(repo_path);

    match options
        .target_branch
        .as_deref()
        .filter(|branch| !branch.is_empty())
    {
        Some(branch) => {
            raw(&git, &["checkout", branch])?;
            *target_branch = Some(branch.to_string());
        }
        None => {
            let status = git.status()?;
            *target_branch = normalize_branch_name(status.current.as_deref());
        }
    }

    let mut merge_args = vec!["merge"];
    if options.no_fast_forward {
        merge_args.push("--no-ff");
    }
    if options.squash {
        merge_args.push("--squash");
    }
    if let Some(message) = options
        .message
        .as_deref()
        .filter(|message| !message.is_empty())
    {
        merge_args.push("-m");
        merge_args.push(message);
    }
    merge_args.push(branch_name);

    let raw_output = raw(&git, &merge_args)?;
    if has_conflict_output(&raw_output) {
        return Ok(build_conflict_result(
            repo_path,
            Operation::Merge,
            target_branch.clone(),
            None,
            Some("Merge stopped because Git detected conflicts."),
        ));
    }

    let payload = if is_already_up_to_date(&raw_output) {
        OperationPayload::new(
            true,
            Operation::Merge,
            Outcome::Noop,
            Code::AlreadyUpToDate,
            format!(
                "{} is already up to date with {branch_name}.",
                target_branch.as_deref().unwrap_or("Current branch")
            ),
            target_branch.clone(),
            None,
        )
    } else {
        OperationPayload::new(
            true,
            Operation::Merge,
            Outcome::Completed,
            Code::Ok,
            normalize_success_message(&raw_output, &format!("Successfully merged {branch_name}.")),
            target_branch.clone(),
            None,
        )
    };

    Ok(finalize_operation_result(repo_path, payload))
}

/// `mergeOperation`: fusiona `branch_name` en la rama actual (o en
/// `options.target_branch`, cambiando antes a ella).
pub fn merge_operation(
    repo_path: &Path,
    branch_name: &str,
    options: &MergeOptions,
) -> OperationResult {
    let mut target_branch = None;

    match merge_inner(repo_path, branch_name, options, &mut target_branch) {
        Ok(result) => result,
        Err(error) => {
            build_generic_error_result(repo_path, Operation::Merge, &error, target_branch, None)
        }
    }
}

/// Cuerpo común de rebase, revert y cherry-pick: leer la rama actual (o
/// cambiar antes a `target_branch`), lanzar el comando y traducir la salida
/// con `describe`.
fn run_on_current_branch(
    repo_path: &Path,
    operation: Operation,
    target_branch: Option<&str>,
    arguments: &[&str],
    describe: impl FnOnce(&str, Option<&str>) -> (Outcome, Code, String),
) -> OperationResult {
    let mut current_branch = None;

    let output = (|| -> OperationOutcome<String> {
        let git = git(repo_path);
        let status = git.status()?;
        current_branch = normalize_branch_name(status.current.as_deref());
        // Igual que en merge: si la rama destino no es la actual se cambia
        // a ella primero; si git se niega (cambios locales) falla limpio sin
        // tocar nada.
        if let Some(target) = target_branch.filter(|target| !target.is_empty())
            && current_branch.as_deref() != Some(target)
        {
            raw(&git, &["checkout", target])?;
            current_branch = Some(target.to_string());
        }
        raw(&git, arguments).map_err(Into::into)
    })();

    match output {
        Ok(raw_output) => {
            let (outcome, code, message) = describe(&raw_output, current_branch.as_deref());
            finalize_operation_result(
                repo_path,
                OperationPayload::new(
                    true,
                    operation,
                    outcome,
                    code,
                    message,
                    current_branch,
                    None,
                ),
            )
        }
        Err(error) => {
            build_generic_error_result(repo_path, operation, &error, current_branch, None)
        }
    }
}

/// `rebaseOperation`: rebasa la rama actual (o `options.target_branch`,
/// cambiando antes a ella) sobre `branch_name`.
pub fn rebase_operation(
    repo_path: &Path,
    branch_name: &str,
    options: &RebaseOptions,
) -> OperationResult {
    let mut rebase_args = vec!["rebase"];
    if options.interactive {
        rebase_args.push("-i");
    }
    rebase_args.push(branch_name);

    run_on_current_branch(
        repo_path,
        Operation::Rebase,
        options.target_branch.as_deref(),
        &rebase_args,
        |raw_output, current_branch| {
            if is_already_up_to_date(raw_output) {
                (
                    Outcome::Noop,
                    Code::AlreadyUpToDate,
                    format!(
                        "{} is already up to date with {branch_name}.",
                        current_branch.unwrap_or("Current branch")
                    ),
                )
            } else {
                (
                    Outcome::Completed,
                    Code::Ok,
                    normalize_success_message(
                        raw_output,
                        &format!("Successfully rebased onto {branch_name}."),
                    ),
                )
            }
        },
    )
}

/// `revertOperation`: crea el commit que deshace `commit`.
pub fn revert_operation(repo_path: &Path, commit: &str) -> OperationResult {
    run_on_current_branch(
        repo_path,
        Operation::Revert,
        None,
        &["revert", "--no-edit", "--end-of-options", commit],
        |raw_output, _| {
            (
                Outcome::Completed,
                Code::Ok,
                normalize_success_message(raw_output, &format!("Reverted commit {commit}.")),
            )
        },
    )
}

/// `cherryPickOperation`: aplica `commit` sobre la rama actual.
pub fn cherry_pick_operation(repo_path: &Path, commit: &str) -> OperationResult {
    run_on_current_branch(
        repo_path,
        Operation::CherryPick,
        None,
        &["cherry-pick", "--end-of-options", commit],
        |raw_output, _| {
            (
                Outcome::Completed,
                Code::Ok,
                normalize_success_message(raw_output, &format!("Cherry-picked commit {commit}.")),
            )
        },
    )
}

fn plural_commits(count: usize) -> String {
    if count == 1 {
        "1 commit".to_string()
    } else {
        format!("{count} commits")
    }
}

/// Cuántos commits añadió la secuencia antes de pararse (`before..HEAD`).
fn applied_since(git: &mergeon_gitcmd::Git, before: Option<&str>) -> usize {
    let Some(before) = before else {
        return 0;
    };
    raw(git, &["rev-list", "--count", &format!("{before}..HEAD")])
        .ok()
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0)
}

/// Cherry-pick o revert de varios commits con un solo `git <comando> a b c`:
/// git los aplica uno a uno y se para en el primero que choca, dejando la
/// secuencia (`.git/sequencer`) para continuar, saltar o abortar.
fn run_commit_sequence(
    repo_path: &Path,
    operation: Operation,
    commits: &[String],
    no_commit: bool,
) -> OperationResult {
    let git = git(repo_path);
    let mut current_branch = None;
    let mut head_before = None;
    let verb = match operation {
        Operation::Revert => "Revert",
        _ => "Cherry-pick",
    };

    let output = (|| -> OperationOutcome<String> {
        let status = git.status()?;
        current_branch = normalize_branch_name(status.current.as_deref());
        head_before = raw(&git, &["rev-parse", "--verify", "--quiet", "HEAD"])
            .ok()
            .map(|head| head.trim().to_string())
            .filter(|head| !head.is_empty());
        let mut arguments = vec![if operation == Operation::Revert {
            "revert"
        } else {
            "cherry-pick"
        }];
        if operation == Operation::Revert {
            arguments.push("--no-edit");
        }
        if no_commit {
            arguments.push("--no-commit");
        }
        arguments.extend(commits.iter().map(String::as_str));
        let output = raw(&git, &arguments)?;
        if no_commit {
            raw(
                &git,
                &["commit", "-m", &squashed_revert_message(&git, commits)],
            )?;
        }
        Ok(output)
    })();

    let count = commits.len();
    let onto = current_branch
        .as_deref()
        .map(|branch| format!(" onto {branch}"))
        .unwrap_or_default();
    match output {
        Ok(_) => {
            let message = match (operation, no_commit) {
                (Operation::Revert, true) => {
                    format!("Reverted {} in a single commit.", plural_commits(count))
                }
                (Operation::Revert, false) => format!("Reverted {}.", plural_commits(count)),
                _ => format!("Cherry-picked {}{onto}.", plural_commits(count)),
            };
            finalize_operation_result(
                repo_path,
                OperationPayload::new(
                    true,
                    operation,
                    Outcome::Completed,
                    Code::Ok,
                    message,
                    current_branch,
                    None,
                ),
            )
        }
        Err(error) if error.control_code().is_none() && has_conflict_output(&error.message) => {
            let applied = if no_commit {
                0
            } else {
                applied_since(&git, head_before.as_deref())
            };
            let message = format!(
                "{verb} stopped on a conflict after {applied} of {count} commits. Resolve it and \
                 continue, skip that commit or abort."
            );
            build_conflict_result(
                repo_path,
                operation,
                current_branch,
                None,
                Some(message.as_str()),
            )
        }
        Err(error) => {
            build_generic_error_result(repo_path, operation, &error, current_branch, None)
        }
    }
}

/// Mensaje del commit único de "Revert N commits" (`--no-commit`): un
/// asunto que dice cuántos y, en el cuerpo, cada commit revertido.
fn squashed_revert_message(git: &mergeon_gitcmd::Git, commits: &[String]) -> String {
    let mut arguments = vec!["log", "-z", "--no-walk=unsorted", "--format=%H%x1f%s"];
    arguments.extend(commits.iter().map(String::as_str));
    let subjects = raw(git, &arguments).unwrap_or_default();
    let mut body = String::new();
    for record in subjects.split('\0') {
        let record = record.trim_start_matches('\n');
        let mut fields = record.splitn(2, '\u{1f}');
        let (Some(hash), subject) = (fields.next().filter(|hash| !hash.is_empty()), fields.next())
        else {
            continue;
        };
        body.push_str(&format!(
            "- {hash} \"{}\"\n",
            subject.unwrap_or_default().trim()
        ));
    }
    if body.is_empty() {
        for hash in commits {
            body.push_str(&format!("- {hash}\n"));
        }
    }
    format!(
        "Revert {}\n\nThis reverts the following commits:\n{body}",
        plural_commits(commits.len())
    )
}

/// Cherry-pick de varios commits sobre la rama actual, en el orden dado (del
/// más viejo al más nuevo). Se para en el primer conflicto y deja la
/// secuencia de git a medias (continuar / saltar / abortar). Con un solo
/// commit es [`cherry_pick_operation`].
pub fn cherry_pick_commits_operation(repo_path: &Path, commits: &[String]) -> OperationResult {
    match commits {
        [] => build_generic_error_result(
            repo_path,
            Operation::CherryPick,
            &crate::exec::OperationError::new("No commits to cherry-pick."),
            None,
            None,
        ),
        [commit] => cherry_pick_operation(repo_path, commit),
        _ => run_commit_sequence(repo_path, Operation::CherryPick, commits, false),
    }
}

/// Revert de varios commits, en el orden dado (del más nuevo al más viejo,
/// para que cada revert se aplique sobre lo que dejó el anterior). Con
/// `single_commit` todo va en un único commit de revert (`--no-commit` y un
/// commit al final). Se para en el primer conflicto como el cherry-pick.
pub fn revert_commits_operation(
    repo_path: &Path,
    commits: &[String],
    single_commit: bool,
) -> OperationResult {
    match commits {
        [] => build_generic_error_result(
            repo_path,
            Operation::Revert,
            &crate::exec::OperationError::new("No commits to revert."),
            None,
            None,
        ),
        [commit] if !single_commit => revert_operation(repo_path, commit),
        _ => run_commit_sequence(repo_path, Operation::Revert, commits, single_commit),
    }
}

/// `resolveConflictOperation`: escribe el contenido resuelto y prepara el archivo.
pub fn resolve_conflict_operation(
    repo_path: &Path,
    file_path: &str,
    content: &str,
) -> ResolveConflictResult {
    resolve_conflict_with_operation(
        repo_path,
        file_path,
        &ConflictResolution::Content(content.to_string()),
    )
}

/// Resuelve el conflicto de `file_path` según `resolution` y lo prepara:
///
/// - `Content`: escribe el texto (tal cual, con sus finales de línea) y `git add`.
/// - `Keep(side)`: saca esa etapa entera del índice (`git checkout-index
///   --stage`), con su modo y sus bytes (binarios incluidos), y `git add`.
/// - `Delete`: `git rm`, que quita las etapas y el archivo del working tree.
pub fn resolve_conflict_with_operation(
    repo_path: &Path,
    file_path: &str,
    resolution: &ConflictResolution,
) -> ResolveConflictResult {
    if let ConflictResolution::Content(content) = resolution
        && let Err(error) = std::fs::write(repo_path.join(file_path), content)
    {
        return create_resolve_conflict_result(
            false,
            Code::WriteFailed,
            clean_message(&error.to_string(), &format!("Failed to write {file_path}.")),
            file_path,
            None,
        );
    }

    let stage = || -> OperationOutcome<_> {
        let git = git(repo_path);
        match resolution {
            ConflictResolution::Content(_) => {
                raw(&git, &["add", "--", file_path])?;
            }
            ConflictResolution::Keep(side) => {
                let stage = format!("--stage={}", side.stage());
                exec_checked(
                    &git,
                    &["checkout-index", "-f", stage.as_str(), "--", file_path],
                )?;
                raw(&git, &["add", "--", file_path])?;
            }
            ConflictResolution::Delete => {
                exec_checked(
                    &git,
                    &["rm", "-q", "-f", "--ignore-unmatch", "--", file_path],
                )?;
                // `git rm` deja el archivo si no lo tenía el índice como suyo.
                match std::fs::remove_file(repo_path.join(file_path)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error.into());
                    }
                    _ => {}
                }
            }
        }
        get_structured_status(repo_path).map_err(Into::into)
    };

    let message = match resolution {
        ConflictResolution::Delete => format!("Resolved {file_path} by deleting it."),
        _ => format!("Resolved {file_path}."),
    };
    match stage() {
        Ok(status) => {
            create_resolve_conflict_result(true, Code::Ok, message, file_path, Some(status))
        }
        Err(error) => {
            let status = safe_get_status(repo_path);
            create_resolve_conflict_result(
                false,
                Code::GitError,
                clean_message(
                    &error.message,
                    &format!("Failed to stage {file_path} after resolving conflicts."),
                ),
                file_path,
                Some(status),
            )
        }
    }
}

/// Comando de git para `action` sobre la operación en curso, si la admite.
fn sequence_command(operation: OperationType, action: &str) -> Option<[&'static str; 2]> {
    let flag = match (operation, action) {
        (_, "continue") => "--continue",
        (_, "abort") => "--abort",
        // `cherry-pick --skip` / `revert --skip` (git ≥ 2.23): el commit que
        // chocó se deja y la secuencia sigue con el resto.
        (OperationType::Rebase | OperationType::CherryPick | OperationType::Revert, "skip") => {
            "--skip"
        }
        _ => return None,
    };
    Some([operation.as_str(), flag])
}

/// `sequenceActionOperation`: `continue`, `abort` o `skip` sobre la operación
/// que haya quedado a medias.
pub fn sequence_action_operation(repo_path: &Path, action: &str) -> SequenceActionResult {
    let operation_state = match detect_operation_state(repo_path) {
        Ok(state) => state,
        Err(error) => {
            // El original deja escapar este error al handler; aquí una mutación
            // nunca falla, así que se devuelve como resultado fallido.
            return create_sequence_action_result(
                false,
                None,
                action,
                Outcome::Failed,
                Code::GitError,
                clean_message(&error.message, "Failed to read the Git operation state."),
                None,
            );
        }
    };

    let Some(operation) = operation_state.kind.filter(|_| operation_state.in_progress) else {
        let status = safe_get_status(repo_path);
        return create_sequence_action_result(
            false,
            None,
            action,
            Outcome::Blocked,
            Code::NoOperationInProgress,
            "No Git operation is currently in progress.",
            Some(status),
        );
    };

    let operation_name = operation.as_str();
    let Some(command) = sequence_command(operation, action) else {
        let status = safe_get_status(repo_path);
        return create_sequence_action_result(
            false,
            Some(operation),
            action,
            Outcome::Blocked,
            Code::ActionNotSupported,
            format!(
                "{} is not supported for {operation_name}.",
                capitalize(action)
            ),
            Some(status),
        );
    };

    let run = || -> OperationOutcome<_> {
        /*
         * Los `--continue` abren el editor para el mensaje del commit. Aquí no hay
         * terminal ni editor: `GIT_EDITOR=true` acepta el mensaje que git ya dejó
         * prellenado (MERGE_MSG y compañía) sin depender del entorno del usuario.
         */
        /*
         * Se decide por código de salida, no con el criterio de `git.raw`
         * (error solo si hay stderr): `rebase --continue` con conflictos sin
         * resolver sale con 1 y explica el motivo por stdout ("needs merge"),
         * y la app Electron lo anunciaba como éxito.
         */
        let output = exec_checked(&git(repo_path).with_env("GIT_EDITOR", "true"), &command)?;
        let status = get_structured_status(repo_path)?;
        Ok((output, status))
    };

    match run() {
        Ok((raw_output, status)) => create_sequence_action_result(
            true,
            Some(operation),
            action,
            Outcome::Completed,
            Code::Ok,
            normalize_success_message(
                &raw_output,
                &format!("{} completed for {operation_name}.", capitalize(action)),
            ),
            Some(status),
        ),
        Err(error) => {
            let status = safe_get_status(repo_path);

            if action == "continue" && is_continue_blocked(&error.message) {
                return create_sequence_action_result(
                    false,
                    Some(operation),
                    action,
                    Outcome::Blocked,
                    Code::ConflictsRemain,
                    format!(
                        "Resolve and stage all conflicted files before continuing {operation_name}."
                    ),
                    Some(status),
                );
            }

            create_sequence_action_result(
                false,
                Some(operation),
                action,
                Outcome::Failed,
                Code::GitError,
                clean_message(
                    &error.message,
                    &format!("Failed to {action} {operation_name}."),
                ),
                Some(status),
            )
        }
    }
}
