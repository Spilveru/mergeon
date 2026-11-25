//! Canales de mutación con lógica propia en `git-handlers.js`:
//! `git:initRepository`, `git:addToIgnore`, `git:setupTracking` y `git:clone`.

use std::io::Write;
use std::path::Path;

use mergeon_git_ops::Outcome;
use mergeon_git_query::{
    RepositoryKind, assert_path_inside_repository, inspect_repository, normalize_repo_relative_path,
};
use mergeon_gitcmd::credentials::redact_url_credentials;
use mergeon_gitcmd::{ControlCode, Git, GitError, NETWORK_TIMEOUT, operation_control_code};
use mergeon_runtime::{MutationError, MutationOptions, OutcomeReport};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::exec::{self, node_fs_error, path_text, realpath, resolve_path};
use super::mutations::{optional, or_null};
use super::{GitApi, MutationConfig};
use crate::mutation::{
    Mutation, StructuredResult, Thrown, build_mutation_request_key, control_message,
    create_operation_id,
};

/// Enum de strings: cada variante se serializa con su texto y lo expone en `as_str()`.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $text:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant,)+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    /// `operation` de los canales con lógica propia.
    pub enum HandlerOperation {
        InitRepository => "init-repository",
        AddToIgnore => "add-to-ignore",
        SetupTracking => "setup-tracking",
        Clone => "clone",
    }
}

string_enum! {
    /// `code` de `git:initRepository`.
    pub enum InitRepositoryCode {
        Ok => "OK",
        AlreadyInitialized => "ALREADY_INITIALIZED",
        MissingPath => "MISSING_PATH",
        AccessError => "ACCESS_ERROR",
    }
}

string_enum! {
    /// `code` de `git:addToIgnore`.
    pub enum AddToIgnoreCode {
        Ok => "OK",
        AlreadyIgnored => "ALREADY_IGNORED",
    }
}

string_enum! {
    /// `code` de `git:setupTracking`.
    pub enum SetupTrackingCode {
        Ok => "OK",
        NoBranchSpecified => "NO_BRANCH_SPECIFIED",
        AlreadyTracking => "ALREADY_TRACKING",
        RemoteBranchNotFound => "REMOTE_BRANCH_NOT_FOUND",
    }
}

string_enum! {
    /// `code` de `git:clone`.
    pub enum CloneCode {
        Ok => "OK",
        ArgumentsRequired => "ARGUMENTS_REQUIRED",
        GitError => "GIT_ERROR",
        IpcError => "IPC_ERROR",
        Cancelled => "CANCELLED",
        Timeout => "TIMEOUT",
    }
}

impl From<ControlCode> for CloneCode {
    fn from(code: ControlCode) -> Self {
        match code {
            ControlCode::Cancelled => CloneCode::Cancelled,
            ControlCode::Timeout => CloneCode::Timeout,
        }
    }
}

/// Resultado de `git:initRepository`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitRepositoryResult {
    pub success: bool,
    pub operation: HandlerOperation,
    pub outcome: Outcome,
    pub code: InitRepositoryCode,
    pub message: String,
}

/// Resultado de `git:addToIgnore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddToIgnoreResult {
    pub success: bool,
    pub operation: HandlerOperation,
    pub outcome: Outcome,
    pub code: AddToIgnoreCode,
    pub message: String,
    /// La ruta ya normalizada, tal como quedó en `.gitignore`.
    pub file_path: String,
}

/// Resultado de `git:setupTracking`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupTrackingResult {
    pub success: bool,
    pub operation: HandlerOperation,
    pub outcome: Outcome,
    pub code: SetupTrackingCode,
    pub message: String,
    pub branch: Option<String>,
    /// La rama remota: la que ya se seguía o la que se configuró.
    pub remote: Option<String>,
    pub upstream_set: bool,
    pub already_tracked: bool,
    /// Solo viene (y en `false`) cuando no existe la rama remota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_remote: Option<bool>,
}

/// Resultado de `git:clone`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneResult {
    pub success: bool,
    pub operation: HandlerOperation,
    pub outcome: Outcome,
    pub code: CloneCode,
    pub message: String,
    /// Igual que `canonical_path`.
    pub path: Option<String>,
    /// Ruta real del clon; si falló, el destino pedido ya resuelto.
    pub canonical_path: Option<String>,
    /// La operación en la cola; no viene si los argumentos no eran válidos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

macro_rules! structured_result {
    ($name:ident) => {
        impl StructuredResult for $name {
            fn success(&self) -> bool {
                self.success
            }
            fn operation(&self) -> Option<&str> {
                Some(self.operation.as_str())
            }
            fn outcome(&self) -> Option<Outcome> {
                Some(self.outcome)
            }
            fn code(&self) -> &str {
                self.code.as_str()
            }
            fn message(&self) -> &str {
                &self.message
            }
        }
    };
}

structured_result!(InitRepositoryResult);
structured_result!(AddToIgnoreResult);
structured_result!(SetupTrackingResult);
structured_result!(CloneResult);

fn init_repository(repo_path: &Path) -> Result<InitRepositoryResult, Thrown> {
    let result = |success, outcome, code, message: &str| InitRepositoryResult {
        success,
        operation: HandlerOperation::InitRepository,
        outcome,
        code,
        message: message.to_string(),
    };

    let inspection = inspect_repository(repo_path);

    if inspection.valid {
        return Ok(result(
            true,
            Outcome::Noop,
            InitRepositoryCode::AlreadyInitialized,
            "Git repository already initialized.",
        ));
    }

    match inspection.kind {
        RepositoryKind::MissingPath => {
            return Ok(result(
                false,
                Outcome::Failed,
                InitRepositoryCode::MissingPath,
                &inspection.message,
            ));
        }
        RepositoryKind::AccessError => {
            return Ok(result(
                false,
                Outcome::Failed,
                InitRepositoryCode::AccessError,
                &inspection.message,
            ));
        }
        _ => {}
    }

    exec::raw(&exec::open(repo_path)?, &["init"])?;
    Ok(result(
        true,
        Outcome::Completed,
        InitRepositoryCode::Ok,
        "Initialized Git repository.",
    ))
}

fn add_to_ignore(repo_path: &Path, file_path: &str) -> Result<AddToIgnoreResult, Thrown> {
    let relative_path = normalize_repo_relative_path(file_path)?;
    assert_path_inside_repository(repo_path, &relative_path)?;
    let gitignore_path = repo_path.join(".gitignore");

    let content = match std::fs::read(&gitignore_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(node_fs_error(&error, "open", &gitignore_path)),
    };

    let already_ignored = content
        .split('\n')
        .map(|entry| entry.strip_suffix('\r').unwrap_or(entry))
        .any(|entry| js_trim(entry) == relative_path);
    if already_ignored {
        return Ok(AddToIgnoreResult {
            success: true,
            operation: HandlerOperation::AddToIgnore,
            outcome: Outcome::Noop,
            code: AddToIgnoreCode::AlreadyIgnored,
            message: format!("{relative_path} is already ignored."),
            file_path: relative_path,
        });
    }

    let prefix = if !content.is_empty() && !content.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&gitignore_path)
        .and_then(|mut file| file.write_all(format!("{prefix}{relative_path}\n").as_bytes()))
        .map_err(|error| node_fs_error(&error, "open", &gitignore_path))?;

    Ok(AddToIgnoreResult {
        success: true,
        operation: HandlerOperation::AddToIgnore,
        outcome: Outcome::Completed,
        code: AddToIgnoreCode::Ok,
        message: format!("Added {relative_path} to .gitignore."),
        file_path: relative_path,
    })
}

/// `String.prototype.trim`: además del espacio en blanco Unicode, quita el BOM.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}')
}

/// `git.branch()` del original. Allí se leía `tracking` del resumen de
/// simple-git, que no trae ese campo; aquí sale de la vista de ramas de
/// `mergeon-git-query` (el mismo `git branch -v -a` más el upstream de cada
/// rama), que es la forma que el handler esperaba.
fn list_branches(repo_path: &Path) -> Result<mergeon_git_query::BranchesOverview, Thrown> {
    const PREFIX: &str = "Failed to get branches: ";
    mergeon_git_query::get_branches(repo_path).map_err(|error| {
        let message = error.message();
        Thrown::new(message.strip_prefix(PREFIX).unwrap_or(message))
    })
}

fn setup_tracking(
    repo_path: &Path,
    branch_name: Option<&str>,
) -> Result<SetupTrackingResult, Thrown> {
    let git = exec::open(repo_path)?;

    // Si no se proporciona branchName, usar la rama actual
    let target_branch = match branch_name.filter(|name| !name.is_empty()) {
        Some(name) => Some(name.to_string()),
        None => git.status()?.current.filter(|name| !name.is_empty()),
    };

    let Some(target_branch) = target_branch else {
        return Ok(SetupTrackingResult {
            success: false,
            operation: HandlerOperation::SetupTracking,
            outcome: Outcome::Blocked,
            code: SetupTrackingCode::NoBranchSpecified,
            message: "No branch specified.".to_string(),
            branch: None,
            remote: None,
            upstream_set: false,
            already_tracked: false,
            has_remote: None,
        });
    };

    // Verificar si ya tiene tracking
    let branches = list_branches(repo_path)?;
    let tracking = branches
        .branch(&target_branch)
        .and_then(|branch| branch.tracking.clone())
        .filter(|tracking| !tracking.is_empty());

    if let Some(tracking) = tracking {
        return Ok(SetupTrackingResult {
            success: true,
            operation: HandlerOperation::SetupTracking,
            outcome: Outcome::Noop,
            code: SetupTrackingCode::AlreadyTracking,
            message: format!("{target_branch} already tracks {tracking}."),
            branch: Some(target_branch),
            remote: Some(tracking),
            upstream_set: true,
            already_tracked: true,
            has_remote: None,
        });
    }

    // Verificar si existe rama remota con el mismo nombre
    let remote_branch = format!("origin/{target_branch}");
    let has_remote = branches
        .all
        .iter()
        .any(|name| *name == format!("remotes/{remote_branch}"));

    if !has_remote {
        return Ok(SetupTrackingResult {
            success: false,
            operation: HandlerOperation::SetupTracking,
            outcome: Outcome::Blocked,
            code: SetupTrackingCode::RemoteBranchNotFound,
            message: format!("No remote branch found for {remote_branch}."),
            branch: Some(target_branch),
            remote: Some(remote_branch),
            upstream_set: false,
            already_tracked: false,
            has_remote: Some(false),
        });
    }

    // Configurar tracking usando git branch --set-upstream-to
    exec::raw(
        &git,
        &[
            "branch",
            "--set-upstream-to",
            &remote_branch,
            &target_branch,
        ],
    )?;

    Ok(SetupTrackingResult {
        success: true,
        operation: HandlerOperation::SetupTracking,
        outcome: Outcome::Completed,
        code: SetupTrackingCode::Ok,
        message: format!("Tracking set up for {target_branch} -> {remote_branch}."),
        branch: Some(target_branch),
        remote: Some(remote_branch),
        upstream_set: true,
        already_tracked: false,
        has_remote: None,
    })
}

/// `outcome`, `code` y `message` de un clone que no terminó bien.
fn clone_failure_verdict(
    control_code: Option<ControlCode>,
    fallback_code: CloneCode,
    error_message: &str,
) -> (Outcome, CloneCode, String) {
    match control_code {
        Some(code) => (code.into(), code.into(), control_message(code).to_string()),
        // Una URL con credenciales dentro (`https://user:token@host/…`) no
        // debe llegar entera al aviso ni al registro de operaciones.
        None => (
            Outcome::Failed,
            fallback_code,
            format!(
                "Failed to clone repository: {}",
                redact_url_credentials(error_message)
            ),
        ),
    }
}

/// El cuerpo de la tarea de clone: nunca falla, todo error queda en el resultado.
fn run_clone(
    repo_url: &str,
    target_path: &str,
    auth_token: Option<&str>,
    operation_id: &str,
) -> CloneResult {
    // Sin directorio de trabajo: el destino todavía no existe. `--` separa
    // las rutas igual que hace simple-git con sus pathspecs.
    // `--progress`: el avance llega a la cola (y a la UI) aunque stderr no
    // sea una terminal; el runner lo quita del stderr del resultado.
    let cloned: Result<String, GitError> = Git::without_cwd()
        .with_github_token(auth_token)
        .with_progress()
        .raw(["clone", "--progress", "--", repo_url, target_path]);

    match cloned {
        Ok(_) => {
            // Remote is automatically saved as clean URL without token.
            let target = Path::new(target_path);
            // Keep the resolved target when the filesystem cannot resolve it.
            let canonical_path =
                path_text(&realpath(target).unwrap_or_else(|_| resolve_path(target)));
            CloneResult {
                success: true,
                operation: HandlerOperation::Clone,
                outcome: Outcome::Completed,
                code: CloneCode::Ok,
                message: "Repository cloned successfully.".to_string(),
                path: Some(canonical_path.clone()),
                canonical_path: Some(canonical_path),
                operation_id: Some(operation_id.to_string()),
            }
        }
        Err(error) => {
            log::error!(
                "Error cloning repository: {}",
                redact_url_credentials(&error.message)
            );
            let (outcome, code, message) = clone_failure_verdict(
                operation_control_code(Some(&error)),
                CloneCode::GitError,
                &error.message,
            );
            CloneResult {
                success: false,
                operation: HandlerOperation::Clone,
                outcome,
                code,
                message,
                path: Some(target_path.to_string()),
                canonical_path: Some(path_text(&resolve_path(Path::new(target_path)))),
                operation_id: Some(operation_id.to_string()),
            }
        }
    }
}

impl GitApi {
    /// Canal `git:initRepository`.
    pub fn init_repository(&self, repo_path: impl AsRef<Path>) -> Mutation<InitRepositoryResult> {
        let repo = repo_path.as_ref();
        self.mutate(
            repo,
            MutationConfig::new(
                "init-repository",
                "Failed to initialize Git repository",
                json!([path_text(repo)]),
            ),
            init_repository,
        )
    }

    /// Canal `git:addToIgnore`: añade `file_path` al `.gitignore` de la raíz.
    pub fn add_to_ignore(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
    ) -> Mutation<AddToIgnoreResult> {
        let repo = repo_path.as_ref();
        let file_arg = file_path.to_string();
        self.mutate(
            repo,
            MutationConfig::new(
                "add-to-ignore",
                "Failed to update .gitignore",
                json!([file_path]),
            )
            .extra(json!({ "filePath": or_null(file_path) })),
            move |repo| add_to_ignore(repo, &file_arg),
        )
    }

    /// Canal `git:setupTracking`: hace que la rama siga a `origin/<rama>` si
    /// esa rama remota existe. Sin `branch_name` usa la rama actual.
    pub fn setup_tracking(
        &self,
        repo_path: impl AsRef<Path>,
        branch_name: Option<&str>,
    ) -> Mutation<SetupTrackingResult> {
        let repo = repo_path.as_ref();
        let branch_arg = branch_name.map(str::to_string);
        self.mutate(
            repo,
            MutationConfig::new(
                "setup-tracking",
                "Failed to set up tracking",
                json!([optional(branch_name)]),
            )
            .extra(json!({ "branch": or_null(branch_name.unwrap_or_default()) })),
            move |repo| setup_tracking(repo, branch_arg.as_deref()),
        )
    }

    /// Canal `git:clone`. `auth_token` es el token de GitHub de la cuenta
    /// activa (el `authContext` del original). La cola es la del destino.
    pub fn clone(
        &self,
        repo_url: &str,
        target_path: &str,
        auth_token: Option<&str>,
    ) -> CloneResult {
        let normalized_target_path = js_trim(target_path).to_string();
        let normalized_repo_url = js_trim(repo_url).trim_end_matches('/').to_string();

        if normalized_target_path.is_empty() || normalized_repo_url.is_empty() {
            let path = (!normalized_target_path.is_empty()).then(|| normalized_target_path.clone());
            return CloneResult {
                success: false,
                operation: HandlerOperation::Clone,
                outcome: Outcome::Blocked,
                code: CloneCode::ArgumentsRequired,
                message: "A repository URL and clone destination are required.".to_string(),
                canonical_path: path
                    .as_deref()
                    .map(|path| path_text(&resolve_path(Path::new(path)))),
                path,
                operation_id: None,
            };
        }

        let operation_id = create_operation_id("clone");
        // El token viaja como objeto para que en la clave solo quede su huella.
        let request_key = build_mutation_request_key(
            "clone",
            &json!([
                normalized_repo_url,
                normalized_target_path,
                auth_token.map(|token| json!({ "token": token }))
            ]),
        );
        let options = MutationOptions::new()
            .operation("clone")
            .operation_id(operation_id.clone())
            .timeout(NETWORK_TIMEOUT)
            .watch(false);

        let task = {
            let repo_url = normalized_repo_url;
            let target_path = normalized_target_path.clone();
            let auth_token = auth_token.map(str::to_string);
            let operation_id = operation_id.clone();
            move || {
                run_clone(
                    &repo_url,
                    &target_path,
                    auth_token.as_deref(),
                    &operation_id,
                )
            }
        };

        let outcome = self.gate.run_exclusive_with_report(
            &normalized_target_path,
            Some(&request_key),
            options,
            task,
            |result: &CloneResult| {
                OutcomeReport::from_fields(
                    result.success,
                    Some(result.code.as_str()),
                    Some(&result.message),
                )
            },
        );

        match outcome {
            Ok(result) => result,
            // La cola rechazó la solicitud: cancelada antes de arrancar, o la
            // tarea no llegó a devolver un resultado.
            Err(error) => {
                let error_message = match &error {
                    MutationError::Control { code, .. } => code.message().to_string(),
                    MutationError::Panicked { message, .. } if !message.is_empty() => {
                        message.clone()
                    }
                    MutationError::Panicked { .. } => "Unable to clone repository.".to_string(),
                };
                let (outcome, code, message) = clone_failure_verdict(
                    error.control_code(),
                    CloneCode::IpcError,
                    &error_message,
                );
                CloneResult {
                    success: false,
                    operation: HandlerOperation::Clone,
                    outcome,
                    code,
                    message,
                    canonical_path: Some(path_text(&resolve_path(Path::new(
                        &normalized_target_path,
                    )))),
                    path: Some(normalized_target_path),
                    operation_id: Some(error.operation_id().map_or(operation_id, str::to_string)),
                }
            }
        }
    }
}
