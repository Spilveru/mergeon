//! Cliente de autenticación y API de GitHub.
//!
//! Además del port, lee pull requests, revisiones y checks de un repositorio
//! (`pulls.rs`) con peticiones condicionales (`ETag`) y control del cupo.
//!
//! Porta `electron/services/github-auth-client.js` (device flow, refresh de
//! sesión, token personal y listado de repositorios),
//! `electron/services/github-repo-url.js` y los canales `github:*` de
//! `electron/ipc/github-handlers.js`.
//!
//! Todas las funciones son síncronas y bloqueantes; quien las llama desde la
//! interfaz debe hacerlo en un hilo de fondo. El HTTP va detrás de
//! [`HttpTransport`] para que los tests no toquen la red.
//!
//! Los tipos que llevan tokens ([`GitHubAccount`], [`AuthContext`],
//! [`HttpRequest`], [`HttpResponse`], [`DeviceLoginSession`]) los ocultan en
//! su `Debug`, y el crate nunca los escribe en el log.

mod client;
mod config;
mod error;
mod handlers;
mod http;
mod pulls;
mod repo_url;
mod types;

pub use client::GitHubClient;
pub use config::{
    AppMode, DEFAULT_APP_SLUG, DEFAULT_CLIENT_ID, ENV_APP_MODE, ENV_APP_SLUG, ENV_CLIENT_ID,
    GITHUB_API_URL, GITHUB_BASE_URL, GitHubConfig, parse_dotenv,
};
pub use error::{GitHubError, GitHubErrorCode, GitHubResult};
pub use handlers::{DEFAULT_LIST_PER_PAGE, DEFAULT_SEARCH_PER_PAGE};
pub use http::{
    DEFAULT_TIMEOUT, HttpMethod, HttpRequest, HttpResponse, HttpTransport, TransportError,
    UreqTransport,
};
pub use pulls::{
    CheckState, CheckSummary, DEFAULT_CACHE_ENTRIES, GitHubPullRequest, MAX_OPEN_PULLS,
    PULLS_PER_PAGE, RateLimit, ResponseCache, ReviewDecision, ReviewSummary, summarize_checks,
    summarize_reviews,
};
pub use repo_url::{GitHubRepositoryRef, parse_github_repository_url};
pub use types::{
    AccountResult, AuthContext, AuthStartResult, DeviceLoginSession, GitHubAccount,
    GitHubInstallation, GitHubRepository, LoginStatus, PollLoginResult, RepositoryOwner,
    RepositoryPage, RepositorySearchResult,
};
