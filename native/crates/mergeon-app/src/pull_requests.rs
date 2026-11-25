//! Pull requests y checks de GitHub del repositorio (solo consulta): sin
//! equivalente en la app Electron.
//!
//! Por repositorio abierto se guarda la lista de PRs abiertos, el PR de la
//! rama actual y, de cada uno, sus checks y su revisión. Todo se pide en
//! segundo plano con el token de la cuenta activa, con peticiones
//! condicionales (`ETag`): una caché de respuestas por cuenta.
//!
//! Cuándo se refresca ([`AppModel::watch_pull_requests`], que llaman las
//! vistas que lo pintan):
//!
//! - al terminar un fetch, pull o push del repositorio activo;
//! - al cambiar de rama y al cambiar de cuenta (un upstream nuevo llega con
//!   el push que lo crea, que también refresca);
//! - al cambiar de repositorio, si lo que se tiene es de hace más de
//!   [`PULL_REQUESTS_REFRESH_INTERVAL`];
//! - cada [`PULL_REQUESTS_REFRESH_INTERVAL`], solo con la ventana enfocada;
//! - con el cupo agotado no se pregunta hasta que se renueva, y entonces se
//!   reintenta solo.
//!
//! Los fallos no avisan con toasts: quedan en [`RepoPullRequests::error`]
//! para que la sección los muestre discretamente.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Context, Subscription, Task};
use mergeon_backend::{FetchPullRequestOptions, PullRequestHead, Remote, pull_request_branch_name};
use mergeon_github::{
    CheckSummary, GitHubClient, GitHubError, GitHubErrorCode, GitHubPullRequest,
    GitHubRepositoryRef, ResponseCache, ReviewSummary, parse_github_repository_url,
};
use mergeon_model::format::now_epoch_ms;

use crate::AppModel;
use crate::convert::StructuredOutcome as _;
use crate::git_client::GitMutation;
use crate::repository_sync::SyncRequest;
use crate::tasks::flow;

/// Cada cuánto se refresca con la ventana enfocada.
pub const PULL_REQUESTS_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// PRs de la lista de los que se piden checks y revisiones (los más
/// recientes); del resto solo se sabe lo que trae el listado.
pub const MAX_DETAILED_PULL_REQUESTS: usize = 30;

/// Cada cuánto mira el vigilante si toca refrescar.
const WATCH_TICK: Duration = Duration::from_secs(60);
/// Un refresco que no es forzado no se repite antes de esto.
const MIN_REFRESH_GAP: Duration = Duration::from_secs(15);
/// Hilos que piden a la vez los detalles de la lista.
const DETAIL_WORKERS: usize = 4;
/// Operaciones tras las que el remoto pudo cambiar.
const REMOTE_OPERATIONS: [&str; 3] = ["fetch", "pull", "push"];

const NETWORK_ERROR: &str = "Can't reach GitHub right now.";
const AUTH_ERROR: &str = "GitHub sign-in expired. Reconnect your account in Settings.";

// ---------------------------------------------------------------------------
// Tipos públicos
// ---------------------------------------------------------------------------

/// Si la función tiene algo que enseñar en este repositorio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullRequestsAvailability {
    /// Aún no se ha mirado.
    #[default]
    Unknown,
    /// Ningún remoto es de github.com (o es de GitHub Enterprise).
    NotGitHub,
    /// La cuenta no ve el repositorio (404/403).
    NoAccess,
    /// Hay remoto de GitHub pero ninguna cuenta con la que preguntar.
    SignedOut,
    Available,
}

impl PullRequestsAvailability {
    /// La sección se pinta (con datos o con "Sign in…").
    pub fn is_visible(self) -> bool {
        matches!(self, Self::SignedOut | Self::Available)
    }
}

/// El remoto de github.com contra el que se pregunta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRemote {
    /// Nombre del remoto en git (`origin`, `upstream`...).
    pub name: String,
    pub repository: GitHubRepositoryRef,
}

impl GitHubRemote {
    /// `https://github.com/<owner>/<repo>`.
    pub fn web_url(&self) -> String {
        format!(
            "https://github.com/{}/{}",
            self.repository.owner, self.repository.repo
        )
    }
}

/// Un PR con sus checks (del último commit) y su revisión, si se pidieron.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestItem {
    pub pull: GitHubPullRequest,
    pub checks: Option<CheckSummary>,
    pub review: Option<ReviewSummary>,
}

/// Lo que se sabe de los PRs de un repositorio.
#[derive(Debug, Clone, Default)]
pub struct RepoPullRequests {
    pub availability: PullRequestsAvailability,
    pub remote: Option<GitHubRemote>,
    /// PRs abiertos, los actualizados más recientemente primero.
    pub pulls: Arc<[PullRequestItem]>,
    /// El PR abierto de [`RepoPullRequests::current_branch`], si lo hay.
    pub current: Option<PullRequestItem>,
    /// La rama para la que se buscó [`RepoPullRequests::current`].
    pub current_branch: Option<String>,
    /// Primera carga en curso (aún no hay nada que enseñar).
    pub loading: bool,
    /// El último fallo, para mostrarlo sin toasts.
    pub error: Option<String>,
    /// Con el cupo agotado: cuándo se renueva (segundos desde la época).
    pub rate_limited_until: Option<u64>,
    /// Cambia (a un valor que no se repite) cada vez que cambia algo de lo
    /// que se pinta.
    pub generation: u64,
    fetched_at: Option<Instant>,
}

impl RepoPullRequests {
    /// El PR de la rama `branch`, si es la que se buscó.
    pub fn current_for(&self, branch: &str) -> Option<&PullRequestItem> {
        (self.current_branch.as_deref() == Some(branch))
            .then_some(self.current.as_ref())
            .flatten()
    }

    /// Un PR por número, de la lista o el de la rama actual.
    pub fn find(&self, number: u64) -> Option<&PullRequestItem> {
        self.pulls
            .iter()
            .chain(self.current.as_ref())
            .find(|item| item.pull.number == number)
    }

    /// Lo que se pinta, para saber si un refresco cambió algo.
    fn same_view(&self, other: &Self) -> bool {
        self.availability == other.availability
            && self.remote == other.remote
            && self.pulls == other.pulls
            && self.current == other.current
            && self.current_branch == other.current_branch
            && self.loading == other.loading
            && self.error == other.error
            && self.rate_limited_until == other.rate_limited_until
    }
}

/// Qué hacer checkout tras traer la rama de un PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRequestCheckout {
    /// La rama vive en el remoto (PR del mismo repositorio).
    RemoteBranch { remote: String, branch: String },
    /// Rama local creada por el fetch (`pr-<n>`, PR de un fork).
    LocalBranch(String),
}

// ---------------------------------------------------------------------------
// Estado interno
// ---------------------------------------------------------------------------

/// Lo que el vigilante compara entre avisos del modelo. Se calcula en cada
/// aviso: nada que recorra ramas o PRs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchKey {
    repo_id: Option<String>,
    branch: Option<String>,
    account: Option<String>,
    remote_busy: bool,
    open_repositories: usize,
}

struct PullRequestWatch {
    _observer: Subscription,
    _ticker: Task<()>,
}

#[derive(Default)]
pub(crate) struct PullRequestsState {
    repos: HashMap<String, RepoPullRequests>,
    /// Respuestas guardadas por cuenta (las respuestas dependen de quién
    /// pregunta).
    caches: HashMap<String, Arc<ResponseCache>>,
    in_flight: HashSet<String>,
    /// Se pidió otro refresco mientras había uno en vuelo.
    pending: HashSet<String>,
    last_key: Option<WatchKey>,
    watch: Option<PullRequestWatch>,
    refreshes: usize,
    generations: u64,
}

impl std::fmt::Debug for PullRequestsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PullRequestsState")
            .field("repos", &self.repos.len())
            .field("in_flight", &self.in_flight)
            .field("watching", &self.watch.is_some())
            .finish_non_exhaustive()
    }
}

/// Lo que trae la primera fase de un refresco.
struct Overview {
    pulls: Vec<GitHubPullRequest>,
    current: Option<PullRequestItem>,
}

/// Checks y revisión de un PR, por número y commit.
type Details = Vec<(u64, String, Option<CheckSummary>, Option<ReviewSummary>)>;

// ---------------------------------------------------------------------------
// Lógica pura
// ---------------------------------------------------------------------------

fn github_ref(remote: &Remote) -> Option<GitHubRepositoryRef> {
    parse_github_repository_url(&remote.refs.fetch)
        .or_else(|| parse_github_repository_url(&remote.refs.push))
}

/// El remoto contra el que se piden los PRs y de dónde sale la rama
/// actual (`owner`, `rama en el remoto`).
///
/// Repositorio del PR: `upstream` si es de GitHub (un fork apunta ahí), si
/// no el remoto del upstream de la rama, si no `origin`, si no el primero de
/// GitHub. La rama actual se busca como `<dueño de su remoto>:<rama
/// remota>` si tiene upstream en GitHub; si no, en `origin` (o en el remoto
/// elegido) con su nombre local.
pub fn pick_github_remote(
    remotes: &[Remote],
    current_branch: Option<&str>,
    upstream: Option<&str>,
) -> Option<(GitHubRemote, Option<(String, String)>)> {
    let github: Vec<(&Remote, GitHubRepositoryRef)> = remotes
        .iter()
        .filter_map(|remote| github_ref(remote).map(|reference| (remote, reference)))
        .collect();
    let by_name = |name: &str| github.iter().find(|(remote, _)| remote.name == name);

    // `origin/feat/x` con remotos que pueden llevar `/`: el más largo que encaje.
    let upstream_parts = upstream.and_then(|upstream| {
        remotes
            .iter()
            .filter(|remote| {
                upstream.len() > remote.name.len() + 1
                    && upstream.starts_with(remote.name.as_str())
                    && upstream.as_bytes()[remote.name.len()] == b'/'
            })
            .max_by_key(|remote| remote.name.len())
            .map(|remote| (remote.name.as_str(), &upstream[remote.name.len() + 1..]))
    });

    let (base_remote, base_ref) = by_name("upstream")
        .or_else(|| upstream_parts.and_then(|(remote, _)| by_name(remote)))
        .or_else(|| by_name("origin"))
        .or_else(|| github.first())?;

    let head = current_branch
        .filter(|branch| !branch.is_empty() && *branch != "HEAD")
        .map(|branch| {
            match upstream_parts.and_then(|(remote, remote_branch)| {
                by_name(remote).map(|(_, reference)| (reference.owner.clone(), remote_branch))
            }) {
                Some((owner, remote_branch)) => (owner, remote_branch.to_string()),
                None => {
                    let owner = by_name("origin")
                        .map(|(_, reference)| reference.owner.clone())
                        .unwrap_or_else(|| base_ref.owner.clone());
                    (owner, branch.to_string())
                }
            }
        });

    Some((
        GitHubRemote {
            name: base_remote.name.clone(),
            repository: base_ref.clone(),
        },
        head,
    ))
}

/// "Retrying at 14:05" para el fin de la espera (segundos desde la época).
fn retry_message(reset_at: u64) -> String {
    use chrono::TimeZone as _;
    match chrono::Local.timestamp_opt(reset_at as i64, 0).single() {
        Some(time) => format!(
            "GitHub rate limit reached. Retrying at {}.",
            time.format("%H:%M")
        ),
        None => "GitHub rate limit reached. Retrying later.".to_string(),
    }
}

fn now_seconds() -> u64 {
    (now_epoch_ms().max(0) / 1000) as u64
}

/// Un dato que no se pudo leer se deja sin saber; el cupo agotado corta.
fn keep<T>(result: Result<T, GitHubError>) -> Result<Option<T>, GitHubError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code == GitHubErrorCode::RateLimited => Err(error),
        Err(_) => Ok(None),
    }
}

/// Los checks y la revisión de un PR. Un fallo que no sea de cupo deja
/// ese dato sin saber; el de cupo corta.
fn pull_details(
    client: &GitHubClient,
    token: &str,
    repository: &GitHubRepositoryRef,
    pull: &GitHubPullRequest,
    cache: &ResponseCache,
) -> Result<(Option<CheckSummary>, Option<ReviewSummary>), GitHubError> {
    let checks = keep(client.commit_checks(token, repository, &pull.head_sha, cache))?;
    let review = keep(client.pull_request_reviews(
        token,
        repository,
        pull.number,
        pull.requested_reviewers,
        cache,
    ))?;
    Ok((checks, review))
}

fn fetch_overview(
    client: &GitHubClient,
    token: &str,
    remote: &GitHubRemote,
    head: Option<&(String, String)>,
    local_branch: Option<&str>,
    cache: &ResponseCache,
) -> Result<Overview, GitHubError> {
    let repository = &remote.repository;
    let pulls = client.list_open_pull_requests(token, repository, cache)?;
    // `pr-<n>` es la rama en la que "Checkout PR branch" trae un PR de un fork.
    let fork_checkout = local_branch
        .and_then(|branch| branch.strip_prefix("pr-"))
        .and_then(|number| number.parse::<u64>().ok())
        .and_then(|number| {
            pulls
                .iter()
                .find(|pull| pull.number == number && !pull.same_repository)
        });
    let current = match head {
        _ if fork_checkout.is_some() => fork_checkout.cloned(),
        Some((owner, branch)) => {
            // El listado ya lo trae casi siempre; si no (más de 100 PRs
            // abiertos), se pregunta por la rama.
            let listed = pulls.iter().find(|pull| {
                pull.head_ref == *branch
                    && pull
                        .head_owner
                        .as_deref()
                        .is_some_and(|head_owner| head_owner.eq_ignore_ascii_case(owner))
            });
            match listed {
                Some(pull) => Some(pull.clone()),
                // Por si la API no filtra (un `head` que no reconoce).
                None => client
                    .find_pull_request_for_branch(token, repository, owner, branch, cache)?
                    .filter(|pull| pull.head_ref == *branch),
            }
        }
        None => None,
    };
    let current = match current {
        Some(pull) => {
            let (checks, review) = pull_details(client, token, repository, &pull, cache)?;
            Some(PullRequestItem {
                pull,
                checks,
                review,
            })
        }
        None => None,
    };
    Ok(Overview { pulls, current })
}

/// Checks y revisiones de varios PRs, unos cuantos a la vez.
fn fetch_details(
    client: &GitHubClient,
    token: &str,
    repository: &GitHubRepositoryRef,
    pulls: &[GitHubPullRequest],
    cache: &ResponseCache,
) -> (Details, Option<GitHubError>) {
    if pulls.is_empty() {
        return (Vec::new(), None);
    }
    let workers = DETAIL_WORKERS.min(pulls.len());
    let results: Vec<(Details, Option<GitHubError>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                scope.spawn(move || {
                    let mut details = Vec::new();
                    for pull in pulls.iter().skip(worker).step_by(workers) {
                        match pull_details(client, token, repository, pull, cache) {
                            Ok((checks, review)) => {
                                details.push((pull.number, pull.head_sha.clone(), checks, review))
                            }
                            Err(error) => return (details, Some(error)),
                        }
                    }
                    (details, None)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_default())
            .collect()
    });

    let mut details = Vec::new();
    let mut error = None;
    for (batch, failure) in results {
        details.extend(batch);
        if error.is_none() {
            error = failure;
        }
    }
    (details, error)
}

/// La lista nueva, conservando checks y revisión de los PRs cuyo commit no
/// cambió hasta que lleguen los nuevos (la fila no parpadea vacía).
fn merge_pulls(
    previous: &[PullRequestItem],
    pulls: Vec<GitHubPullRequest>,
) -> Vec<PullRequestItem> {
    pulls
        .into_iter()
        .map(|pull| {
            let known = previous.iter().find(|item| {
                item.pull.number == pull.number && item.pull.head_sha == pull.head_sha
            });
            PullRequestItem {
                checks: known.and_then(|item| item.checks),
                review: known.and_then(|item| item.review),
                pull,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// AppModel
// ---------------------------------------------------------------------------

impl AppModel {
    /// Lo que se sabe de los PRs de un repositorio abierto.
    pub fn pull_requests(&self, repo_id: &str) -> Option<&RepoPullRequests> {
        self.github.pull_requests.repos.get(repo_id)
    }

    /// Lo que se sabe de los PRs del repositorio activo.
    pub fn active_pull_requests(&self) -> Option<&RepoPullRequests> {
        self.pull_requests(&self.active_repository()?.id)
    }

    /// El PR de la rama actual del repositorio activo, si se conoce.
    pub fn active_branch_pull_request(&self) -> Option<&PullRequestItem> {
        let repository = self.active_repository()?;
        let branch = repository.current_branch.as_deref()?;
        let state = self.pull_requests(&repository.id)?;
        if state.availability != PullRequestsAvailability::Available {
            return None;
        }
        state.current_for(branch)
    }

    /// El vigilante de PRs está en marcha.
    pub fn is_watching_pull_requests(&self) -> bool {
        self.github.pull_requests.watch.is_some()
    }

    /// Refrescos de PRs empezados (tests).
    #[doc(hidden)]
    pub fn pull_request_refresh_count(&self) -> usize {
        self.github.pull_requests.refreshes
    }

    /// Pone en marcha el vigilante de PRs (ver el módulo). Lo llaman las
    /// vistas que los pintan; llamarlo otra vez no hace nada. Recargar la
    /// aplicación lo detiene: las vistas lo vuelven a pedir.
    pub fn watch_pull_requests(&mut self, cx: &mut Context<Self>) {
        if self.github.pull_requests.watch.is_some() {
            return;
        }
        let this = cx.entity();
        let observer = cx.observe(&this, |this, _, cx| this.pull_requests_model_changed(cx));
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_TICK).await;
                if this
                    .update(cx, |this, cx| this.pull_requests_tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.github.pull_requests.watch = Some(PullRequestWatch {
            _observer: observer,
            _ticker: ticker,
        });
        self.pull_requests_model_changed(cx);
    }

    /// Cuenta con la que se pregunta: (clave de la caché, token).
    fn pull_request_auth(&self) -> Option<(String, String)> {
        let account = self.active_github_account()?;
        let context = self.get_git_auth_context()?;
        Some((account.id.clone(), context.token))
    }

    /// La cuenta con la que se preguntaría, sin copiar su token.
    fn pull_request_account_key(&self) -> Option<String> {
        self.active_github_account()
            .filter(|account| !account.token.is_empty() && !account.requires_reconnect)
            .map(|account| account.id.clone())
    }

    fn pull_requests_watch_key(&self) -> WatchKey {
        let repository = self.active_repository();
        WatchKey {
            repo_id: repository.map(|repository| repository.id.clone()),
            branch: repository.and_then(|repository| repository.current_branch.clone()),
            account: self.pull_request_account_key(),
            remote_busy: repository.is_some_and(|repository| {
                REMOTE_OPERATIONS.iter().any(|operation| {
                    self.is_single_flight_running(&Self::get_repo_operation_key(
                        &repository.id,
                        operation,
                    ))
                })
            }),
            open_repositories: self.open_repositories().len(),
        }
    }

    fn pull_requests_model_changed(&mut self, cx: &mut Context<Self>) {
        let key = self.pull_requests_watch_key();
        let state = &mut self.github.pull_requests;
        if state.last_key.as_ref() == Some(&key) {
            return;
        }
        let previous = state.last_key.replace(key.clone());

        if previous
            .as_ref()
            .is_some_and(|previous| previous.open_repositories != key.open_repositories)
        {
            let open: HashSet<String> = self
                .open_repositories()
                .iter()
                .map(|repository| repository.id.clone())
                .collect();
            self.github
                .pull_requests
                .repos
                .retain(|repo_id, _| open.contains(repo_id));
        }

        let Some(repo_id) = key.repo_id.clone() else {
            return;
        };
        let Some(previous) = previous else {
            self.ensure_pull_requests(&repo_id, cx);
            return;
        };

        if previous.account != key.account {
            // Otra cuenta ve otras cosas: se empieza de cero.
            let state = &mut self.github.pull_requests;
            state.repos.clear();
            if let Some(account) = &previous.account {
                state.caches.remove(account);
            }
            cx.notify();
            self.refresh_pull_requests(&repo_id, true, cx).detach();
        } else if previous.repo_id != key.repo_id {
            self.ensure_pull_requests(&repo_id, cx);
        } else if previous.branch != key.branch || (previous.remote_busy && !key.remote_busy) {
            self.refresh_pull_requests(&repo_id, true, cx).detach();
        }
    }

    /// Refresca si nunca se pidió o si lo que hay es viejo.
    fn ensure_pull_requests(&mut self, repo_id: &str, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let stale = self
            .github
            .pull_requests
            .repos
            .get(repo_id)
            .and_then(|state| state.fetched_at)
            .is_none_or(|at| now.saturating_duration_since(at) >= PULL_REQUESTS_REFRESH_INTERVAL);
        if stale {
            self.refresh_pull_requests(repo_id, false, cx).detach();
        }
    }

    fn pull_requests_tick(&mut self, cx: &mut Context<Self>) {
        if !self.window_focused() {
            return;
        }
        let Some(repo_id) = self
            .active_repository()
            .map(|repository| repository.id.clone())
        else {
            return;
        };
        let rate_limit_over = self
            .github
            .pull_requests
            .repos
            .get(&repo_id)
            .and_then(|state| state.rate_limited_until)
            .is_some_and(|until| now_seconds() >= until);
        if rate_limit_over {
            self.refresh_pull_requests(&repo_id, true, cx).detach();
        } else {
            self.ensure_pull_requests(&repo_id, cx);
        }
    }

    /// Cambia el estado de un repositorio y avisa solo si cambió lo que se
    /// pinta.
    fn update_pull_requests(
        &mut self,
        repo_id: &str,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut RepoPullRequests),
    ) {
        // El repositorio se cerró mientras se preguntaba.
        if self.repositories.repository(repo_id).is_none() {
            return;
        }
        let state = &mut self.github.pull_requests;
        let entry = state.repos.entry(repo_id.to_string()).or_default();
        let before = entry.clone();
        update(entry);
        if !entry.same_view(&before) {
            // Única en toda la app: tras olvidar un estado (otra cuenta) el
            // nuevo no repite la generación del viejo.
            state.generations += 1;
            entry.generation = state.generations;
            cx.notify();
        }
    }

    /// Pide a GitHub los PRs del repositorio. Sin `force` no se repite si el
    /// último es muy reciente. Con uno en vuelo se encadena otro al acabar.
    /// Nunca avisa con toasts: el fallo queda en el estado.
    pub fn refresh_pull_requests(
        &mut self,
        repo_id: &str,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(repository) = self.repositories.repository(repo_id) else {
            return Task::ready(());
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();
        let current_branch = repository
            .current_branch
            .clone()
            .filter(|branch| !branch.is_empty() && branch != "HEAD");
        let upstream = current_branch.as_ref().and_then(|current| {
            repository
                .branches
                .iter()
                .find(|branch| &branch.name == current)
                .and_then(|branch| branch.upstream.clone())
        });
        let auth = self.pull_request_auth();
        let now = cx.background_executor().now();

        let state = &mut self.github.pull_requests;
        if state.in_flight.contains(&repo_id) {
            state.pending.insert(repo_id);
            return Task::ready(());
        }
        let known = state.repos.get(&repo_id);
        if !force
            && known
                .and_then(|known| known.fetched_at)
                .is_some_and(|at| now.saturating_duration_since(at) < MIN_REFRESH_GAP)
        {
            return Task::ready(());
        }
        if known
            .and_then(|known| known.rate_limited_until)
            .is_some_and(|until| until > now_seconds())
        {
            return Task::ready(());
        }
        state.in_flight.insert(repo_id.clone());
        state.refreshes += 1;
        let cache = auth.as_ref().map(|(account, _)| {
            state
                .caches
                .entry(account.clone())
                .or_insert_with(|| Arc::new(ResponseCache::new()))
                .clone()
        });
        let backend = self.backend.clone();

        flow(cx, async move |this, cx| {
            let remotes = cx
                .background_executor()
                .spawn(async move { backend.git().get_remotes(&repo_path) })
                .await;
            let target = remotes.ok().and_then(|remotes| {
                pick_github_remote(&remotes, current_branch.as_deref(), upstream.as_deref())
            });
            let client = this.read_with(cx, |this, _| this.backend.github().clone());

            let outcome = match (target, auth, cache) {
                (None, _, _) => {
                    this.update(cx, |this, cx| {
                        this.update_pull_requests(&repo_id, cx, |state| {
                            *state = RepoPullRequests {
                                availability: PullRequestsAvailability::NotGitHub,
                                fetched_at: Some(now),
                                generation: state.generation,
                                ..Default::default()
                            };
                        });
                    });
                    None
                }
                (Some((remote, _)), None, _) | (Some((remote, _)), _, None) => {
                    this.update(cx, |this, cx| {
                        this.update_pull_requests(&repo_id, cx, |state| {
                            *state = RepoPullRequests {
                                availability: PullRequestsAvailability::SignedOut,
                                remote: Some(remote),
                                fetched_at: Some(now),
                                generation: state.generation,
                                ..Default::default()
                            };
                        });
                    });
                    None
                }
                (Some((remote, head)), Some((_, token)), Some(cache)) => {
                    Some((remote, head, token, cache))
                }
            };

            if let Some((remote, head, token, cache)) = outcome {
                this.update(cx, |this, cx| {
                    this.update_pull_requests(&repo_id, cx, |state| {
                        let was_available = state.availability
                            == PullRequestsAvailability::Available
                            && state.remote.as_ref() == Some(&remote);
                        if !was_available {
                            state.pulls = Arc::from([]);
                            state.current = None;
                            state.error = None;
                        }
                        state.availability = PullRequestsAvailability::Available;
                        state.remote = Some(remote.clone());
                        state.loading = !was_available;
                    });
                });

                let overview = {
                    let client = client.clone();
                    let token = token.clone();
                    let remote = remote.clone();
                    let cache = cache.clone();
                    let head = head.clone();
                    let local_branch = current_branch.clone();
                    cx.background_executor()
                        .spawn(async move {
                            fetch_overview(
                                &client,
                                &token,
                                &remote,
                                head.as_ref(),
                                local_branch.as_deref(),
                                &cache,
                            )
                        })
                        .await
                };
                let branch_name = current_branch.clone();
                let detailed = this.update(cx, |this, cx| {
                    let finished_at = cx.background_executor().now();
                    let mut detailed = None;
                    this.update_pull_requests(&repo_id, cx, |state| {
                        state.loading = false;
                        state.fetched_at = Some(finished_at);
                        match overview {
                            Ok(overview) => {
                                let mut pulls = merge_pulls(&state.pulls, overview.pulls);
                                if let Some(current) = &overview.current
                                    && let Some(item) = pulls
                                        .iter_mut()
                                        .find(|item| item.pull.number == current.pull.number)
                                {
                                    *item = current.clone();
                                }
                                let wanted: Vec<GitHubPullRequest> = pulls
                                    .iter()
                                    .take(MAX_DETAILED_PULL_REQUESTS)
                                    .filter(|item| {
                                        overview.current.as_ref().is_none_or(|current| {
                                            current.pull.number != item.pull.number
                                        })
                                    })
                                    .map(|item| item.pull.clone())
                                    .collect();
                                state.pulls = pulls.into();
                                state.current = overview.current;
                                state.current_branch = branch_name;
                                state.error = None;
                                state.rate_limited_until = None;
                                detailed = Some(wanted);
                            }
                            Err(error) => apply_error(state, &error, &cache),
                        }
                    });
                    detailed
                });

                if let Some(wanted) = detailed.filter(|wanted| !wanted.is_empty()) {
                    let repository = remote.repository.clone();
                    let (details, error) = {
                        let cache = cache.clone();
                        cx.background_executor()
                            .spawn(async move {
                                fetch_details(&client, &token, &repository, &wanted, &cache)
                            })
                            .await
                    };
                    this.update(cx, |this, cx| {
                        this.update_pull_requests(&repo_id, cx, |state| {
                            let mut pulls = state.pulls.to_vec();
                            for (number, sha, checks, review) in details {
                                if let Some(item) = pulls.iter_mut().find(|item| {
                                    item.pull.number == number && item.pull.head_sha == sha
                                }) {
                                    item.checks = checks;
                                    item.review = review;
                                }
                            }
                            state.pulls = pulls.into();
                            if let Some(error) = error {
                                apply_error(state, &error, &cache);
                            }
                        });
                    });
                }
            }

            let again = this.update(cx, |this, _| {
                let state = &mut this.github.pull_requests;
                state.in_flight.remove(&repo_id);
                state.pending.remove(&repo_id)
            });
            if again {
                this.update(cx, |this, cx| {
                    this.refresh_pull_requests(&repo_id, true, cx)
                })
                .await;
            }
        })
    }

    /// Trae la rama del PR `number` (como `fetch`, con su aviso y su entrada
    /// en el log), relee ramas y remotos y dice a qué hacer checkout.
    /// `None` si el PR no se conoce o el fetch falló (ya avisado).
    pub fn fetch_pull_request(
        &mut self,
        repo_id: &str,
        number: u64,
        cx: &mut Context<Self>,
    ) -> Task<Option<PullRequestCheckout>> {
        let Some(repository) = self.repositories.repository(repo_id) else {
            return Task::ready(None);
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();
        let Some(state) = self.pull_requests(&repo_id) else {
            return Task::ready(None);
        };
        let (Some(remote), Some(item)) = (state.remote.clone(), state.find(number).cloned()) else {
            return Task::ready(None);
        };
        let pull = item.pull;
        let (head, checkout) = if pull.same_repository && !pull.head_ref.is_empty() {
            (
                PullRequestHead::SameRepository {
                    branch: pull.head_ref.clone(),
                },
                PullRequestCheckout::RemoteBranch {
                    remote: remote.name.clone(),
                    branch: pull.head_ref.clone(),
                },
            )
        } else {
            (
                PullRequestHead::Fork { number },
                PullRequestCheckout::LocalBranch(pull_request_branch_name(number)),
            )
        };
        let options = FetchPullRequestOptions {
            remote: remote.name.clone(),
            head,
            auth_token: self.git_auth_token(),
        };

        flow(cx, async move |this, cx| {
            let result: GitMutation = this
                .update(cx, |this, cx| {
                    this.fetch_pull_request_head(&repo_path, number, options, cx)
                })
                .await;
            if !result.succeeded_or_noop() {
                return None;
            }
            this.update(cx, |this, cx| {
                this.apply_repository_operation_result(repo_id.as_str(), &result, cx);
                this.sync_repository_read_model(
                    repo_id.as_str(),
                    SyncRequest::new()
                        .branches(true)
                        .remotes(true)
                        .operation("fetch")
                        .result(result),
                    cx,
                )
            })
            .await;
            Some(checkout)
        })
    }
}

/// Un fallo de refresco: se guarda para mostrarlo, sin toasts.
fn apply_error(state: &mut RepoPullRequests, error: &GitHubError, cache: &ResponseCache) {
    log::debug!(
        "pull requests: {} ({}, {})",
        error.message,
        error.code,
        error.status
    );
    match error.code {
        GitHubErrorCode::RateLimited => {
            let reset_at = cache
                .rate_limit()
                .map(|limit| limit.reset_at)
                .filter(|reset_at| *reset_at > 0)
                .unwrap_or_else(|| now_seconds() + 60);
            state.rate_limited_until = Some(reset_at);
            state.error = Some(retry_message(reset_at));
        }
        GitHubErrorCode::AuthRequired => state.error = Some(AUTH_ERROR.to_string()),
        _ if matches!(error.status, 403 | 404) => {
            state.availability = PullRequestsAvailability::NoAccess;
            state.pulls = Arc::from([]);
            state.current = None;
            state.error = None;
        }
        _ => state.error = Some(NETWORK_ERROR.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_git_query::RemoteRefs;

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_string(),
            refs: RemoteRefs {
                fetch: url.to_string(),
                push: url.to_string(),
            },
        }
    }

    /// Remoto, `owner/repo` y de dónde sale la rama actual.
    type Picked = (String, String, Option<(String, String)>);

    fn pick(remotes: &[Remote], branch: Option<&str>, upstream: Option<&str>) -> Option<Picked> {
        pick_github_remote(remotes, branch, upstream).map(|(remote, head)| {
            (
                remote.name,
                format!("{}/{}", remote.repository.owner, remote.repository.repo),
                head,
            )
        })
    }

    fn head(owner: &str, branch: &str) -> Option<(String, String)> {
        Some((owner.to_string(), branch.to_string()))
    }

    #[test]
    fn asks_origin_for_a_plain_clone() {
        let remotes = [remote("origin", "git@github.com:acme/web.git")];
        assert_eq!(
            pick(&remotes, Some("feat/x"), Some("origin/feat/x")),
            Some(("origin".into(), "acme/web".into(), head("acme", "feat/x")))
        );
        // Sin upstream se busca con el nombre local en origin.
        assert_eq!(
            pick(&remotes, Some("wip"), None),
            Some(("origin".into(), "acme/web".into(), head("acme", "wip")))
        );
        // HEAD desanclado: no hay rama que buscar.
        assert_eq!(
            pick(&remotes, Some("HEAD"), None),
            Some(("origin".into(), "acme/web".into(), None))
        );
    }

    #[test]
    fn a_fork_asks_upstream_for_its_own_branches() {
        let remotes = [
            remote("origin", "https://github.com/ana/web.git"),
            remote("upstream", "https://github.com/acme/web.git"),
        ];
        assert_eq!(
            pick(&remotes, Some("fix"), Some("origin/fix-remote")),
            Some((
                "upstream".into(),
                "acme/web".into(),
                head("ana", "fix-remote")
            ))
        );
    }

    #[test]
    fn ignores_remotes_that_are_not_on_github() {
        let remotes = [
            remote("origin", "https://gitlab.com/acme/web.git"),
            remote("gh", "git@github.com:acme/web.git"),
        ];
        assert_eq!(
            pick(&remotes, Some("main"), Some("origin/main")),
            Some(("gh".into(), "acme/web".into(), head("acme", "main")))
        );

        let enterprise = [remote("origin", "https://github.acme.corp/acme/web.git")];
        assert_eq!(pick(&enterprise, Some("main"), None), None);
        assert_eq!(pick(&[], Some("main"), None), None);
    }

    #[test]
    fn remote_names_with_slashes_split_the_upstream_right() {
        let remotes = [
            remote("team/gh", "git@github.com:team/web.git"),
            remote("team", "git@github.com:other/web.git"),
        ];
        assert_eq!(
            pick(&remotes, Some("x"), Some("team/gh/feat")),
            Some(("team/gh".into(), "team/web".into(), head("team", "feat")))
        );
    }

    fn pull(number: u64, sha: &str) -> GitHubPullRequest {
        GitHubPullRequest {
            number,
            title: format!("PR {number}"),
            html_url: String::new(),
            draft: false,
            author: None,
            author_avatar_url: None,
            head_ref: "x".into(),
            head_sha: sha.into(),
            head_repository: None,
            head_owner: None,
            base_ref: "main".into(),
            same_repository: true,
            updated_at: None,
            requested_reviewers: 0,
        }
    }

    #[test]
    fn keeps_known_details_while_the_commit_does_not_change() {
        let checks = CheckSummary {
            total: 1,
            passed: 1,
            failed: 0,
            pending: 0,
        };
        let previous = [
            PullRequestItem {
                pull: pull(1, "a"),
                checks: Some(checks),
                review: None,
            },
            PullRequestItem {
                pull: pull(2, "b"),
                checks: Some(checks),
                review: None,
            },
        ];
        let merged = merge_pulls(&previous, vec![pull(2, "b2"), pull(1, "a"), pull(3, "c")]);
        assert_eq!(merged[0].checks, None);
        assert_eq!(merged[1].checks, Some(checks));
        assert_eq!(merged[2].checks, None);
    }

    #[test]
    fn rate_limit_errors_wait_for_the_reset_and_say_when() {
        let cache = ResponseCache::new();
        let mut state = RepoPullRequests::default();
        apply_error(
            &mut state,
            &GitHubError::new("limit", GitHubErrorCode::RateLimited, 403),
            &cache,
        );
        let until = state.rate_limited_until.unwrap();
        assert!(until >= now_seconds());
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .starts_with("GitHub rate limit reached.")
        );

        let mut state = RepoPullRequests {
            availability: PullRequestsAvailability::Available,
            ..Default::default()
        };
        apply_error(
            &mut state,
            &GitHubError::new("Not Found", GitHubErrorCode::GithubError, 404),
            &cache,
        );
        assert_eq!(state.availability, PullRequestsAvailability::NoAccess);
        assert!(!state.availability.is_visible());

        let mut state = RepoPullRequests::default();
        apply_error(
            &mut state,
            &GitHubError::new("down", GitHubErrorCode::GithubUnavailable, 503),
            &cache,
        );
        assert_eq!(state.error.as_deref(), Some(NETWORK_ERROR));
    }
}
