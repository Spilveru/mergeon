//! Limpieza en lote de ramas locales ya integradas (sin original).
//!
//! Recibe la lista que el usuario confirmó en el diálogo. Cada rama sin
//! `force` se vuelve a comprobar contra la base justo antes de borrarla (el
//! usuario pudo añadirle commits mientras el diálogo estaba abierto); las que
//! llevan `force` (squash-merge, upstream borrado) se borran con `-D` porque
//! el usuario lo pidió así. Con `delete_remote`, también su rama en el
//! remoto (si sigue existiendo), con el token de GitHub como el resto de
//! operaciones de red.

use std::path::Path;

use crate::exec::{authenticated_git, git, raw};
use crate::messages::js_trim;
use crate::results::{MutationExtra, MutationPayload, finalize_mutation_result};
use crate::types::{CleanupBranchesOptions, Code, MutationResult, Operation, Outcome};

const FIELD_SEPARATOR: char = '\u{1f}';

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Punta y upstream (remoto y ref en el remoto) de una rama local.
struct LocalBranch {
    tip: String,
    remote: Option<String>,
    remote_ref: Option<String>,
    gone: bool,
}

fn read_branch(git: &mergeon_gitcmd::Git, name: &str) -> Option<LocalBranch> {
    let format = [
        "%(objectname)",
        "%(upstream:remotename)",
        "%(upstream:remoteref)",
        "%(upstream:track)",
    ]
    .join(&FIELD_SEPARATOR.to_string());
    let output = raw(
        git,
        &[
            "for-each-ref".to_string(),
            format!("--format={format}"),
            format!("refs/heads/{name}"),
        ],
    )
    .ok()?;
    // `for-each-ref refs/heads/x` también casa `refs/heads/x/y`: la primera
    // línea es la exacta si existe, pero se comprueba igual.
    let line = output.lines().next()?;
    let fields: Vec<&str> = line.split(FIELD_SEPARATOR).collect();
    let [tip, remote, remote_ref, track] = fields.as_slice() else {
        return None;
    };
    let exact = raw(
        git,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "--quiet".to_string(),
            format!("refs/heads/{name}"),
        ],
    )
    .ok()?;
    if js_trim(&exact) != *tip {
        return None;
    }
    let text = |value: &str| (!value.is_empty()).then(|| value.to_string());
    Some(LocalBranch {
        tip: tip.to_string(),
        remote: text(remote),
        remote_ref: text(remote_ref),
        gone: track.contains("gone"),
    })
}

fn is_ancestor(git: &mergeon_gitcmd::Git, tip: &str, base: &str) -> bool {
    git.exec(["merge-base", "--is-ancestor", tip, base])
        .is_ok_and(|output| output.success())
}

/// `cleanupBranches`: borra las ramas pedidas y, si se pide, sus ramas remotas.
pub fn cleanup_branches_operation(
    repo_path: &Path,
    options: &CleanupBranchesOptions,
) -> MutationResult {
    let base = js_trim(&options.base).to_string();
    let blocked = |code: Code, message: String| {
        finalize_mutation_result(
            repo_path,
            MutationPayload::failure(Operation::CleanupBranches, Outcome::Blocked, code, message),
        )
    };
    if options.branches.is_empty() {
        return blocked(Code::NoChanges, "No branches selected.".to_string());
    }

    let local = git(repo_path);
    let base_oid = if base.is_empty() || base.starts_with('-') {
        None
    } else {
        raw(
            &local,
            &[
                "rev-parse".to_string(),
                "--verify".to_string(),
                "--quiet".to_string(),
                "--end-of-options".to_string(),
                format!("{base}^{{commit}}"),
            ],
        )
        .ok()
        .map(|oid| js_trim(&oid).to_string())
        .filter(|oid| !oid.is_empty())
    };
    let Some(base_oid) = base_oid else {
        return blocked(
            Code::GitError,
            format!("Base branch '{base}' does not exist."),
        );
    };
    let current = local
        .exec(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|output| output.success())
        .map(|output| js_trim(&output.stdout_text()).to_string());
    let remote_git = authenticated_git(repo_path, options.auth_token.as_deref());

    let mut deleted = Vec::new();
    let mut deleted_remote = 0;
    let mut failures: Vec<String> = Vec::new();
    let mut log = Vec::new();

    for request in &options.branches {
        let name = js_trim(&request.name).to_string();
        if name.is_empty() || name.starts_with('-') {
            failures.push(format!("{name}: invalid branch name"));
            continue;
        }
        if current.as_deref() == Some(name.as_str()) {
            failures.push(format!("{name}: it is the current branch"));
            continue;
        }
        let Some(branch) = read_branch(&local, &name) else {
            failures.push(format!("{name}: branch not found"));
            continue;
        };
        if !request.force && !is_ancestor(&local, &branch.tip, &base_oid) {
            failures.push(format!("{name}: not fully merged into {base}"));
            continue;
        }

        match raw(&local, &["branch", "-D", name.as_str()]) {
            Ok(output) => {
                deleted.push(name.clone());
                log.push(js_trim(&output).to_string());
            }
            Err(error) => {
                failures.push(format!("{name}: {}", js_trim(&error.message)));
                continue;
            }
        }

        if !options.delete_remote || branch.gone {
            continue;
        }
        let (Some(remote), Some(remote_ref)) = (&branch.remote, &branch.remote_ref) else {
            continue;
        };
        // Un upstream que es otro repo local (`.`) no es una rama remota.
        if remote == "." {
            continue;
        }
        match raw(
            &remote_git,
            &[
                "push",
                "--delete",
                "--end-of-options",
                remote.as_str(),
                remote_ref.as_str(),
            ],
        ) {
            Ok(_) => {
                deleted_remote += 1;
                let short = remote_ref.strip_prefix("refs/heads/").unwrap_or(remote_ref);
                log.push(format!("Deleted {remote}/{short}."));
            }
            Err(error) => failures.push(format!(
                "{remote}/{}: {}",
                remote_ref.strip_prefix("refs/heads/").unwrap_or(remote_ref),
                js_trim(&error.message)
            )),
        }
    }

    let mut message = format!("Deleted {}", plural(deleted.len(), "branch", "branches"));
    if deleted_remote > 0 {
        message.push_str(&format!(
            " and {}",
            plural(deleted_remote, "remote branch", "remote branches")
        ));
    }
    message.push('.');

    let summary = log
        .iter()
        .chain(failures.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let extra = MutationExtra {
        summary: (!summary.is_empty()).then_some(summary),
        refs_changed: Some(!deleted.is_empty()),
        ..Default::default()
    };

    let payload = if failures.is_empty() {
        MutationPayload::completed(Operation::CleanupBranches, message)
    } else {
        let reasons = failures.join("\n");
        let (outcome, code) = if deleted.is_empty() {
            (Outcome::Failed, Code::GitError)
        } else {
            (Outcome::Partial, Code::PartialFailure)
        };
        MutationPayload::failure(
            Operation::CleanupBranches,
            outcome,
            code,
            format!("{message} {} failed:\n{reasons}", failures.len()),
        )
    };
    finalize_mutation_result(repo_path, payload.with(extra))
}
