//! Usuario y contraseña (o token) para remotos HTTPS que no son de GitHub
//! (GitLab, Bitbucket, Azure DevOps, servidores propios).
//!
//! Git corre sin terminal (`GIT_TERMINAL_PROMPT=0`): si ningún credential
//! helper del usuario contesta, fetch, pull, push y clone fallan con
//! "could not read Username…" o "Authentication failed…".
//! [`AppModel::with_http_credentials`] envuelve esas operaciones: si fallan por
//! eso, pide usuario y contraseña con [`crate::UiHost::ask_credentials`], los
//! deja en memoria para ese origen (`https://host[:puerto]`) y repite la
//! operación. Cómo llegan a git sin pasar por sus argumentos está en
//! `mergeon_gitcmd::credentials`.
//!
//! Las credenciales viven mientras siga abierta alguna pestaña que las usó
//! (o el destino de un clone); al cerrar la última se borran de memoria.
//! Nunca van al log de operaciones ni a los avisos. Con "Remember" (solo si
//! hay un credential helper configurado) se guardan además con
//! `git credential approve`, cuando la operación ya funcionó con ellas.
//!
//! El token de la cuenta de GitHub sigue su propio camino (cabecera HTTP solo
//! para github.com): con una cuenta conectada no se piden credenciales para
//! github.com.

use std::collections::{BTreeSet, HashMap, HashSet};

use gpui_kit::{Context, Task};
use mergeon_gitcmd::GITHUB_ORIGIN;
use mergeon_gitcmd::credentials::{auth_failure_origin, origin_host, quoted_url, url_username};
use mergeon_model::notify::{NotifyOptions, NotifyType};
use mergeon_model::operation_types::OperationResult;

use crate::AppModel;
use crate::convert::StructuredOutcome;
use crate::tasks::flow;

/// Veces que se piden credenciales para una misma operación antes de darla
/// por fallida.
pub const MAX_CREDENTIAL_PROMPTS: usize = 3;

/// Lo que el diálogo de credenciales necesita saber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsRequest {
    /// `https://host[:puerto]`.
    pub origin: String,
    /// `host[:puerto]`, lo que se enseña.
    pub host: String,
    /// Operación que las pidió (`fetch`, `pull`, `push`, `clone`).
    pub operation: String,
    /// Usuario propuesto: el de la URL o el último tecleado para ese origen.
    pub username: Option<String>,
    /// Las anteriores no valieron.
    pub refused: bool,
    /// Hay un credential helper configurado: se ofrece "Remember".
    pub can_remember: bool,
}

/// Lo que el usuario tecleó. El `Debug` no enseña la contraseña y al soltarse
/// se borra de memoria.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialsAnswer {
    pub username: String,
    pub password: String,
    pub remember: bool,
}

impl std::fmt::Debug for CredentialsAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialsAnswer")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("remember", &self.remember)
            .finish()
    }
}

impl Drop for CredentialsAnswer {
    fn drop(&mut self) {
        mergeon_gitcmd::credentials::wipe(&mut self.password);
    }
}

/// Qué orígenes tienen credenciales en la sesión y quién las usa.
#[derive(Debug, Default)]
pub(crate) struct CredentialState {
    /// Origen → rutas de las pestañas (o destinos de clone) que las usaron.
    owners: HashMap<String, BTreeSet<String>>,
    /// Último usuario tecleado por origen, para proponerlo.
    usernames: HashMap<String, String>,
    /// Avisos de operaciones que, si fallan por credenciales, se resuelven
    /// con el diálogo en vez de con un toast de error.
    prompting: HashSet<String>,
}

/// Clave del aviso de una operación (`invoke_structured_operation`).
pub(crate) fn operation_toast_key(operation: &str, repo_path: &str) -> String {
    format!("operation:{operation}:{repo_path}")
}

impl AppModel {
    /// Origen al que hay que pedirle credenciales tras `result`, o `None` si
    /// el fallo no es de credenciales HTTP (o es github.com con una cuenta
    /// conectada, que usa su token).
    pub fn credentials_prompt_origin(&self, result: &OperationResult) -> Option<String> {
        if result.success || self.ui_host.is_none() {
            return None;
        }
        let origin = auth_failure_origin(result.message.as_deref()?)?;
        if origin == GITHUB_ORIGIN && self.git_auth_token().is_some() {
            return None;
        }
        Some(origin)
    }

    /// El aviso `toast_key` es de una operación que pedirá credenciales si
    /// falla por ellas: su toast de error sobra.
    pub(crate) fn suppresses_credentials_toast(
        &self,
        toast_key: &str,
        result: &OperationResult,
    ) -> bool {
        self.credentials.prompting.contains(toast_key)
            && self.credentials_prompt_origin(result).is_some()
    }

    /// Orígenes con credenciales en la sesión (tests y diagnóstico).
    pub fn http_credential_origins(&self) -> Vec<String> {
        let mut origins: Vec<String> = self.credentials.owners.keys().cloned().collect();
        origins.sort();
        origins
    }

    /// Una pestaña se cerró: sus credenciales se olvidan si nadie más las usa.
    pub(crate) fn release_http_credentials(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        let mut released = Vec::new();
        self.credentials.owners.retain(|origin, owners| {
            owners.remove(repo_path);
            if owners.is_empty() {
                released.push(origin.clone());
                false
            } else {
                true
            }
        });
        if released.is_empty() {
            return;
        }
        for origin in &released {
            self.credentials.usernames.remove(origin);
        }
        self.blocking(cx, move |backend| {
            for origin in &released {
                backend.git().forget_http_credentials(origin);
            }
        })
        .detach();
    }

    /// Ejecuta `attempt` (fetch, pull, push o clone de `repo_path`) y, si
    /// falla porque el remoto HTTPS pide usuario y contraseña, los pide y la
    /// repite (hasta [`MAX_CREDENTIAL_PROMPTS`] veces). `owner` es la pestaña
    /// a la que quedan ligadas las credenciales. Si el usuario cancela, se
    /// avisa del fallo original.
    pub(crate) fn with_http_credentials<T>(
        &mut self,
        operation: &'static str,
        repo_path: &str,
        owner: &str,
        attempt: impl Fn(&mut Self, &mut Context<Self>) -> Task<T> + 'static,
        cx: &mut Context<Self>,
    ) -> Task<T>
    where
        T: StructuredOutcome + 'static,
    {
        let toast_key = operation_toast_key(operation, repo_path);
        let repo_path = repo_path.to_string();
        let owner = owner.to_string();
        self.credentials.prompting.insert(toast_key.clone());

        flow(cx, async move |this, cx| {
            let mut prompts = 0;
            // Lo que hay que guardar con el helper del usuario si la operación
            // funciona (se borra de memoria al soltarse).
            let mut remember = None::<(String, CredentialsAnswer)>;
            let result = loop {
                let result = this.update(cx, |this, cx| attempt(this, cx)).await;
                let summary = result.to_operation_result();
                let origin = this.read_with(cx, |this, _| this.credentials_prompt_origin(&summary));

                let Some(origin) = origin else {
                    if summary.success
                        && let Some((origin, answer)) = remember.take()
                    {
                        let approve = this.update(cx, |this, cx| {
                            let repo_path = repo_path.clone();
                            this.blocking(cx, move |backend| {
                                let config_path = Some(repo_path.as_str())
                                    .filter(|path| std::path::Path::new(path).exists());
                                backend.git().approve_http_credentials(
                                    config_path,
                                    &origin,
                                    &answer.username,
                                    &answer.password,
                                )
                            })
                        });
                        if let Err(error) = approve.await {
                            this.update(cx, |this, cx| {
                                this.notify(
                                    NotifyType::Warning,
                                    "Couldn't save the credentials with Git's credential helper",
                                    NotifyOptions {
                                        details: Some(error),
                                        ..Default::default()
                                    },
                                    cx,
                                );
                            });
                        }
                    }
                    break result;
                };

                if prompts >= MAX_CREDENTIAL_PROMPTS {
                    this.update(cx, |this, cx| this.forget_origin(&origin, cx));
                    break this.update(cx, |this, cx| {
                        this.credentials.prompting.remove(&toast_key);
                        this.show_credentials_failure(operation, &repo_path, &summary, cx);
                        result
                    });
                }
                prompts += 1;

                // Si ya había credenciales para ese origen, se usaron y no valieron.
                let refused =
                    this.read_with(cx, |this, _| this.credentials.owners.contains_key(&origin));
                let config_path = repo_path.clone();
                let helper_origin = origin.clone();
                let can_remember = this
                    .update(cx, |this, cx| {
                        this.blocking(cx, move |backend| {
                            let path = Some(config_path.as_str())
                                .filter(|path| std::path::Path::new(path).join(".git").exists());
                            backend.git().has_credential_helper(path, &helper_origin)
                        })
                    })
                    .await;
                let username = this.read_with(cx, |this, _| {
                    this.credentials
                        .usernames
                        .get(&origin)
                        .cloned()
                        .or_else(|| {
                            summary
                                .message
                                .as_deref()
                                .and_then(quoted_url)
                                .and_then(|url| url_username(&url))
                        })
                });
                let request = CredentialsRequest {
                    host: origin_host(&origin).to_string(),
                    origin: origin.clone(),
                    operation: operation.to_string(),
                    username,
                    refused,
                    can_remember,
                };
                let answer = this
                    .update(cx, |this, cx| this.ask_credentials(request, cx))
                    .await;

                let Some(answer) = answer.filter(|answer| !answer.username.is_empty()) else {
                    // Cancelado: aviso claro en lugar del error crudo de git.
                    break this.update(cx, |this, cx| {
                        this.credentials.prompting.remove(&toast_key);
                        this.show_credentials_cancelled(
                            operation,
                            &repo_path,
                            origin_host(&origin),
                            &summary,
                            cx,
                        );
                        result
                    });
                };

                let (set_origin, username, password) = (
                    origin.clone(),
                    answer.username.clone(),
                    answer.password.clone(),
                );
                let stored = this.update(cx, |this, cx| {
                    this.credentials
                        .owners
                        .entry(origin.clone())
                        .or_default()
                        .insert(owner.clone());
                    this.credentials
                        .usernames
                        .insert(origin.clone(), answer.username.clone());
                    this.blocking(cx, move |backend| {
                        let mut password = password;
                        backend
                            .git()
                            .set_http_credentials(&set_origin, &username, &password);
                        mergeon_gitcmd::credentials::wipe(&mut password);
                    })
                });
                stored.await;
                remember = answer.remember.then(|| (origin.clone(), answer.clone()));
            };
            this.update(cx, |this, _| {
                this.credentials.prompting.remove(&toast_key);
            });
            result
        })
    }

    fn forget_origin(&mut self, origin: &str, cx: &mut Context<Self>) {
        self.credentials.owners.remove(origin);
        let origin = origin.to_string();
        self.blocking(cx, move |backend| {
            backend.git().forget_http_credentials(&origin);
        })
        .detach();
    }

    /// El aviso de error que se calló mientras se pedían credenciales.
    fn show_credentials_failure(
        &mut self,
        operation: &str,
        repo_path: &str,
        summary: &OperationResult,
        cx: &mut Context<Self>,
    ) {
        let toast =
            mergeon_model::notify::ToastId::Named(operation_toast_key(operation, repo_path));
        self.notify_git_result(
            crate::git_client::NotifyMode::Operation,
            repo_path,
            summary,
            toast,
            cx,
        );
    }

    /// El usuario canceló el diálogo de inicio de sesión: mensaje claro en vez
    /// del stderr técnico de git (`terminal prompts disabled`).
    fn show_credentials_cancelled(
        &mut self,
        operation: &str,
        repo_path: &str,
        host: &str,
        summary: &OperationResult,
        cx: &mut Context<Self>,
    ) {
        let message =
            self.message_for_repository(repo_path, &format!("Sign-in to {host} cancelled."));
        let toast =
            mergeon_model::notify::ToastId::Named(operation_toast_key(operation, repo_path));
        self.notify(
            NotifyType::Warning,
            &message,
            NotifyOptions {
                id: Some(toast),
                details: summary.message.clone().filter(|text| !text.is_empty()),
                ..Default::default()
            },
            cx,
        );
    }
}
