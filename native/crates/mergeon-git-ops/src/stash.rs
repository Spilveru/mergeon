//! Stashes: guardar, aplicar, sacar, borrar y convertir en rama.

use mergeon_gitcmd::{FileStatus, get_structured_status};
use std::path::Path;

use crate::exec::{
    OperationError, OperationOutcome, exec_checked, exec_with_stdin_checked, git, raw,
};
use crate::messages::js_trim;
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, create_mutation_result,
    finalize_mutation_result, non_empty, require_args, safe_get_status,
};
use crate::types::{Code, MutationResult, Operation, Outcome, StashOptions};
use crate::version::{StashCapabilities, stash_capabilities};

fn looks_like_sha(value: &str) -> bool {
    (7..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/*
 * `git stash pop|drop` solo aceptan referencias `stash@{n}`; el SHA de commit
 * que devuelve `stash list` no les sirve. Pero el índice `n` se corre cada vez
 * que aparece o desaparece un stash, así que el único identificador estable
 * entre operaciones es el SHA. Este helper acepta ambos formatos: un SHA se
 * traduce al `stash@{n}` vigente leyendo el reflog de refs/stash en el momento
 * de ejecutar; si el stash ya no existe (o no hay stashes) devuelve `None`.
 */
pub(crate) fn resolve_stash_ref(repo_path: &Path, stash_ref: &str) -> Option<String> {
    let trimmed = js_trim(stash_ref);
    if !looks_like_sha(trimmed) {
        return non_empty(trimmed);
    }

    let wanted = trimmed.to_lowercase();
    let output = raw(
        &git(repo_path),
        &["log", "-g", "refs/stash", "--format=%H %gd"],
    )
    .ok()?;

    output.split('\n').find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        let reference = parts.next()?;
        sha.to_lowercase()
            .starts_with(&wanted)
            .then(|| reference.to_string())
    })
}

/// `buildStashNotFoundResult`.
fn stash_not_found_result(
    repo_path: &Path,
    operation: Operation,
    stash_ref: &str,
) -> MutationResult {
    finalize_mutation_result(
        repo_path,
        MutationPayload::failure(
            operation,
            Outcome::Failed,
            Code::StashNotFound,
            "The stash no longer exists.",
        )
        .with(MutationExtra {
            stash_ref: Some(stash_ref.to_string()),
            ..Default::default()
        }),
    )
}

/// `stashOperation`: guarda todos los cambios en un stash, con mensaje
/// opcional. Es [`stash_push_operation`] sin más opciones.
pub fn stash_operation(repo_path: &Path, message: Option<&str>) -> MutationResult {
    stash_push_operation(
        repo_path,
        &StashOptions {
            message: message.map(str::to_string),
            ..Default::default()
        },
    )
}

fn is_untracked_file(file: &FileStatus) -> bool {
    file.index == "?" || file.working_dir == "?"
}

fn has_side(column: &str) -> bool {
    !matches!(column, "" | " " | "?" | "!")
}

fn has_staged_side(file: &FileStatus) -> bool {
    has_side(&file.index)
}

/// Las rutas que se pasan a git: las pedidas, sin repetir y sin vacías.
fn stash_pathspecs(paths: &[String]) -> Vec<String> {
    let mut pathspecs: Vec<String> = Vec::new();
    for path in paths {
        if !path.is_empty() && !pathspecs.contains(path) {
            pathspecs.push(path.clone());
        }
    }
    pathspecs
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn stash_failure(
    repo_path: &Path,
    outcome: Outcome,
    code: Code,
    message: String,
) -> MutationResult {
    finalize_mutation_result(
        repo_path,
        MutationPayload::failure(Operation::Stash, outcome, code, message),
    )
}

/// El SHA de `refs/stash`, o `None` si no hay stashes.
fn stash_head(repo_path: &Path) -> OperationOutcome<Option<String>> {
    let output = git(repo_path).exec(["rev-parse", "--verify", "--quiet", "refs/stash"])?;
    Ok(output
        .success()
        .then(|| js_trim(&output.stdout_text()).to_string())
        .filter(|sha| !sha.is_empty()))
}

/// Qué impide guardar con estas opciones, antes de llamar a git.
fn stash_blocker(
    options: &StashOptions,
    capabilities: StashCapabilities,
    files: &[FileStatus],
    pathspecs: &[String],
) -> Option<(Outcome, Code, String)> {
    let blocked = |message: &str| Some((Outcome::Blocked, Code::GitError, message.to_string()));
    if options.staged_only {
        if !capabilities.staged_only {
            return blocked("Stashing only staged changes needs Git 2.35 or newer.");
        }
        if options.include_untracked {
            return blocked("Only staged changes can't be combined with untracked files.");
        }
        if options.keep_index {
            return blocked("Only staged changes can't be combined with keeping the index.");
        }
    }

    let selected = |file: &&FileStatus| {
        pathspecs.is_empty()
            || pathspecs.iter().any(|path| {
                file.path == *path
                    || file.from.as_deref() == Some(path.as_str())
                    || file
                        .path
                        .strip_prefix(path.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
    };
    let selected_files: Vec<&FileStatus> = files.iter().filter(selected).collect();

    // El origen de un renombre o un borrado preparados ya no están en el
    // índice: `stash push -- <ruta>` no los encuentra y guardar solo la ruta
    // nueva dejaría un stash que no se puede recuperar limpio.
    if let Some(file) = selected_files.iter().find(|file| {
        matches!(file.index.as_str(), "R" | "D")
            && (pathspecs.contains(&file.path)
                || file
                    .from
                    .as_ref()
                    .is_some_and(|from| pathspecs.contains(from)))
    }) {
        let what = if file.index == "R" {
            "rename"
        } else {
            "deletion"
        };
        return blocked(&format!(
            "Git can't stash the staged {what} of {} by path: unstage it first or stash all changes.",
            file.path
        ));
    }

    if !options.include_untracked && !pathspecs.is_empty() {
        let untracked: Vec<&str> = selected_files
            .iter()
            .filter(|file| is_untracked_file(file) && pathspecs.contains(&file.path))
            .map(|file| file.path.as_str())
            .collect();
        if let Some(first) = untracked.first() {
            return blocked(&format!(
                "{first} is untracked: turn on \"Include untracked files\" to stash it."
            ));
        }
    }

    let stashable = selected_files.iter().any(|file| {
        if options.staged_only {
            has_staged_side(file)
        } else {
            options.include_untracked || !is_untracked_file(file)
        }
    });
    if !stashable {
        let message = match (options.staged_only, pathspecs.is_empty()) {
            (true, true) => "There are no staged changes to stash.",
            (true, false) => "The selected files have no staged changes to stash.",
            (false, true) => "There are no changes to stash.",
            (false, false) => "The selected files have no changes to stash.",
        };
        return Some((Outcome::Noop, Code::NoChanges, message.to_string()));
    }
    None
}

/// `git stash push` con opciones: mensaje, solo unas rutas, lo que no se
/// sigue, conservar el índice o solo lo preparado.
///
/// Las rutas son literales (`GIT_LITERAL_PATHSPECS`: `*` o `:` en un nombre no
/// son comodines ni magia) y, si git lo admite (2.26+), van por stdin con
/// `--pathspec-from-file`, sin límite de línea de comandos. Si git deja un
/// stash creado pero falla al limpiar el working tree (`--staged` con un
/// archivo a medio preparar cuyas partes se tocan), ese stash se borra: los
/// cambios siguen en el working tree y no se guarda nada a medias.
pub fn stash_push_operation(repo_path: &Path, options: &StashOptions) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        if status.is_clean {
            return Ok(stash_failure(
                repo_path,
                Outcome::Noop,
                Code::NoChanges,
                "There are no changes to stash.".to_string(),
            ));
        }

        let capabilities = stash_capabilities();
        let pathspecs = stash_pathspecs(&options.paths);
        if let Some((outcome, code, message)) =
            stash_blocker(options, capabilities, &status.files, &pathspecs)
        {
            return Ok(stash_failure(repo_path, outcome, code, message));
        }

        let mut arguments: Vec<String> = vec!["stash".into(), "push".into()];
        if let Some(message) = options
            .message
            .as_deref()
            .map(js_trim)
            .filter(|message| !message.is_empty())
        {
            arguments.extend(["-m".to_string(), message.to_string()]);
        }
        if options.include_untracked {
            arguments.push("--include-untracked".into());
        }
        if options.keep_index {
            arguments.push("--keep-index".into());
        }
        if options.staged_only {
            arguments.push("--staged".into());
        }
        let mut stdin: Option<Vec<u8>> = None;
        if !pathspecs.is_empty() {
            if capabilities.pathspec_from_file {
                arguments.extend([
                    "--pathspec-from-file=-".to_string(),
                    "--pathspec-file-nul".to_string(),
                ]);
                stdin = Some(
                    pathspecs
                        .iter()
                        .flat_map(|path| path.bytes().chain(std::iter::once(0)))
                        .collect(),
                );
            } else {
                arguments.push("--".into());
                arguments.extend(pathspecs.iter().cloned());
            }
        }

        let before = stash_head(repo_path)?;
        // Rutas literales: `*` o `:` en un nombre no son comodines ni magia.
        // Solo con rutas: sin ellas, `stash --keep-index` usa `:/` por dentro
        // y con `GIT_LITERAL_PATHSPECS` falla a medias.
        let mut runner = git(repo_path);
        if !pathspecs.is_empty() {
            runner = runner.with_env("GIT_LITERAL_PATHSPECS", "1");
        }
        let output = match &stdin {
            Some(stdin) => exec_with_stdin_checked(&runner, &arguments, stdin),
            None => exec_checked(&runner, &arguments),
        };
        let after = stash_head(repo_path)?;

        if let Err(error) = output {
            let error = OperationError::from(error);
            if error.control_code().is_none() && after.is_some() && after != before {
                // El stash quedó creado pero git no pudo sacar los cambios
                // del working tree: se borra para no tenerlos dos veces.
                raw(&git(repo_path), &["stash", "drop", "--quiet", "stash@{0}"])?;
                let message = if options.staged_only {
                    "Git could not take the staged changes out of the working tree: some files also have unstaged edits next to them. Nothing was stashed; stash the whole files instead."
                } else {
                    "Git could not take the changes out of the working tree. Nothing was stashed."
                };
                return Ok(stash_failure(
                    repo_path,
                    Outcome::Failed,
                    Code::GitError,
                    message.to_string(),
                ));
            }
            return Err(error);
        }

        // `No local changes to save` sale con 0 y no crea nada.
        let Some(stash_ref) = after.filter(|after| before.as_ref() != Some(after)) else {
            return Ok(stash_failure(
                repo_path,
                Outcome::Noop,
                Code::NoChanges,
                if pathspecs.is_empty() {
                    "There are no changes to stash."
                } else {
                    "The selected files have no changes to stash."
                }
                .to_string(),
            ));
        };

        /*
         * Devolvemos el SHA del stash recién creado (no `stash@{0}`) para que los
         * flujos que luego hacen pop/drop apunten a ese stash concreto aunque el
         * índice se haya corrido por stashes intermedios.
         */
        let message = match (options.paths.is_empty(), options.staged_only) {
            (true, false) => "Changes stashed successfully.".to_string(),
            (true, true) => "Staged changes stashed.".to_string(),
            (false, staged) => {
                let count = options.paths.len();
                format!(
                    "Stashed {}changes in {count} file{}.",
                    if staged { "the staged " } else { "" },
                    plural(count)
                )
            }
        };
        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::Stash, message).with(MutationExtra {
                stash_ref: Some(stash_ref),
                files_affected: pathspecs.clone(),
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::Stash,
            &error,
            MutationExtra::default(),
        )
    })
}

/// Cuerpo común de aplicar y sacar un stash: solo cambian el subcomando y los textos.
fn restore_stash(
    repo_path: &Path,
    operation: Operation,
    subcommand: &str,
    stash_ref: &str,
    describe_conflict: impl Fn(&str) -> String,
    describe_success: impl Fn(&str) -> String,
) -> MutationResult {
    let extra = MutationExtra {
        stash_ref: Some(stash_ref.to_string()),
        ..Default::default()
    };

    let run = || -> OperationOutcome<MutationResult> {
        let Some(resolved_ref) = resolve_stash_ref(repo_path, stash_ref) else {
            return Ok(stash_not_found_result(repo_path, operation, stash_ref));
        };
        raw(
            &git(repo_path),
            &["stash", subcommand, resolved_ref.as_str()],
        )?;
        let status = safe_get_status(repo_path);

        if status.conflict_state.pending_count > 0 {
            let files_affected = status.conflict_state.pending_files.clone();
            return Ok(create_mutation_result(
                MutationPayload::failure(
                    operation,
                    Outcome::Conflicted,
                    Code::StashConflict,
                    describe_conflict(&resolved_ref),
                )
                .with(MutationExtra {
                    files_affected,
                    ..extra.clone()
                }),
                Some(status),
            ));
        }

        Ok(create_mutation_result(
            MutationPayload::completed(operation, describe_success(&resolved_ref))
                .with(extra.clone()),
            Some(status),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(repo_path, operation, &error, extra.clone())
    })
}

/// `applyStashOperation`: aplica un stash y lo conserva.
pub fn apply_stash_operation(repo_path: &Path, stash_ref: &str) -> MutationResult {
    restore_stash(
        repo_path,
        Operation::ApplyStash,
        "apply",
        stash_ref,
        |resolved_ref| format!("Applying {resolved_ref} created conflicts."),
        |resolved_ref| format!("Applied {resolved_ref}."),
    )
}

/// `popStashOperation`: aplica un stash y lo elimina de la lista.
pub fn pop_stash_operation(repo_path: &Path, stash_ref: &str) -> MutationResult {
    restore_stash(
        repo_path,
        Operation::PopStash,
        "pop",
        stash_ref,
        |resolved_ref| format!("Popping {resolved_ref} created conflicts."),
        |resolved_ref| format!("Popped {resolved_ref}."),
    )
}

/// `dropStashOperation`: elimina un stash sin aplicarlo.
pub fn drop_stash_operation(repo_path: &Path, stash_ref: &str) -> MutationResult {
    let extra = MutationExtra {
        stash_ref: Some(stash_ref.to_string()),
        ..Default::default()
    };

    let run = || -> OperationOutcome<MutationResult> {
        let Some(resolved_ref) = resolve_stash_ref(repo_path, stash_ref) else {
            return Ok(stash_not_found_result(
                repo_path,
                Operation::DropStash,
                stash_ref,
            ));
        };
        raw(&git(repo_path), &["stash", "drop", resolved_ref.as_str()])?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::DropStash, format!("Dropped {resolved_ref}."))
                .with(extra.clone()),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(repo_path, Operation::DropStash, &error, extra.clone())
    })
}

/// `stashBranchOperation`: crea una rama a partir de un stash y cambia a ella.
pub fn stash_branch_operation(
    repo_path: &Path,
    stash_ref: &str,
    branch_name: &str,
) -> MutationResult {
    let [target_branch] = match require_args(
        repo_path,
        Operation::StashBranch,
        [branch_name],
        "A branch name is required.",
        |[branch]| MutationExtra {
            branch: non_empty(branch),
            stash_ref: non_empty(js_trim(stash_ref)),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let [normalized_stash_ref] = match require_args(
        repo_path,
        Operation::StashBranch,
        [stash_ref],
        "A stash reference is required.",
        |[stash_ref]| MutationExtra {
            branch: Some(target_branch.clone()),
            stash_ref: non_empty(stash_ref),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let extra = MutationExtra {
        branch: Some(target_branch.clone()),
        stash_ref: Some(normalized_stash_ref.clone()),
        ..Default::default()
    };

    let run = || -> OperationOutcome<MutationResult> {
        let Some(resolved_ref) = resolve_stash_ref(repo_path, &normalized_stash_ref) else {
            return Ok(stash_not_found_result(
                repo_path,
                Operation::StashBranch,
                &normalized_stash_ref,
            ));
        };
        raw(
            &git(repo_path),
            &[
                "stash",
                "branch",
                target_branch.as_str(),
                resolved_ref.as_str(),
            ],
        )?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::StashBranch,
                format!("Created {target_branch} from {normalized_stash_ref} and switched to it."),
            )
            .with(extra.clone()),
        ))
    };

    run().unwrap_or_else(|error| {
        let status = safe_get_status(repo_path);
        if status.conflict_state.pending_count > 0 {
            let files_affected = status.conflict_state.pending_files.clone();
            return create_mutation_result(
                MutationPayload::failure(
                    Operation::StashBranch,
                    Outcome::Conflicted,
                    Code::StashConflict,
                    format!(
                        "Creating {target_branch} from {normalized_stash_ref} created conflicts."
                    ),
                )
                .with(MutationExtra {
                    files_affected,
                    ..extra.clone()
                }),
                Some(status),
            );
        }

        build_mutation_error_result(repo_path, Operation::StashBranch, &error, extra.clone())
    })
}
