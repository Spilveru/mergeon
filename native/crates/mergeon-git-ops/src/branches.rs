//! Ramas: checkout, creación, renombrado, borrado, fast-forward y reset.

use std::path::Path;

use crate::exec::{OperationOutcome, authenticated_git, git, raw};
use crate::messages::{
    capitalize, is_already_up_to_date, js_trim, normalize_branch_name, normalize_success_message,
};
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, create_mutation_result,
    finalize_mutation_result, non_empty, require_args, safe_get_status,
};
use crate::types::{Code, MutationResult, Operation, Outcome};

/// `checkoutOperation`: cambia a una rama, o a un commit con HEAD desanclado.
pub fn checkout_operation(repo_path: &Path, reference: &str) -> MutationResult {
    let run = || -> OperationOutcome<MutationResult> {
        raw(
            &git(repo_path),
            &["checkout", "--end-of-options", reference],
        )?;
        let status = safe_get_status(repo_path);
        let current = normalize_branch_name(status.current.as_deref());

        let payload = match &current {
            Some(branch) => {
                MutationPayload::completed(Operation::Checkout, format!("Switched to {branch}."))
            }
            None => MutationPayload::new(
                true,
                Operation::Checkout,
                Outcome::Completed,
                Code::DetachedHead,
                format!("Checked out {reference} in detached HEAD state."),
            ),
        };

        Ok(create_mutation_result(
            payload.with(MutationExtra {
                branch: current,
                ..Default::default()
            }),
            Some(status),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::Checkout,
            &error,
            MutationExtra {
                branch: Some(reference.to_string()),
                ..Default::default()
            },
        )
    })
}

/// `checkoutRemoteBranchOperation`: crea la rama local que sigue a `remote_branch`
/// (`origin/feature` → `feature`) y cambia a ella.
pub fn checkout_remote_branch_operation(repo_path: &Path, remote_branch: &str) -> MutationResult {
    let local_branch_name = remote_branch
        .split_once('/')
        .map(|(_, branch)| branch)
        .unwrap_or_default();

    let run = || -> OperationOutcome<MutationResult> {
        raw(
            &git(repo_path),
            &[
                "checkout",
                "-b",
                local_branch_name,
                "--track",
                remote_branch,
            ],
        )?;

        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::CheckoutRemoteBranch,
                format!("Created and switched to {local_branch_name} tracking {remote_branch}."),
            )
            .with(MutationExtra {
                branch: Some(local_branch_name.to_string()),
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(
            repo_path,
            Operation::CheckoutRemoteBranch,
            &error,
            MutationExtra {
                branch: non_empty(local_branch_name),
                remote: Some(remote_branch.to_string()),
                ..Default::default()
            },
        )
    })
}

/// `createBranchOperation`: crea una rama desde `start_point` (o HEAD) y, si
/// `checkout_after_create`, cambia a ella.
pub fn create_branch_operation(
    repo_path: &Path,
    branch_name: &str,
    start_point: Option<&str>,
    checkout_after_create: bool,
) -> MutationResult {
    let base_reference = js_trim(start_point.unwrap_or_default());

    let [target_branch] = match require_args(
        repo_path,
        Operation::CreateBranch,
        [branch_name],
        "A branch name is required.",
        |[branch]| MutationExtra {
            branch: non_empty(branch),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let run = || -> OperationOutcome<()> {
        let git = git(repo_path);
        let target = target_branch.as_str();
        match (checkout_after_create, base_reference.is_empty()) {
            (true, false) => raw(&git, &["checkout", "-b", target, base_reference])?,
            (true, true) => raw(&git, &["checkout", "-b", target])?,
            (false, false) => raw(&git, &["branch", "-v", target, base_reference])?,
            (false, true) => raw(&git, &["branch", "-v", target])?,
        };
        Ok(())
    };

    let extra = MutationExtra {
        branch: Some(target_branch.clone()),
        ..Default::default()
    };

    match run() {
        Ok(()) => {
            let message = match (checkout_after_create, base_reference.is_empty()) {
                (true, false) => {
                    format!("Created {target_branch} from {base_reference} and switched to it.")
                }
                (true, true) => format!("Created and switched to {target_branch}."),
                (false, false) => format!("Created {target_branch} from {base_reference}."),
                (false, true) => format!("Created {target_branch}."),
            };
            finalize_mutation_result(
                repo_path,
                MutationPayload::completed(Operation::CreateBranch, message).with(extra),
            )
        }
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::CreateBranch, &error, extra)
        }
    }
}

/// `deleteRemoteBranchOperation`: borra una rama en el remoto, sin token de
/// autenticación (como en la app Electron). Ver
/// [`delete_remote_branch_with_auth_operation`].
pub fn delete_remote_branch_operation(
    repo_path: &Path,
    remote_name: &str,
    branch_name: &str,
) -> MutationResult {
    delete_remote_branch_with_auth_operation(repo_path, remote_name, branch_name, None)
}

/// Borra una rama en el remoto autenticando con el token de GitHub si lo hay,
/// como push, fetch, pull y las operaciones de tags remotos. La app Electron no
/// lo pasaba aquí, así que con un remoto HTTPS privado el borrado fallaba por
/// falta de credenciales.
pub fn delete_remote_branch_with_auth_operation(
    repo_path: &Path,
    remote_name: &str,
    branch_name: &str,
    auth_token: Option<&str>,
) -> MutationResult {
    let [remote, branch] = match require_args(
        repo_path,
        Operation::DeleteRemoteBranch,
        [remote_name, branch_name],
        "A remote and branch name are required.",
        |[remote, branch]| MutationExtra {
            branch: non_empty(branch),
            remote: non_empty(remote),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let extra = MutationExtra {
        branch: Some(branch.clone()),
        remote: Some(remote.clone()),
        ..Default::default()
    };

    // Mismos argumentos que arma `git.push(remoto, rama, { '--delete': null })`.
    let pushed = raw(
        &authenticated_git(repo_path, auth_token),
        &[
            "push",
            remote.as_str(),
            branch.as_str(),
            "--delete",
            "--verbose",
            "--porcelain",
        ],
    );

    match pushed {
        Ok(_) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::DeleteRemoteBranch,
                format!("Deleted {remote}/{branch}."),
            )
            .with(extra),
        ),
        Err(error) => build_mutation_error_result(
            repo_path,
            Operation::DeleteRemoteBranch,
            &error.into(),
            extra,
        ),
    }
}

/// `renameBranchOperation`.
pub fn rename_branch_operation(
    repo_path: &Path,
    branch_name: &str,
    next_branch_name: &str,
) -> MutationResult {
    let [source_branch, target_branch] = match require_args(
        repo_path,
        Operation::RenameBranch,
        [branch_name, next_branch_name],
        "Both the current and new branch names are required.",
        |[source, target]| MutationExtra {
            branch: non_empty(target).or_else(|| non_empty(source)),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let extra = MutationExtra {
        branch: Some(target_branch.clone()),
        ..Default::default()
    };

    if source_branch == target_branch {
        return finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                Operation::RenameBranch,
                Outcome::Noop,
                Code::NoBranchChange,
                format!("Branch is already named {target_branch}."),
            )
            .with(extra),
        );
    }

    let run = || -> OperationOutcome<()> {
        let git = git(repo_path);
        let status = git.status()?;
        let current_branch = normalize_branch_name(status.current.as_deref());

        if current_branch.as_deref() == Some(source_branch.as_str()) {
            raw(&git, &["branch", "-v", "-m", target_branch.as_str()])?;
        } else {
            raw(
                &git,
                &[
                    "branch",
                    "-v",
                    "-m",
                    source_branch.as_str(),
                    target_branch.as_str(),
                ],
            )?;
        }
        Ok(())
    };

    match run() {
        Ok(()) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::RenameBranch,
                format!("Renamed {source_branch} to {target_branch}."),
            )
            .with(extra),
        ),
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::RenameBranch, &error, extra)
        }
    }
}

/// `deleteBranchOperation`: borra una rama local ya fusionada.
pub fn delete_branch_operation(repo_path: &Path, branch_name: &str) -> MutationResult {
    let [target_branch] = match require_args(
        repo_path,
        Operation::DeleteBranch,
        [branch_name],
        "A branch name is required.",
        |[branch]| MutationExtra {
            branch: non_empty(branch),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let extra = MutationExtra {
        branch: Some(target_branch.clone()),
        ..Default::default()
    };

    let run = || -> OperationOutcome<MutationPayload> {
        let git = git(repo_path);
        let status = git.status()?;
        let current_branch = normalize_branch_name(status.current.as_deref());

        if current_branch.as_deref() == Some(target_branch.as_str()) {
            return Ok(MutationPayload::failure(
                Operation::DeleteBranch,
                Outcome::Blocked,
                Code::ActiveBranch,
                format!("Cannot delete the currently checked out branch '{target_branch}'."),
            ));
        }

        raw(&git, &["branch", "-v", "-d", target_branch.as_str()])?;

        Ok(MutationPayload::completed(
            Operation::DeleteBranch,
            format!("Deleted branch {target_branch}."),
        ))
    };

    match run() {
        Ok(payload) => finalize_mutation_result(repo_path, payload.with(extra)),
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::DeleteBranch, &error, extra)
        }
    }
}

/// `fastForwardBranchOperation`: avanza la rama actual hasta su upstream.
pub fn fast_forward_branch_operation(
    repo_path: &Path,
    branch_name: &str,
    upstream_branch: &str,
) -> MutationResult {
    let [target_branch, upstream] = match require_args(
        repo_path,
        Operation::FastForwardBranch,
        [branch_name, upstream_branch],
        "Both the branch and its upstream are required for fast-forward.",
        |[branch, upstream]| MutationExtra {
            branch: non_empty(branch),
            remote: non_empty(upstream),
            ..Default::default()
        },
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    let extra = MutationExtra {
        branch: Some(target_branch.clone()),
        remote: Some(upstream.clone()),
        ..Default::default()
    };

    let run = || -> OperationOutcome<MutationPayload> {
        let git = git(repo_path);
        let status = git.status()?;
        let current_branch = normalize_branch_name(status.current.as_deref());

        if current_branch.as_deref() != Some(target_branch.as_str()) {
            return Ok(MutationPayload::failure(
                Operation::FastForwardBranch,
                Outcome::Blocked,
                Code::BranchNotCheckedOut,
                format!("Switch to {target_branch} before fast-forwarding it."),
            ));
        }

        let raw_output = raw(&git, &["merge", "--ff-only", upstream.as_str()])?;

        Ok(if is_already_up_to_date(&raw_output) {
            MutationPayload::new(
                true,
                Operation::FastForwardBranch,
                Outcome::Noop,
                Code::AlreadyUpToDate,
                format!("{target_branch} is already up to date with {upstream}."),
            )
        } else {
            MutationPayload::completed(
                Operation::FastForwardBranch,
                normalize_success_message(
                    &raw_output,
                    &format!("Fast-forwarded {target_branch} to {upstream}."),
                ),
            )
        })
    };

    match run() {
        Ok(payload) => finalize_mutation_result(repo_path, payload.with(extra)),
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::FastForwardBranch, &error, extra)
        }
    }
}

const RESET_MODES: [&str; 5] = ["soft", "mixed", "hard", "keep", "merge"];

/// `resetOperation`: mueve la rama actual a `commit` con el modo indicado.
pub fn reset_operation(repo_path: &Path, commit: &str, mode: &str) -> MutationResult {
    let extra = MutationExtra {
        commit: Some(commit.to_string()),
        mode: Some(mode.to_string()),
        ..Default::default()
    };

    // El modo se interpola como flag: solo los de la allowlist.
    if !RESET_MODES.contains(&mode) {
        return finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                Operation::Reset,
                Outcome::Blocked,
                Code::GitError,
                format!("Unsupported reset mode '{mode}'."),
            )
            .with(extra),
        );
    }

    let mode_flag = format!("--{mode}");
    match raw(
        &git(repo_path),
        &["reset", mode_flag.as_str(), "--end-of-options", commit],
    ) {
        Ok(_) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::Reset,
                format!(
                    "{} reset to {}.",
                    capitalize(mode),
                    commit.chars().take(7).collect::<String>()
                ),
            )
            .with(extra),
        ),
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::Reset, &error.into(), extra)
        }
    }
}
