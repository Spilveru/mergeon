//! Tags: crear, publicar, borrar (local y remoto) y hacer checkout.

use mergeon_gitcmd::Git;
use std::path::Path;

use crate::exec::{authenticated_git, git, raw};
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, create_mutation_result,
    finalize_mutation_result, safe_get_status,
};
use crate::types::{Code, MutationResult, Operation, Outcome, RemoteTagOptions};

/// Ejecuta un comando de tags y arma el resultado: todas estas operaciones
/// comparten la misma forma (un comando, un mensaje de éxito, el mismo `extra`).
fn run_tag_command(
    repo_path: &Path,
    operation: Operation,
    git: &Git,
    arguments: &[&str],
    success_message: String,
    extra: MutationExtra,
) -> MutationResult {
    match raw(git, arguments) {
        Ok(_) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(operation, success_message).with(extra),
        ),
        Err(error) => build_mutation_error_result(repo_path, operation, &error.into(), extra),
    }
}

/// `createTagOperation`: tag ligero sobre `commit` (HEAD por defecto), o
/// anotado si se da un mensaje.
pub fn create_tag_operation(
    repo_path: &Path,
    tag_name: &str,
    commit: Option<&str>,
    message: Option<&str>,
) -> MutationResult {
    let commit = commit.unwrap_or("HEAD");
    let arguments = match message.filter(|message| !message.is_empty()) {
        Some(message) => vec!["tag", "-a", tag_name, commit, "-m", message],
        None => vec!["tag", tag_name, commit],
    };

    run_tag_command(
        repo_path,
        Operation::CreateTag,
        &git(repo_path),
        &arguments,
        format!("Created tag {tag_name}."),
        MutationExtra {
            tag: Some(tag_name.to_string()),
            ..Default::default()
        },
    )
}

/*
 * Un tag creado con `git tag` se queda en la máquina: nadie más lo ve hasta que
 * se empuja. Son operaciones aparte a propósito, para que crear no implique
 * publicar sin decirlo.
 */
pub fn push_tag_operation(
    repo_path: &Path,
    tag_name: &str,
    options: &RemoteTagOptions,
) -> MutationResult {
    let remote = options.remote.as_str();
    let tag_ref = format!("refs/tags/{tag_name}");

    run_tag_command(
        repo_path,
        Operation::PushTag,
        &authenticated_git(repo_path, options.auth_token.as_deref()),
        &["push", remote, tag_ref.as_str()],
        format!("Pushed tag {tag_name} to {remote}."),
        MutationExtra {
            tag: Some(tag_name.to_string()),
            remote: Some(remote.to_string()),
            ..Default::default()
        },
    )
}

/// `deleteRemoteTagOperation`: borra el tag en el remoto.
pub fn delete_remote_tag_operation(
    repo_path: &Path,
    tag_name: &str,
    options: &RemoteTagOptions,
) -> MutationResult {
    let remote = options.remote.as_str();
    let tag_ref = format!("refs/tags/{tag_name}");

    run_tag_command(
        repo_path,
        Operation::DeleteRemoteTag,
        &authenticated_git(repo_path, options.auth_token.as_deref()),
        &["push", remote, "--delete", tag_ref.as_str()],
        format!("Deleted tag {tag_name} on {remote}."),
        MutationExtra {
            tag: Some(tag_name.to_string()),
            remote: Some(remote.to_string()),
            ..Default::default()
        },
    )
}

/// `deleteTagOperation`: borra el tag local.
pub fn delete_tag_operation(repo_path: &Path, tag_name: &str) -> MutationResult {
    run_tag_command(
        repo_path,
        Operation::DeleteTag,
        &git(repo_path),
        &["tag", "-d", tag_name],
        format!("Deleted tag {tag_name}."),
        MutationExtra {
            tag: Some(tag_name.to_string()),
            ..Default::default()
        },
    )
}

/// `checkoutTagOperation`: deja HEAD desanclado en el commit del tag.
pub fn checkout_tag_operation(repo_path: &Path, tag_name: &str) -> MutationResult {
    let extra = MutationExtra {
        tag: Some(tag_name.to_string()),
        ..Default::default()
    };

    match raw(&git(repo_path), &["checkout", tag_name]) {
        Ok(_) => {
            let status = safe_get_status(repo_path);
            create_mutation_result(
                MutationPayload::new(
                    true,
                    Operation::CheckoutTag,
                    Outcome::Completed,
                    Code::DetachedHead,
                    format!("Checked out tag {tag_name} in detached HEAD state."),
                )
                .with(extra),
                Some(status),
            )
        }
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::CheckoutTag, &error.into(), extra)
        }
    }
}
