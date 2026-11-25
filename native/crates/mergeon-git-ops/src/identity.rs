//! Identidad de commit (`user.name` / `user.email`) y resolución de refs.

use mergeon_gitcmd::{GitError, GitResult};
use std::path::Path;

use crate::exec::{git, raw};
use crate::messages::{clean_message, js_trim};
use crate::results::non_empty;
use crate::types::{
    Code, GitIdentity, IdentityValues, Operation, Outcome, SetIdentityOptions, SetIdentityResult,
};

fn scope_flag(scope: &str) -> &'static str {
    if scope == "global" {
        "--global"
    } else {
        "--local"
    }
}

fn read_config_value(repo_path: &Path, key: &str, scope: &str) -> Option<String> {
    // git config --get sale con 1 cuando la clave no existe.
    let value = raw(
        &git(repo_path),
        &["config", scope_flag(scope), "--get", key],
    )
    .ok()?;
    non_empty(js_trim(&value))
}

fn write_config_value(repo_path: &Path, key: &str, value: &str, scope: &str) -> GitResult<()> {
    if !value.is_empty() {
        raw(&git(repo_path), &["config", scope_flag(scope), key, value])?;
        return Ok(());
    }
    // Desconfigurar una clave que no existe no es un error.
    let _ = raw(
        &git(repo_path),
        &["config", scope_flag(scope), "--unset-all", key],
    );
    Ok(())
}

/*
 * Identidad de commit efectiva: la local pisa a la global. Se devuelven ambas
 * para que la UI muestre "heredado de global" cuando la local está vacía.
 */
pub fn get_git_identity(repo_path: &Path) -> GitIdentity {
    GitIdentity {
        local: IdentityValues {
            name: read_config_value(repo_path, "user.name", "local"),
            email: read_config_value(repo_path, "user.email", "local"),
        },
        global: IdentityValues {
            name: read_config_value(repo_path, "user.name", "global"),
            email: read_config_value(repo_path, "user.email", "global"),
        },
    }
}

/// `setGitIdentityOperation`: escribe nombre y correo en el ámbito indicado;
/// un valor vacío borra la clave.
pub fn set_git_identity_operation(
    repo_path: &Path,
    options: &SetIdentityOptions,
) -> SetIdentityResult {
    let result = |success: bool, outcome: Outcome, code: Code, message: String| SetIdentityResult {
        success,
        operation: Operation::SetIdentity,
        outcome,
        code,
        message,
    };
    let scope = options.scope.as_str();

    if scope != "local" && scope != "global" {
        return result(
            false,
            Outcome::Blocked,
            Code::GitError,
            format!("Unsupported identity scope '{scope}'."),
        );
    }

    let write = || -> GitResult<()> {
        let name = js_trim(options.name.as_deref().unwrap_or_default());
        let email = js_trim(options.email.as_deref().unwrap_or_default());
        write_config_value(repo_path, "user.name", name, scope)?;
        write_config_value(repo_path, "user.email", email, scope)
    };

    match write() {
        Ok(()) => result(
            true,
            Outcome::Completed,
            Code::Ok,
            if scope == "global" {
                "Global git identity updated.".to_string()
            } else {
                "Repository git identity updated.".to_string()
            },
        ),
        Err(error) => result(
            false,
            Outcome::Failed,
            Code::GitError,
            clean_message(&error.message, "Failed to update the git identity."),
        ),
    }
}

/*
 * Hash completo del commit al que apunta un ref (rama local, remota o tag).
 * `^{commit}` pela los tags anotados hasta el commit; `--end-of-options`
 * evita que un nombre de ref con pinta de flag se interprete como opción.
 */
pub fn resolve_ref_to_commit(repo_path: &Path, reference: &str) -> GitResult<String> {
    let trimmed_ref = js_trim(reference);
    if trimmed_ref.is_empty() {
        return Err(GitError::message("A ref is required."));
    }

    let peeled = format!("{trimmed_ref}^{{commit}}");
    let output = raw(
        &git(repo_path),
        &["rev-parse", "--verify", "--end-of-options", peeled.as_str()],
    )?;
    Ok(js_trim(&output).to_string())
}
