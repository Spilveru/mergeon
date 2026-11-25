//! Credenciales HTTP tecleadas en la app para remotos que no son de GitHub.
//! La lógica (helper en línea, detección de fallos, `git credential
//! approve`) está en [`mergeon_gitcmd::credentials`]; aquí solo se expone a
//! la capa de aplicación.

use std::path::Path;

use mergeon_gitcmd::credentials::{self, HttpCredentials};

use super::GitApi;

impl GitApi {
    /// Usa `username` / `password` para `origin` (`https://host[:puerto]`)
    /// en los git de red de esta sesión, hasta [`Self::forget_http_credentials`].
    pub fn set_http_credentials(&self, origin: &str, username: &str, password: &str) {
        credentials::set_session_credentials(HttpCredentials::new(origin, username, password));
    }

    /// Olvida (y borra de memoria) las credenciales de `origin`.
    pub fn forget_http_credentials(&self, origin: &str) -> bool {
        credentials::forget_session_credentials(origin)
    }

    /// Hay credenciales de sesión para `origin`.
    pub fn has_http_credentials(&self, origin: &str) -> bool {
        credentials::has_session_credentials(origin)
    }

    /// Hay un credential helper configurado para `origin` (visto desde
    /// `repo_path`, o la configuración global sin repositorio).
    pub fn has_credential_helper(&self, repo_path: Option<&str>, origin: &str) -> bool {
        credentials::has_credential_helper(repo_path.map(Path::new), origin)
    }

    /// `git credential approve` con el helper del usuario ("Remember").
    pub fn approve_http_credentials(
        &self,
        repo_path: Option<&str>,
        origin: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        let credentials = HttpCredentials::new(origin, username, password);
        credentials::approve_http_credentials(repo_path.map(Path::new), &credentials)
            .map_err(|error| error.message)
    }
}
