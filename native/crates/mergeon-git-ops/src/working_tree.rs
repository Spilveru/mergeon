//! Área de trabajo e índice: preparar, quitar del stage, descartar y confirmar.

use mergeon_gitcmd::{FileStatus, StructuredStatus, get_structured_status};
use std::path::Path;

use crate::exec::{OperationError, OperationOutcome, exec_checked, git, parse_commit_hash, raw};
use crate::messages::{clean_message, js_trim};
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, finalize_mutation_result,
};
use crate::types::{ApplyPatchOptions, Code, CommitOptions, MutationResult, Operation, Outcome};

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn is_untracked(file: &FileStatus) -> bool {
    file.working_dir == "?" || file.working_dir == "??"
}

/// `hasStagedChanges`.
fn has_staged_changes(status: &StructuredStatus) -> bool {
    status.files.iter().any(has_index_changes)
}

/// `getTrackedFileEntry`.
fn tracked_file_entry<'a>(status: &'a StructuredStatus, file_path: &str) -> Option<&'a FileStatus> {
    status.files.iter().find(|file| file.path == file_path)
}

/// `hasWorkingTreeChanges`.
fn has_working_tree_changes(file: &FileStatus) -> bool {
    !file.working_dir.is_empty() && file.working_dir != " " && file.working_dir != "U"
}

/// `hasIndexChanges`.
fn has_index_changes(file: &FileStatus) -> bool {
    !file.index.is_empty() && file.index != " " && file.index != "?" && file.index != "??"
}

/// `getStageCandidates`.
fn stage_candidates(status: &StructuredStatus) -> Vec<String> {
    status
        .files
        .iter()
        .filter(|file| {
            if file.working_dir == "U" {
                return false;
            }
            is_untracked(file) || has_working_tree_changes(file)
        })
        .map(|file| file.path.clone())
        .collect()
}

/// `getUnstageCandidates`.
fn unstage_candidates(status: &StructuredStatus) -> Vec<String> {
    status
        .files
        .iter()
        .filter(|file| has_index_changes(file))
        .map(|file| file.path.clone())
        .collect()
}

/// `getDiscardCandidates`.
fn discard_candidates(status: &StructuredStatus) -> Vec<String> {
    status
        .files
        .iter()
        .filter(|file| has_working_tree_changes(file))
        .map(|file| file.path.clone())
        .collect()
}

/// Borra un archivo o directorio sin fallar si ya no existe
/// (`fs.rm(..., { recursive: true, force: true })`).
fn remove_path(path: &Path) -> std::io::Result<()> {
    let removed = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `discardFile`: los archivos sin seguimiento se borran; el resto se restaura del índice.
fn discard_file(repo_path: &Path, file_path: &str) -> OperationOutcome<bool> {
    let git = git(repo_path);
    let file_status = raw(&git, &["status", "--porcelain", "--", file_path])?;
    let normalized_status = js_trim(&file_status);

    if normalized_status.is_empty() {
        return Ok(false);
    }

    if normalized_status.starts_with("??") {
        remove_path(&repo_path.join(file_path))?;
        return Ok(true);
    }

    raw(&git, &["checkout", "--", file_path])?;
    Ok(true)
}

/// `stageOperation`: prepara un archivo.
pub fn stage_operation(repo_path: &Path, file_path: &str) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        let stageable = tracked_file_entry(&status, file_path)
            .is_some_and(|file| is_untracked(file) || has_working_tree_changes(file));

        if !stageable {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::Stage,
                    Outcome::Noop,
                    Code::NothingToStage,
                    format!("No unstaged changes found for {file_path}."),
                )
                .with(MutationExtra {
                    file_path: Some(file_path.to_string()),
                    ..Default::default()
                }),
            ));
        }

        raw(&git(repo_path), &["add", file_path])?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::Stage, format!("Staged {file_path}.")).with(
                MutationExtra {
                    file_path: Some(file_path.to_string()),
                    files_affected: vec![file_path.to_string()],
                    ..Default::default()
                },
            ),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::Stage,
            &error,
            MutationExtra {
                file_path: Some(file_path.to_string()),
                files_affected: vec![file_path.to_string()],
                ..Default::default()
            },
        )
    })
}

/*
 * Prepara, quita del stage o descarta un bloque (o unas líneas sueltas),
 * aplicando un parche recortado. Contra el índice (`--cached`) deja el working
 * tree intacto, que es justo lo que distingue esto de preparar el archivo
 * entero; con `--reverse` el mismo parche sirve para deshacerlo. Con
 * `worktree` se aplica al working tree: al revés, es descartar.
 *
 * Antes se comprueba con `git apply --check`: un parche que ya no encaja (el
 * archivo cambió desde que se leyó el diff) no toca nada y da un aviso claro en
 * vez del error crudo de git.
 */
pub fn apply_patch_operation(
    repo_path: &Path,
    patch: &str,
    options: &ApplyPatchOptions,
) -> MutationResult {
    let operation = if options.worktree {
        Operation::DiscardHunk
    } else if options.reverse {
        Operation::UnstageHunk
    } else {
        Operation::StageHunk
    };
    let file_path = options.file_path.clone();
    let files_affected: Vec<String> = file_path
        .iter()
        .filter(|path| !path.is_empty())
        .cloned()
        .collect();

    if js_trim(patch).is_empty() {
        return finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                operation,
                Outcome::Blocked,
                Code::EmptyPatch,
                "There is nothing to apply.",
            )
            .with(MutationExtra {
                file_path,
                ..Default::default()
            }),
        );
    }

    let mut flags: Vec<String> = Vec::new();
    if !options.worktree {
        flags.push("--cached".to_string());
    }
    if options.reverse {
        flags.push("--reverse".to_string());
    }

    /*
     * `git apply` recibe el parche como **ruta de archivo**: darle el texto hace
     * que git busque un archivo con ese nombre. Hay que escribirlo en disco
     * primero. El directorio temporal se borra al salir, también si falla.
     */
    let run = || -> OperationOutcome<Result<(), String>> {
        let directory = tempfile::Builder::new()
            .prefix("mergeon-patch-")
            .tempdir()?;
        let patch_file = directory.path().join("block.patch");
        std::fs::write(&patch_file, patch)?;
        let patch_arg = patch_file.to_string_lossy().into_owned();

        let mut check = vec!["apply".to_string(), "--check".to_string()];
        check.extend(flags.iter().cloned());
        check.push(patch_arg.clone());
        // `exec_checked`: falla por el código de salida aunque git calle.
        if let Err(error) = exec_checked(&git(repo_path), &check) {
            let error = OperationError::from(error);
            if error.control_code().is_some() {
                return Err(error);
            }
            return Ok(Err(error.message));
        }

        let mut arguments = vec!["apply".to_string()];
        arguments.extend(flags.iter().cloned());
        arguments.push(patch_arg);
        raw(&git(repo_path), &arguments)?;
        Ok(Ok(()))
    };

    let extra = MutationExtra {
        file_path,
        files_affected,
        ..Default::default()
    };

    match run() {
        Ok(Ok(())) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                operation,
                match operation {
                    Operation::DiscardHunk => "Discarded the selected changes.",
                    Operation::UnstageHunk => "Unstaged the selected changes.",
                    _ => "Staged the selected changes.",
                },
            )
            .with(extra),
        ),
        Ok(Err(git_output)) => finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                operation,
                Outcome::Failed,
                Code::PatchDoesNotApply,
                patch_does_not_apply_message(extra.file_path.as_deref(), &git_output),
            )
            .with(MutationExtra {
                files_affected: Vec::new(),
                ..extra
            }),
        ),
        Err(error) => build_mutation_error_result(repo_path, operation, &error, extra),
    }
}

/// Lo que se le dice a quien pidió aplicar un parche que ya no encaja: qué
/// pasó, qué hacer y, al final, el primer error de git (para quien lo entienda).
fn patch_does_not_apply_message(file_path: Option<&str>, git_output: &str) -> String {
    let target = match file_path.filter(|path| !path.is_empty()) {
        Some(path) => format!("to {path}"),
        None => "to the file".to_string(),
    };
    let detail = git_output
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("error:"))
        .or_else(|| {
            git_output
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
        });
    let mut message = format!(
        "The selected changes no longer apply {target}: the patch does not match the file. \
Reload the diff and try again."
    );
    if let Some(detail) = detail {
        message.push_str(&format!(" ({detail})"));
    }
    message
}

/// Descarta del working tree un bloque o unas líneas: el parche (sacado de
/// `git diff`, índice → working tree) se aplica al revés sin `--cached`.
pub fn discard_patch_operation(
    repo_path: &Path,
    patch: &str,
    file_path: Option<String>,
) -> MutationResult {
    apply_patch_operation(
        repo_path,
        patch,
        &ApplyPatchOptions {
            reverse: true,
            worktree: true,
            file_path,
        },
    )
}

/// `stageAllOperation`: prepara todos los cambios sin preparar.
pub fn stage_all_operation(repo_path: &Path) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        let files_to_stage = stage_candidates(&status);

        if files_to_stage.is_empty() {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::StageAll,
                    Outcome::Noop,
                    Code::NothingToStage,
                    "There are no unstaged changes to stage.",
                ),
            ));
        }

        let mut arguments = vec!["add".to_string()];
        arguments.extend(files_to_stage.iter().cloned());
        raw(&git(repo_path), &arguments)?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::StageAll,
                format!(
                    "Staged {} file{}.",
                    files_to_stage.len(),
                    plural(files_to_stage.len())
                ),
            )
            .with(MutationExtra {
                files_affected: files_to_stage,
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::StageAll,
            &error,
            MutationExtra::default(),
        )
    })
}

/// `unstageOperation`: quita un archivo del stage.
pub fn unstage_operation(repo_path: &Path, file_path: &str) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        let staged = tracked_file_entry(&status, file_path).is_some_and(has_index_changes);

        if !staged {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::Unstage,
                    Outcome::Noop,
                    Code::NothingToUnstage,
                    format!("No staged changes found for {file_path}."),
                )
                .with(MutationExtra {
                    file_path: Some(file_path.to_string()),
                    ..Default::default()
                }),
            ));
        }

        raw(&git(repo_path), &["reset", "HEAD", "--", file_path])?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::Unstage, format!("Unstaged {file_path}.")).with(
                MutationExtra {
                    file_path: Some(file_path.to_string()),
                    files_affected: vec![file_path.to_string()],
                    ..Default::default()
                },
            ),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::Unstage,
            &error,
            MutationExtra {
                file_path: Some(file_path.to_string()),
                files_affected: vec![file_path.to_string()],
                ..Default::default()
            },
        )
    })
}

/// `unstageAllOperation`: quita del stage todo lo preparado.
pub fn unstage_all_operation(repo_path: &Path) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        let files_to_unstage = unstage_candidates(&status);

        if files_to_unstage.is_empty() {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::UnstageAll,
                    Outcome::Noop,
                    Code::NothingToUnstage,
                    "There are no staged changes to unstage.",
                ),
            ));
        }

        let mut arguments = vec!["reset".to_string(), "HEAD".to_string(), "--".to_string()];
        arguments.extend(files_to_unstage.iter().cloned());
        raw(&git(repo_path), &arguments)?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::UnstageAll,
                format!(
                    "Unstaged {} file{}.",
                    files_to_unstage.len(),
                    plural(files_to_unstage.len())
                ),
            )
            .with(MutationExtra {
                files_affected: files_to_unstage,
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::UnstageAll,
            &error,
            MutationExtra::default(),
        )
    })
}

/// `discardChangesOperation`: descarta los cambios sin preparar de un archivo.
pub fn discard_changes_operation(repo_path: &Path, file_path: &str) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        let discardable = tracked_file_entry(&status, file_path)
            .is_some_and(|file| is_untracked(file) || has_working_tree_changes(file));

        if !discardable {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::Discard,
                    Outcome::Noop,
                    Code::NothingToDiscard,
                    format!("No working tree changes found for {file_path}."),
                )
                .with(MutationExtra {
                    file_path: Some(file_path.to_string()),
                    ..Default::default()
                }),
            ));
        }

        discard_file(repo_path, file_path)?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::Discard,
                format!("Discarded changes in {file_path}."),
            )
            .with(MutationExtra {
                file_path: Some(file_path.to_string()),
                files_affected: vec![file_path.to_string()],
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::Discard,
            &error,
            MutationExtra {
                file_path: Some(file_path.to_string()),
                files_affected: vec![file_path.to_string()],
                ..Default::default()
            },
        )
    })
}

/// `discardAllOperation`: descarta `file_paths`, o todos los cambios sin
/// preparar si no se indica ninguno.
pub fn discard_all_operation(repo_path: &Path, file_paths: Option<&[String]>) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;
        let requested = match file_paths {
            Some(paths) if !paths.is_empty() => paths.to_vec(),
            _ => discard_candidates(&status),
        };
        let mut files_to_discard: Vec<String> = Vec::new();
        for file_path in requested {
            if !files_to_discard.contains(&file_path) {
                files_to_discard.push(file_path);
            }
        }

        if files_to_discard.is_empty() {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::DiscardAll,
                    Outcome::Noop,
                    Code::NothingToDiscard,
                    "There are no working tree changes to discard.",
                ),
            ));
        }

        for file_path in &files_to_discard {
            discard_file(repo_path, file_path)?;
        }

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::DiscardAll,
                format!(
                    "Discarded changes in {} file{}.",
                    files_to_discard.len(),
                    plural(files_to_discard.len())
                ),
            )
            .with(MutationExtra {
                files_affected: files_to_discard,
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::DiscardAll,
            &error,
            MutationExtra::default(),
        )
    })
}

/// La salida de un `git commit -S` fallido habla de la firma (gpg o
/// ssh-keygen sin clave, clave que no se puede leer…).
fn is_signing_failure(message: &str) -> bool {
    let message = message.to_lowercase();
    [
        "failed to sign",
        "signingkey",
        "gpg.ssh",
        "cannot run gpg",
        "ssh-keygen",
        "load key",
        "secret key",
        "signing failed",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

/// Cómo arreglarlo y, después, qué dijo git (sin "failed to write commit
/// object", que no explica nada). Lo primero cabe entero en un aviso.
fn signing_failure_message(output: &str) -> String {
    let details = output
        .lines()
        .map(|line| clean_message(line, ""))
        .filter(|line| !line.is_empty() && !line.contains("failed to write commit object"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut message = String::from(
        "Could not sign the commit: set a signing key (user.signingkey; gpg.format ssh for SSH keys) or turn off Sign.",
    );
    if !details.is_empty() {
        message.push_str(" Git said: ");
        message.push_str(details.trim_end_matches('.'));
        message.push('.');
    }
    message
}

/// El hash completo de `target` si es un commit de la historia de HEAD; si
/// no, el mensaje que explica por qué no sirve de destino de un fixup.
fn resolve_fixup_target(
    repo_path: &Path,
    target: &str,
) -> OperationOutcome<Result<String, String>> {
    let short: String = target.chars().take(7).collect();
    let runner = git(repo_path);
    let resolved = runner.exec([
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &format!("{target}^{{commit}}"),
    ])?;
    if !resolved.success() {
        return Ok(Err(format!("Commit {short} does not exist.")));
    }
    let full = js_trim(&resolved.stdout_text()).to_string();
    let ancestor = runner.exec(["merge-base", "--is-ancestor", &full, "HEAD"])?;
    if !ancestor.success() {
        return Ok(Err(format!(
            "Commit {short} is not part of the current branch history."
        )));
    }
    Ok(Ok(full))
}

/// `commitOperation`: crea un commit con lo preparado, o enmienda el último
/// (`amend`), firmado si se pide (`sign`). Con `fixup` crea `fixup! <asunto>`
/// para ese commit (que debe estar en la historia de HEAD); el mensaje se
/// ignora.
pub fn commit_operation(
    repo_path: &Path,
    message: &str,
    options: &CommitOptions,
) -> MutationResult {
    let trimmed_message = js_trim(message);
    let fixup_target = options
        .fixup
        .as_deref()
        .map(js_trim)
        .filter(|target| !target.is_empty());

    if fixup_target.is_some() && options.amend {
        return finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                Operation::Commit,
                Outcome::Blocked,
                Code::GitError,
                "A fixup commit can't amend the last commit.",
            ),
        );
    }

    // Un fixup lleva el mensaje que pone git (`fixup! <asunto>`).
    if trimmed_message.is_empty() && fixup_target.is_none() {
        return finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                Operation::Commit,
                Outcome::Blocked,
                Code::CommitMessageRequired,
                "A commit message is required before committing.",
            ),
        );
    }

    let run = || -> OperationOutcome<MutationResult> {
        let status = get_structured_status(repo_path)?;

        if status.operation_state.in_progress {
            let in_progress = status
                .operation_state
                .kind
                .map_or("a Git operation", |kind| kind.as_str());
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::Commit,
                    Outcome::Blocked,
                    Code::CommitBlockedByOperation,
                    format!("Cannot create a commit while {in_progress} is in progress."),
                ),
            ));
        }

        if !options.amend && !has_staged_changes(&status) {
            let payload = if status.is_clean {
                MutationPayload::failure(
                    Operation::Commit,
                    Outcome::Noop,
                    Code::NoChanges,
                    "There are no changes to commit.",
                )
            } else {
                MutationPayload::failure(
                    Operation::Commit,
                    Outcome::Blocked,
                    Code::NothingToStage,
                    "Stage changes before creating a commit.",
                )
            };
            return Ok(finalize_mutation_result(repo_path, payload));
        }

        // El destino del fixup: un commit de la historia de HEAD.
        let fixup = match fixup_target {
            Some(target) => match resolve_fixup_target(repo_path, target)? {
                Ok(full) => Some(full),
                Err(message) => {
                    return Ok(finalize_mutation_result(
                        repo_path,
                        MutationPayload::failure(
                            Operation::Commit,
                            Outcome::Blocked,
                            Code::GitError,
                            message,
                        ),
                    ));
                }
            },
            None => None,
        };
        let fixup_argument = fixup.as_ref().map(|full| format!("--fixup={full}"));

        // Enmendar no exige nada preparado: corregir solo el mensaje del último
        // commit es su uso más frecuente.
        let mut arguments = vec!["-c", "core.abbrev=40", "commit"];
        match &fixup_argument {
            Some(fixup_argument) => arguments.push(fixup_argument.as_str()),
            None => arguments.extend(["-m", trimmed_message]),
        }
        if options.amend {
            arguments.push("--amend");
        }
        if options.sign {
            arguments.push("-S");
        }
        let output = match raw(&git(repo_path), &arguments).map_err(OperationError::from) {
            Ok(output) => output,
            // Sin clave configurada git solo dice "failed to write commit
            // object": se explica qué falta en vez de un error genérico.
            Err(error)
                if options.sign
                    && error.control_code().is_none()
                    && is_signing_failure(&error.message) =>
            {
                return Ok(finalize_mutation_result(
                    repo_path,
                    MutationPayload::failure(
                        Operation::Commit,
                        Outcome::Failed,
                        Code::SigningFailed,
                        signing_failure_message(&error.message),
                    ),
                ));
            }
            Err(error) => return Err(error),
        };
        let commit = parse_commit_hash(&output);

        let short = |hash: &str| hash.chars().take(7).collect::<String>();
        let message = if let Some(target) = &fixup {
            if commit.is_empty() {
                format!("Created a fixup commit for {}.", short(target))
            } else {
                format!(
                    "Created fixup commit {} for {}.",
                    short(&commit),
                    short(target)
                )
            }
        } else if commit.is_empty() {
            "Commit created successfully.".to_string()
        } else {
            format!(
                "Created commit {}.",
                commit.chars().take(7).collect::<String>()
            )
        };

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::Commit, message),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::Commit,
            &error,
            MutationExtra::default(),
        )
    })
}
