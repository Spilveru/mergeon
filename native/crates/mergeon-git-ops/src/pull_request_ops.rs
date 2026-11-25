//! Traer la rama de un pull request de GitHub para poder hacer checkout.
//!
//! - PR desde el mismo repositorio: se actualiza solo su rama de seguimiento
//!   (`refs/heads/<rama>` → `refs/remotes/<remoto>/<rama>`, como un fetch
//!   normal) y el checkout de la rama remota hace el resto.
//! - PR desde un fork: `git fetch <remoto> pull/<n>/head:pr-<n>` crea (o
//!   avanza, solo en fast-forward) la rama local `pr-<n>`.

use std::path::Path;

use crate::exec::{OperationOutcome, authenticated_git, get_remotes_verbose, raw};
use crate::results::{
    MutationExtra, MutationPayload, build_remote_mutation_error_result, compute_refs_changed,
    finalize_mutation_result, read_refs_fingerprint,
};
use crate::types::{Code, MutationResult, Operation, Outcome};

/// De dónde sale la rama del PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRequestHead {
    /// La rama vive en el propio remoto.
    SameRepository { branch: String },
    /// La rama vive en un fork: se trae por `pull/<n>/head`.
    Fork { number: u64 },
}

#[derive(Clone, PartialEq, Eq)]
pub struct FetchPullRequestOptions {
    /// El remoto que apunta al repositorio del PR.
    pub remote: String,
    pub head: PullRequestHead,
    /// Token de GitHub para el fetch (como en `FetchOptions`).
    pub auth_token: Option<String>,
}

// El token no se imprime.
impl std::fmt::Debug for FetchPullRequestOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchPullRequestOptions")
            .field("remote", &self.remote)
            .field("head", &self.head)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// La rama local en la que se trae un PR de un fork.
pub fn pull_request_branch_name(number: u64) -> String {
    format!("pr-{number}")
}

/// Un nombre de rama que se puede poner en un refspec sin cambiarle el
/// sentido: nada de opciones, `:`, comodines ni rangos.
fn is_safe_branch_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("@{")
        && !name.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, ':' | '~' | '^' | '?' | '*' | '[' | '\\')
        })
}

/// `git fetch` de la rama de un PR (ver el módulo). Termina como `fetch`
/// en el log de operaciones.
pub fn fetch_pull_request_operation(
    repo_path: &Path,
    options: &FetchPullRequestOptions,
) -> MutationResult {
    let remote = options.remote.as_str();
    let blocked = |code: Code, message: String| {
        finalize_mutation_result(
            repo_path,
            MutationPayload::failure(Operation::Fetch, Outcome::Blocked, code, message).with(
                MutationExtra {
                    remote: Some(remote.to_string()),
                    refs_changed: Some(false),
                    ..Default::default()
                },
            ),
        )
    };

    let (refspec, branch, message) = match &options.head {
        PullRequestHead::SameRepository { branch } => {
            if !is_safe_branch_name(branch) || !is_safe_branch_name(remote) {
                return blocked(
                    Code::GitError,
                    format!("'{branch}' is not a branch name that can be fetched."),
                );
            }
            (
                format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
                format!("{remote}/{branch}"),
                format!("Fetched '{branch}' from '{remote}'."),
            )
        }
        PullRequestHead::Fork { number } => {
            if *number == 0 || !is_safe_branch_name(remote) {
                return blocked(
                    Code::GitError,
                    format!("Pull request #{number} cannot be fetched."),
                );
            }
            let local = pull_request_branch_name(*number);
            (
                format!("refs/pull/{number}/head:refs/heads/{local}"),
                local.clone(),
                format!("Fetched pull request #{number} into '{local}'."),
            )
        }
    };

    let git = authenticated_git(repo_path, options.auth_token.as_deref());
    let refs_before = read_refs_fingerprint(repo_path);
    let run = || -> OperationOutcome<MutationResult> {
        let remotes = get_remotes_verbose(&git)?;
        if !remotes.iter().any(|candidate| candidate.name == remote) {
            return Ok(blocked(
                Code::RemoteNotFound,
                format!("Remote '{remote}' not found. Please configure the remote first."),
            ));
        }
        raw(&git, &["fetch", "--no-tags", remote, refspec.as_str()])?;
        let refs_changed = compute_refs_changed(
            refs_before.as_deref(),
            read_refs_fingerprint(repo_path).as_deref(),
        );
        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::Fetch, message.clone()).with(MutationExtra {
                remote: Some(remote.to_string()),
                branch: Some(branch.clone()),
                refs_changed,
                ..Default::default()
            }),
        ))
    };

    run().unwrap_or_else(|error| {
        build_remote_mutation_error_result(
            repo_path,
            Operation::Fetch,
            &error,
            MutationExtra {
                remote: Some(remote.to_string()),
                ..Default::default()
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_branch_names_go_into_a_refspec() {
        for name in ["main", "feat/search", "release-2.4", "ünïcode"] {
            assert!(is_safe_branch_name(name), "{name}");
        }
        for name in [
            "",
            "-upload-pack=x",
            "a:b",
            "a b",
            "a..b",
            "a*",
            "x.lock",
            "/x",
            "x/",
            "a@{1}",
            "a\\b",
        ] {
            assert!(!is_safe_branch_name(name), "{name}");
        }
    }
}
