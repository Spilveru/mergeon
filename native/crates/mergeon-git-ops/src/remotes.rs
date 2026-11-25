//! Configuración de remotos y actualización de submódulos.

use std::path::Path;

use crate::exec::{git, raw};
use crate::messages::js_trim;
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, finalize_mutation_result,
    non_empty, require_args,
};
use crate::types::{MutationResult, Operation, SubmoduleUpdateOptions};

/// Ejecuta el comando de la operación y arma el resultado con `extra`, o con
/// `error_extra` si falla.
fn run_command(
    repo_path: &Path,
    operation: Operation,
    arguments: &[&str],
    success_message: String,
    extra: MutationExtra,
    error_extra: MutationExtra,
) -> MutationResult {
    match raw(&git(repo_path), arguments) {
        Ok(_) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(operation, success_message).with(extra),
        ),
        Err(error) => build_mutation_error_result(repo_path, operation, &error.into(), error_extra),
    }
}

fn remote_extra(remote: &str) -> MutationExtra {
    MutationExtra {
        remote: non_empty(remote),
        ..Default::default()
    }
}

/// `addRemoteOperation`.
pub fn add_remote_operation(repo_path: &Path, name: &str, url: &str) -> MutationResult {
    let [remote, url] = match require_args(
        repo_path,
        Operation::AddRemote,
        [name, url],
        "A remote name and URL are required.",
        |[remote, _]| remote_extra(remote),
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    run_command(
        repo_path,
        Operation::AddRemote,
        &["remote", "add", "--", remote.as_str(), url.as_str()],
        format!("Remote '{remote}' added successfully."),
        remote_extra(&remote),
        remote_extra(&remote),
    )
}

/// `removeRemoteOperation`.
pub fn remove_remote_operation(repo_path: &Path, name: &str) -> MutationResult {
    let [remote] = match require_args(
        repo_path,
        Operation::RemoveRemote,
        [name],
        "A remote name is required.",
        |[remote]| remote_extra(remote),
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    run_command(
        repo_path,
        Operation::RemoveRemote,
        &["remote", "remove", remote.as_str()],
        format!("Remote '{remote}' removed."),
        remote_extra(&remote),
        remote_extra(&remote),
    )
}

/// `renameRemoteOperation`.
pub fn rename_remote_operation(repo_path: &Path, name: &str, new_name: &str) -> MutationResult {
    let [remote, new_name] = match require_args(
        repo_path,
        Operation::RenameRemote,
        [name, new_name],
        "The current and new remote names are required.",
        |[remote, _]| remote_extra(remote),
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    run_command(
        repo_path,
        Operation::RenameRemote,
        &["remote", "rename", remote.as_str(), new_name.as_str()],
        format!("Remote '{remote}' renamed to '{new_name}'."),
        remote_extra(&new_name),
        remote_extra(&remote),
    )
}

/// `setRemoteUrlOperation`.
pub fn set_remote_url_operation(repo_path: &Path, name: &str, url: &str) -> MutationResult {
    let [remote, url] = match require_args(
        repo_path,
        Operation::SetRemoteUrl,
        [name, url],
        "A remote name and URL are required.",
        |[remote, _]| remote_extra(remote),
    ) {
        Ok(values) => values,
        Err(result) => return *result,
    };

    run_command(
        repo_path,
        Operation::SetRemoteUrl,
        &["remote", "set-url", remote.as_str(), url.as_str()],
        format!("Updated the URL of remote '{remote}'."),
        remote_extra(&remote),
        remote_extra(&remote),
    )
}

/*
 * `submodule update --init` cubre los dos gestos de la UI: inicializar un
 * submódulo nuevo y sincronizar uno existente al commit registrado. Puede
 * clonar por red, así que quien la lanza le da el timeout de operaciones de red.
 */
pub fn submodule_update_operation(
    repo_path: &Path,
    options: &SubmoduleUpdateOptions,
) -> MutationResult {
    let paths: Vec<String> = options
        .paths
        .iter()
        .map(|path| js_trim(path).to_string())
        .filter(|path| !path.is_empty())
        .collect();

    let mut arguments = vec!["submodule", "update", "--init"];
    if !paths.is_empty() {
        arguments.push("--");
        arguments.extend(paths.iter().map(String::as_str));
    }

    let extra = MutationExtra {
        files_affected: paths.clone(),
        ..Default::default()
    };
    let success_message = if paths.is_empty() {
        "Updated all submodules.".to_string()
    } else {
        format!(
            "Updated {} submodule{}.",
            paths.len(),
            if paths.len() != 1 { "s" } else { "" }
        )
    };

    run_command(
        repo_path,
        Operation::SubmoduleUpdate,
        &arguments,
        success_message,
        extra.clone(),
        extra,
    )
}
