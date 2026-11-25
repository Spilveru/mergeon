//! Canales `github:*` de `electron/ipc/github-handlers.js`: envuelven las
//! funciones del cliente y anteponen al mensaje de error el prefijo del canal,
//! conservando `code` y `status`.

use crate::client::GitHubClient;
use crate::error::GitHubResult;
use crate::types::{
    AccountResult, AuthContext, AuthStartResult, GitHubRepository, PollLoginResult,
    RepositorySearchResult,
};

/// `perPage` por defecto del canal `github:getUserRepositories`.
pub const DEFAULT_LIST_PER_PAGE: u32 = 100;
/// `perPage` por defecto del canal `github:searchRepositories`.
pub const DEFAULT_SEARCH_PER_PAGE: u32 = 30;

impl GitHubClient {
    fn load_repositories_for_context(
        &self,
        auth_context: &AuthContext,
        page: u32,
        per_page: u32,
        query: Option<&str>,
    ) -> GitHubResult<RepositorySearchResult> {
        let result = self.get_repositories(
            &auth_context.token,
            auth_context.installation_id,
            page,
            per_page,
            query,
        )?;

        Ok(RepositorySearchResult {
            total_count: result.total_count,
            repositories: result.repositories,
        })
    }

    /// Canal `github:auth:start`.
    pub fn auth_start(&self) -> GitHubResult<AuthStartResult> {
        self.start_login()
            .map(|session| AuthStartResult {
                session,
                client_id: self.client_id().to_string(),
            })
            .map_err(|error| error.with_prefix("Failed to start GitHub login"))
    }

    /// Canal `github:auth:poll`.
    pub fn auth_poll(&self, device_code: &str) -> GitHubResult<PollLoginResult> {
        self.poll_login(device_code)
            .map_err(|error| error.with_prefix("Failed to poll GitHub login"))
    }

    /// Canal `github:auth:refresh`.
    pub fn auth_refresh(&self, refresh_token: &str) -> GitHubResult<AccountResult> {
        self.refresh_session(refresh_token)
            .map_err(|error| error.with_prefix("Failed to refresh session"))
    }

    /// Canal `github:auth:token`.
    pub fn auth_token(&self, personal_access_token: &str) -> GitHubResult<AccountResult> {
        self.authenticate_with_token(personal_access_token)
            .map_err(|error| error.with_prefix("Failed to authenticate with token"))
    }

    /// Canal `github:getUserRepositories`: una página de repositorios de la
    /// instalación del contexto (o por afiliación si no tiene instalación).
    pub fn get_user_repositories(
        &self,
        auth_context: &AuthContext,
        page: u32,
        per_page: u32,
    ) -> GitHubResult<Vec<GitHubRepository>> {
        self.load_repositories_for_context(auth_context, page, per_page, None)
            .map(|result| result.repositories)
            .map_err(|error| error.with_prefix("Failed to fetch repositories"))
    }

    /// Canal `github:searchRepositories`.
    pub fn search_repositories(
        &self,
        auth_context: &AuthContext,
        query: &str,
        page: u32,
        per_page: u32,
    ) -> GitHubResult<RepositorySearchResult> {
        self.load_repositories_for_context(auth_context, page, per_page, Some(query))
            .map_err(|error| error.with_prefix("Search failed"))
    }
}
