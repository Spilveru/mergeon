//! Pull requests y checks de un repositorio (API REST v3, solo lectura).
//!
//! Cada petición va con el token de la cuenta y, si ya se hizo antes, con
//! `If-None-Match`: GitHub contesta `304 Not Modified` sin cuerpo y sin
//! gastar cupo, y la respuesta sale de la [`ResponseCache`]. La caché
//! también recuerda lo último que dijeron las cabeceras `X-RateLimit-*`.
//!
//! Lo que se pide:
//!
//! - PRs abiertos: `GET /repos/{o}/{r}/pulls?state=open`, ordenados por
//!   actualización y paginados (`Link: rel="next"`) hasta [`MAX_OPEN_PULLS`].
//! - El PR de una rama: el mismo listado con `head=owner:rama`.
//! - Revisiones: `GET /repos/{o}/{r}/pulls/{n}/reviews`, resumidas en
//!   [`ReviewSummary`].
//! - Checks del último commit: `GET /repos/{o}/{r}/commits/{sha}/check-runs`
//!   y `GET /repos/{o}/{r}/commits/{sha}/status`, combinados en
//!   [`CheckSummary`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{GitHubClient, append_query, parse_payload, text, truthy_text, uint};
use crate::error::{GitHubError, GitHubErrorCode, GitHubResult};
use crate::http::{HttpMethod, HttpRequest, HttpResponse};
use crate::repo_url::GitHubRepositoryRef;

/// PRs por página al listar (el máximo de la API).
pub const PULLS_PER_PAGE: u32 = 100;
/// Tope de PRs abiertos que se listan.
pub const MAX_OPEN_PULLS: usize = 100;
/// Respuestas que guarda una [`ResponseCache`] por defecto.
pub const DEFAULT_CACHE_ENTRIES: usize = 512;

const ACCEPT: &str = "application/vnd.github+json";
const API_VERSION: &str = "2022-11-28";

// ---------------------------------------------------------------------------
// Cupo de la API
// ---------------------------------------------------------------------------

/// Lo que dicen las cabeceras `X-RateLimit-*` de la última respuesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub limit: u64,
    pub remaining: u64,
    /// Cuándo se renueva el cupo, en segundos desde la época.
    pub reset_at: u64,
}

impl RateLimit {
    fn from_response(response: &HttpResponse) -> Option<Self> {
        let header = |name: &str| response.header(name)?.trim().parse::<u64>().ok();
        Some(Self {
            limit: header("x-ratelimit-limit").unwrap_or(0),
            remaining: header("x-ratelimit-remaining")?,
            reset_at: header("x-ratelimit-reset").unwrap_or(0),
        })
    }

    /// No queda cupo.
    pub fn is_exhausted(&self) -> bool {
        self.remaining == 0
    }
}

// ---------------------------------------------------------------------------
// Caché de respuestas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CachedResponse {
    etag: String,
    payload: Value,
    next: Option<String>,
    used_at: u64,
}

#[derive(Debug, Default)]
struct CacheInner {
    entries: HashMap<String, CachedResponse>,
    clock: u64,
    rate_limit: Option<RateLimit>,
    not_modified: u64,
}

/// Respuestas ya recibidas por URL (con su `ETag`) y el último estado del
/// cupo. Se puede compartir entre hilos. Las respuestas dependen de quién
/// pregunta: hay que usar una caché por cuenta.
///
/// No guarda tokens: solo URLs (sin credenciales) y cuerpos JSON.
#[derive(Debug)]
pub struct ResponseCache {
    inner: Mutex<CacheInner>,
    max_entries: usize,
}

impl Default for ResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponseCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CACHE_ENTRIES)
    }

    /// Caché que guarda como mucho `max_entries` respuestas: al pasarse se
    /// olvidan las usadas hace más tiempo.
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(CacheInner::default()),
            max_entries: max_entries.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// El cupo según la última respuesta (o la última espera que pidió
    /// GitHub).
    pub fn rate_limit(&self) -> Option<RateLimit> {
        self.lock().rate_limit
    }

    /// Respuestas guardadas.
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cuántas respuestas llegaron como `304 Not Modified`.
    pub fn not_modified_count(&self) -> u64 {
        self.lock().not_modified
    }

    /// Olvida las respuestas guardadas (el cupo se conserva).
    pub fn clear(&self) {
        self.lock().entries.clear();
    }

    fn etag(&self, url: &str) -> Option<String> {
        self.lock().entries.get(url).map(|entry| entry.etag.clone())
    }

    fn take_not_modified(&self, url: &str) -> Option<(Value, Option<String>)> {
        let mut inner = self.lock();
        inner.clock += 1;
        inner.not_modified += 1;
        let clock = inner.clock;
        let entry = inner.entries.get_mut(url)?;
        entry.used_at = clock;
        Some((entry.payload.clone(), entry.next.clone()))
    }

    fn store(&self, url: &str, etag: String, payload: Value, next: Option<String>) {
        let mut inner = self.lock();
        inner.clock += 1;
        let used_at = inner.clock;
        inner.entries.insert(
            url.to_string(),
            CachedResponse {
                etag,
                payload,
                next,
                used_at,
            },
        );
        if inner.entries.len() > self.max_entries {
            let excess = inner.entries.len() - self.max_entries;
            let mut oldest: Vec<(u64, String)> = inner
                .entries
                .iter()
                .map(|(url, entry)| (entry.used_at, url.clone()))
                .collect();
            oldest.sort_unstable();
            for (_, url) in oldest.into_iter().take(excess) {
                inner.entries.remove(&url);
            }
        }
    }

    fn set_rate_limit(&self, rate_limit: RateLimit) {
        self.lock().rate_limit = Some(rate_limit);
    }
}

// ---------------------------------------------------------------------------
// Tipos
// ---------------------------------------------------------------------------

/// Un pull request abierto, con lo que se pinta de él.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPullRequest {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub draft: bool,
    /// `user.login`.
    pub author: Option<String>,
    pub author_avatar_url: Option<String>,
    /// La rama del PR (`head.ref`).
    pub head_ref: String,
    /// El último commit del PR (`head.sha`).
    pub head_sha: String,
    /// `owner/repo` de la rama; `None` si el fork se borró.
    pub head_repository: Option<String>,
    /// Dueño de la rama (`head.repo.owner.login`, o `head.user.login`).
    pub head_owner: Option<String>,
    /// La rama destino (`base.ref`).
    pub base_ref: String,
    /// La rama vive en el mismo repositorio que el PR (no en un fork).
    pub same_repository: bool,
    pub updated_at: Option<String>,
    /// Personas y equipos a los que se pidió revisión y aún no revisaron.
    pub requested_reviewers: usize,
}

impl GitHubPullRequest {
    fn from_payload(payload: &Value) -> Option<Self> {
        let number = uint(payload.get("number"))?;
        let head = payload.get("head");
        let base = payload.get("base");
        let head_repo = head.and_then(|head| head.get("repo"));
        let base_repo = base.and_then(|base| base.get("repo"));
        let full_name = |repo: Option<&Value>| {
            truthy_text(repo.and_then(|repo| repo.get("full_name"))).map(|name| name.to_lowercase())
        };
        let head_repository = truthy_text(head_repo.and_then(|repo| repo.get("full_name")));
        let same_repository = match (full_name(head_repo), full_name(base_repo)) {
            (Some(head), Some(base)) => head == base,
            _ => false,
        };
        let count = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        };

        Some(Self {
            number,
            title: text(payload.get("title")).unwrap_or_default(),
            html_url: text(payload.get("html_url")).unwrap_or_default(),
            draft: payload
                .get("draft")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            author: truthy_text(payload.get("user").and_then(|user| user.get("login"))),
            author_avatar_url: truthy_text(
                payload.get("user").and_then(|user| user.get("avatar_url")),
            ),
            head_ref: text(head.and_then(|head| head.get("ref"))).unwrap_or_default(),
            head_sha: text(head.and_then(|head| head.get("sha"))).unwrap_or_default(),
            head_owner: truthy_text(
                head_repo
                    .and_then(|repo| repo.get("owner"))
                    .and_then(|owner| owner.get("login")),
            )
            .or_else(|| {
                truthy_text(
                    head.and_then(|head| head.get("user"))
                        .and_then(|user| user.get("login")),
                )
            }),
            head_repository,
            base_ref: text(base.and_then(|base| base.get("ref"))).unwrap_or_default(),
            same_repository,
            updated_at: truthy_text(payload.get("updated_at")),
            requested_reviewers: count("requested_reviewers") + count("requested_teams"),
        })
    }
}

/// Cómo va la revisión de un PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    /// Se pidió revisión y nadie la ha hecho todavía.
    Pending,
    /// Ni revisiones ni revisores pedidos.
    None,
}

/// Las revisiones de un PR, resumidas: cuenta la última de cada revisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummary {
    pub decision: ReviewDecision,
    pub approvals: usize,
    pub changes_requested: usize,
}

/// Resume las revisiones (en orden cronológico, como las da la API). Por
/// revisor cuenta la última que aprueba o pide cambios; un comentario no la
/// anula y una descartada (`DISMISSED`) sí.
pub fn summarize_reviews(reviews: &[Value], requested_reviewers: usize) -> ReviewSummary {
    let mut latest: Vec<(String, &str)> = Vec::new();
    for review in reviews {
        let Some(login) = truthy_text(review.get("user").and_then(|user| user.get("login"))) else {
            continue;
        };
        let state = review.get("state").and_then(Value::as_str).unwrap_or("");
        let state = match state {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes",
            "DISMISSED" => "dismissed",
            _ => continue,
        };
        match latest.iter_mut().find(|(known, _)| *known == login) {
            Some(entry) => entry.1 = state,
            None => latest.push((login, state)),
        }
    }

    let approvals = latest
        .iter()
        .filter(|(_, state)| *state == "approved")
        .count();
    let changes_requested = latest
        .iter()
        .filter(|(_, state)| *state == "changes")
        .count();
    let decision = if changes_requested > 0 {
        ReviewDecision::ChangesRequested
    } else if approvals > 0 {
        ReviewDecision::Approved
    } else if requested_reviewers > 0 {
        ReviewDecision::Pending
    } else {
        ReviewDecision::None
    };

    ReviewSummary {
        decision,
        approvals,
        changes_requested,
    }
}

/// Estado combinado de los checks de un commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Success,
    Failure,
    Pending,
}

/// Los check runs y los estados (API de statuses) de un commit, contados.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
}

impl CheckSummary {
    /// "5/5 checks": los que pasaron de todos.
    pub fn label(&self) -> String {
        format!(
            "{}/{} {}",
            self.passed,
            self.total,
            if self.total == 1 { "check" } else { "checks" }
        )
    }

    /// Falla si alguno falló; si no, pendiente si falta alguno; `None` sin
    /// checks.
    pub fn state(&self) -> Option<CheckState> {
        if self.total == 0 {
            None
        } else if self.failed > 0 {
            Some(CheckState::Failure)
        } else if self.pending > 0 {
            Some(CheckState::Pending)
        } else {
            Some(CheckState::Success)
        }
    }
}

/// Cuenta los `check_runs` de `/check-runs` y los `statuses` de `/status`.
/// Un run que no terminó está pendiente; terminado, pasa si acabó en
/// `success`, `neutral`, `skipped` o `stale` y falla en lo demás.
pub fn summarize_checks(check_runs: &Value, combined_status: &Value) -> CheckSummary {
    let mut summary = CheckSummary::default();
    let runs = check_runs
        .get("check_runs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for run in runs {
        summary.total += 1;
        let status = run.get("status").and_then(Value::as_str).unwrap_or("");
        if status != "completed" {
            summary.pending += 1;
            continue;
        }
        match run.get("conclusion").and_then(Value::as_str).unwrap_or("") {
            "success" | "neutral" | "skipped" | "stale" => summary.passed += 1,
            _ => summary.failed += 1,
        }
    }

    let statuses = combined_status
        .get("statuses")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for status in statuses {
        summary.total += 1;
        match status.get("state").and_then(Value::as_str).unwrap_or("") {
            "success" => summary.passed += 1,
            "pending" => summary.pending += 1,
            _ => summary.failed += 1,
        }
    }
    summary
}

// ---------------------------------------------------------------------------
// Peticiones
// ---------------------------------------------------------------------------

/// `rel="next"` de una cabecera `Link`.
pub(crate) fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let mut pieces = part.split(';');
        let target = pieces.next()?.trim();
        let is_next = pieces.any(|param| {
            let param = param.trim();
            param == "rel=\"next\"" || param == "rel=next"
        });
        if !is_next {
            return None;
        }
        target
            .strip_prefix('<')
            .and_then(|target| target.strip_suffix('>'))
            .map(str::to_string)
    })
}

/// Un segmento de ruta (`owner`, `repo`, un sha) codificado.
fn path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Una página de la API: el cuerpo y el enlace a la siguiente.
struct Page {
    payload: Value,
    next: Option<String>,
}

impl GitHubClient {
    fn repository_url(&self, repo: &GitHubRepositoryRef, rest: &str) -> String {
        format!(
            "{}/repos/{}/{}/{rest}",
            self.config.api_url,
            path_segment(&repo.owner),
            path_segment(&repo.repo)
        )
    }

    fn now_seconds(&self) -> u64 {
        (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
    }

    /// GET condicional: con la respuesta anterior en la caché se manda su
    /// `ETag` y un `304` la devuelve sin bajar nada.
    fn cached_get(&self, url: &str, token: &str, cache: &ResponseCache) -> GitHubResult<Page> {
        let mut headers = vec![
            ("Accept".to_string(), ACCEPT.to_string()),
            ("X-GitHub-Api-Version".to_string(), API_VERSION.to_string()),
        ];
        if !token.is_empty() {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        if let Some(etag) = cache.etag(url) {
            headers.push(("If-None-Match".to_string(), etag));
        }
        let request = HttpRequest {
            method: HttpMethod::Get,
            url: url.to_string(),
            headers,
            body: None,
            timeout: self.config.timeout,
        };

        let response = self
            .transport
            .send(&request)
            .map_err(|error| self.transport_failure(&request, error))?;
        log::debug!("GitHub GET {} -> {}", request.url, response.status);

        let rate_limit = RateLimit::from_response(&response);
        if let Some(rate_limit) = rate_limit {
            cache.set_rate_limit(rate_limit);
        }

        if response.status == 304 {
            return cache
                .take_not_modified(url)
                .map(|(payload, next)| Page { payload, next })
                .ok_or_else(|| {
                    GitHubError::new(
                        "Invalid response from GitHub",
                        GitHubErrorCode::InvalidResponse,
                        502,
                    )
                });
        }

        // Cupo agotado (403 o 429 con `remaining: 0`) o límite secundario
        // (`Retry-After`): no hay que volver a preguntar hasta que pase.
        if matches!(response.status, 403 | 429) {
            let retry_after = response
                .header("retry-after")
                .and_then(|value| value.trim().parse::<u64>().ok());
            let exhausted = rate_limit.is_some_and(|limit| limit.is_exhausted());
            if exhausted || retry_after.is_some() || response.status == 429 {
                if let Some(seconds) = retry_after {
                    let reset_at = self.now_seconds() + seconds;
                    cache.set_rate_limit(RateLimit {
                        limit: rate_limit.map_or(0, |limit| limit.limit),
                        remaining: 0,
                        reset_at: rate_limit.map_or(reset_at, |limit| limit.reset_at.max(reset_at)),
                    });
                } else if rate_limit.is_none() {
                    // 429 sin cabeceras: un minuto de margen.
                    cache.set_rate_limit(RateLimit {
                        limit: 0,
                        remaining: 0,
                        reset_at: self.now_seconds() + 60,
                    });
                }
                return Err(GitHubError::new(
                    "GitHub API rate limit exceeded",
                    GitHubErrorCode::RateLimited,
                    response.status,
                ));
            }
        }

        let payload = parse_payload(&response.body)?;
        if !response.is_ok() {
            return Err(GitHubError::new(
                truthy_text(payload.get("message"))
                    .unwrap_or_else(|| format!("GitHub request failed: {}", response.status)),
                if response.status == 401 {
                    GitHubErrorCode::AuthRequired
                } else {
                    GitHubErrorCode::GithubError
                },
                response.status,
            ));
        }

        let next = response.header("link").and_then(next_link);
        if let Some(etag) = response.header("etag").filter(|etag| !etag.is_empty()) {
            cache.store(url, etag.to_string(), payload.clone(), next.clone());
        }
        Ok(Page { payload, next })
    }

    /// Lista paginada de PRs a partir de `first_url`, como mucho `max`.
    fn collect_pulls(
        &self,
        first_url: String,
        token: &str,
        cache: &ResponseCache,
        max: usize,
    ) -> GitHubResult<Vec<GitHubPullRequest>> {
        let mut pulls = Vec::new();
        let mut url = Some(first_url);
        while let Some(current) = url.take() {
            let page = self.cached_get(&current, token, cache)?;
            let items = page
                .payload
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            pulls.extend(items.iter().filter_map(GitHubPullRequest::from_payload));
            if pulls.len() >= max {
                pulls.truncate(max);
                break;
            }
            url = page.next;
        }
        Ok(pulls)
    }

    /// PRs abiertos del repositorio, los actualizados más recientemente
    /// primero ([`MAX_OPEN_PULLS`] como mucho).
    pub fn list_open_pull_requests(
        &self,
        token: &str,
        repo: &GitHubRepositoryRef,
        cache: &ResponseCache,
    ) -> GitHubResult<Vec<GitHubPullRequest>> {
        self.list_open_pull_requests_paged(token, repo, cache, PULLS_PER_PAGE, MAX_OPEN_PULLS)
    }

    /// Como [`GitHubClient::list_open_pull_requests`], con el tamaño de
    /// página y el tope elegidos.
    pub fn list_open_pull_requests_paged(
        &self,
        token: &str,
        repo: &GitHubRepositoryRef,
        cache: &ResponseCache,
        per_page: u32,
        max: usize,
    ) -> GitHubResult<Vec<GitHubPullRequest>> {
        let url = append_query(
            &self.repository_url(repo, "pulls"),
            &[
                ("state", "open".to_string()),
                ("sort", "updated".to_string()),
                ("direction", "desc".to_string()),
                ("per_page", per_page.clamp(1, 100).to_string()),
            ],
        );
        self.collect_pulls(url, token, cache, max)
    }

    /// El PR abierto cuya rama es `head_owner:branch`, si lo hay.
    pub fn find_pull_request_for_branch(
        &self,
        token: &str,
        repo: &GitHubRepositoryRef,
        head_owner: &str,
        branch: &str,
        cache: &ResponseCache,
    ) -> GitHubResult<Option<GitHubPullRequest>> {
        if branch.is_empty() || head_owner.is_empty() {
            return Ok(None);
        }
        let url = append_query(
            &self.repository_url(repo, "pulls"),
            &[
                ("state", "open".to_string()),
                ("head", format!("{head_owner}:{branch}")),
                ("per_page", "1".to_string()),
            ],
        );
        Ok(self.collect_pulls(url, token, cache, 1)?.into_iter().next())
    }

    /// Las revisiones del PR `number`, resumidas.
    pub fn pull_request_reviews(
        &self,
        token: &str,
        repo: &GitHubRepositoryRef,
        number: u64,
        requested_reviewers: usize,
        cache: &ResponseCache,
    ) -> GitHubResult<ReviewSummary> {
        let url = append_query(
            &self.repository_url(repo, &format!("pulls/{number}/reviews")),
            &[("per_page", "100".to_string())],
        );
        let page = self.cached_get(&url, token, cache)?;
        let reviews = page
            .payload
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        Ok(summarize_reviews(reviews, requested_reviewers))
    }

    /// Los checks del commit `sha`: check runs más estados. Si la cuenta no
    /// puede leer una de las dos fuentes (403/404, p. ej. una GitHub App sin
    /// permiso de checks) se cuenta la otra; el cupo agotado sí es un error.
    pub fn commit_checks(
        &self,
        token: &str,
        repo: &GitHubRepositoryRef,
        sha: &str,
        cache: &ResponseCache,
    ) -> GitHubResult<CheckSummary> {
        if sha.is_empty() {
            return Ok(CheckSummary::default());
        }
        let sha = path_segment(sha);
        let tolerate = |result: GitHubResult<Page>| match result {
            Ok(page) => Ok(page.payload),
            Err(error)
                if error.code != GitHubErrorCode::RateLimited
                    && matches!(error.status, 403 | 404) =>
            {
                Ok(Value::Null)
            }
            Err(error) => Err(error),
        };
        let runs_url = append_query(
            &self.repository_url(repo, &format!("commits/{sha}/check-runs")),
            &[("per_page", "100".to_string())],
        );
        let check_runs = tolerate(self.cached_get(&runs_url, token, cache))?;
        let status_url = append_query(
            &self.repository_url(repo, &format!("commits/{sha}/status")),
            &[("per_page", "100".to_string())],
        );
        let combined_status = tolerate(self.cached_get(&status_url, token, cache))?;
        Ok(summarize_checks(&check_runs, &combined_status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_next_page_from_the_link_header() {
        let link = "<https://api.github.com/repositories/1/pulls?page=2>; rel=\"next\", \
                    <https://api.github.com/repositories/1/pulls?page=5>; rel=\"last\"";
        assert_eq!(
            next_link(link).as_deref(),
            Some("https://api.github.com/repositories/1/pulls?page=2")
        );
        assert_eq!(
            next_link("<https://x/y?page=1>; rel=\"prev\", <https://x/y?page=9>; rel=\"last\""),
            None
        );
        assert_eq!(next_link(""), None);
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(path_segment("my.repo-name_1"), "my.repo-name_1");
        assert_eq!(path_segment("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn the_latest_review_of_each_reviewer_decides() {
        let review =
            |login: &str, state: &str| json!({ "user": { "login": login }, "state": state });

        // Aprobó y luego solo comentó: sigue aprobado.
        let summary =
            summarize_reviews(&[review("ana", "APPROVED"), review("ana", "COMMENTED")], 0);
        assert_eq!(summary.decision, ReviewDecision::Approved);
        assert_eq!(summary.approvals, 1);

        // Pidió cambios y después aprobó.
        let summary = summarize_reviews(
            &[
                review("ana", "CHANGES_REQUESTED"),
                review("ana", "APPROVED"),
            ],
            0,
        );
        assert_eq!(summary.decision, ReviewDecision::Approved);

        // Uno aprueba y otro pide cambios: pesan más los cambios.
        let summary = summarize_reviews(
            &[
                review("ana", "APPROVED"),
                review("bea", "CHANGES_REQUESTED"),
            ],
            0,
        );
        assert_eq!(summary.decision, ReviewDecision::ChangesRequested);
        assert_eq!((summary.approvals, summary.changes_requested), (1, 1));

        // Una revisión descartada deja de contar.
        let summary = summarize_reviews(
            &[
                review("ana", "CHANGES_REQUESTED"),
                review("ana", "DISMISSED"),
            ],
            1,
        );
        assert_eq!(summary.decision, ReviewDecision::Pending);

        assert_eq!(summarize_reviews(&[], 0).decision, ReviewDecision::None);
        assert_eq!(summarize_reviews(&[], 2).decision, ReviewDecision::Pending);
    }

    #[test]
    fn combines_check_runs_and_statuses() {
        let runs = json!({ "total_count": 4, "check_runs": [
            { "status": "completed", "conclusion": "success" },
            { "status": "completed", "conclusion": "skipped" },
            { "status": "in_progress", "conclusion": null },
            { "status": "completed", "conclusion": "failure" },
        ]});
        let statuses = json!({ "state": "pending", "statuses": [
            { "state": "success" }, { "state": "pending" }, { "state": "error" },
        ]});
        let summary = summarize_checks(&runs, &statuses);
        assert_eq!(
            summary,
            CheckSummary {
                total: 7,
                passed: 3,
                failed: 2,
                pending: 2,
            }
        );
        assert_eq!(summary.state(), Some(CheckState::Failure));

        let only_pending = summarize_checks(
            &json!({ "check_runs": [{ "status": "queued" }, { "status": "completed", "conclusion": "success" }] }),
            &Value::Null,
        );
        assert_eq!(only_pending.state(), Some(CheckState::Pending));

        // `/status` sin estados dice "pending" pero no hay nada que esperar.
        let none = summarize_checks(
            &json!({ "check_runs": [] }),
            &json!({ "state": "pending", "statuses": [] }),
        );
        assert_eq!(none.state(), None);

        let green = summarize_checks(
            &json!({ "check_runs": [{ "status": "completed", "conclusion": "success" }] }),
            &json!({ "statuses": [{ "state": "success" }] }),
        );
        assert_eq!(green.state(), Some(CheckState::Success));
        assert_eq!((green.passed, green.total), (2, 2));
    }

    #[test]
    fn reads_a_pull_request_from_a_fork_and_from_the_same_repository() {
        let pull = |head_repo: Value| {
            json!({
                "number": 42,
                "title": "Add search",
                "html_url": "https://github.com/acme/web/pull/42",
                "draft": true,
                "user": { "login": "ana", "avatar_url": "https://avatars/ana" },
                "head": { "ref": "feat/search", "sha": "abc123", "repo": head_repo,
                          "user": { "login": "ana" } },
                "base": { "ref": "main", "repo": { "full_name": "acme/web" } },
                "updated_at": "2026-11-10T10:00:00Z",
                "requested_reviewers": [{ "login": "bea" }],
                "requested_teams": [{ "slug": "core" }]
            })
        };

        let same = GitHubPullRequest::from_payload(&pull(
            json!({ "full_name": "Acme/Web", "owner": { "login": "acme" } }),
        ))
        .unwrap();
        assert!(same.same_repository);
        assert!(same.draft);
        assert_eq!(same.head_owner.as_deref(), Some("acme"));
        assert_eq!(same.requested_reviewers, 2);
        assert_eq!(same.author.as_deref(), Some("ana"));

        let fork = GitHubPullRequest::from_payload(&pull(
            json!({ "full_name": "ana/web", "owner": { "login": "ana" } }),
        ))
        .unwrap();
        assert!(!fork.same_repository);
        assert_eq!(fork.head_repository.as_deref(), Some("ana/web"));

        // El fork se borró: `head.repo` es null.
        let gone = GitHubPullRequest::from_payload(&pull(Value::Null)).unwrap();
        assert!(!gone.same_repository);
        assert_eq!(gone.head_repository, None);
        assert_eq!(gone.head_owner.as_deref(), Some("ana"));

        assert_eq!(
            GitHubPullRequest::from_payload(&json!({ "title": "x" })),
            None
        );
    }

    #[test]
    fn the_cache_forgets_the_least_recently_used_responses() {
        let cache = ResponseCache::with_capacity(2);
        cache.store("a", "\"1\"".into(), json!([]), None);
        cache.store("b", "\"2\"".into(), json!([]), None);
        // Usar "a" la hace más reciente que "b".
        assert!(cache.take_not_modified("a").is_some());
        cache.store("c", "\"3\"".into(), json!([]), None);
        assert_eq!(cache.len(), 2);
        assert!(cache.etag("a").is_some());
        assert!(cache.etag("b").is_none());
        assert!(cache.etag("c").is_some());
    }
}
