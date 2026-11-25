//! Funciones puras que los componentes Vue llevaban en su `<script setup>`:
//! nombres de carpeta, tamaños, rutas de clonado y la clasificación de los
//! fallos de GitHub que hace `Dashboard.vue`.

use std::cmp::Ordering;

use mergeon_github::GitHubError;

/// `getParentFolderName`: la carpeta que contiene al repositorio, que es el
/// grupo bajo el que aparece en el inicio.
pub fn parent_folder_name(repo_path: &str) -> String {
    let parts: Vec<&str> = repo_path
        .split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    parts
        .len()
        .checked_sub(2)
        .and_then(|index| parts.get(index))
        .map_or_else(|| "Workspace".to_string(), |name| (*name).to_string())
}

/// `formatRepositorySize`: "0 B", "512 B", "1.5 KB", "308 KB", "2.4 GB".
pub fn format_repository_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }

    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if size >= 10.0 || unit == 0 {
        // `Math.round`.
        format!("{} {}", (size + 0.5).floor(), UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// `joinLocalPath`: la carpeta de destino más el nombre del repositorio, con
/// el separador de la plataforma.
pub fn join_local_path(base_path: &str, name: &str, windows: bool) -> String {
    let separator = if windows { '\\' } else { '/' };
    let normalized = base_path.trim_end_matches(['\\', '/']);
    let normalized = if windows {
        normalized.replace('/', "\\")
    } else {
        normalized.to_string()
    };
    format!("{normalized}{separator}{name}")
}

/// `isPermanentAuthError` de `Dashboard.vue`: la sesión ya no vale y hay que
/// reconectar la cuenta.
pub fn is_permanent_auth_error(error: &GitHubError) -> bool {
    let code = error.code.as_str();
    if code == "session_expired" || code == "auth_required" {
        return true;
    }
    if error.status == 401 {
        return true;
    }

    let message = error.message.to_lowercase();
    message.contains("session not found")
        || message.contains("session expired")
        || message.contains("auth required")
}

/// `isTransientAuthError` de `Dashboard.vue`: GitHub no responde.
pub fn is_transient_auth_error(error: &GitHubError) -> bool {
    let code = error.code.as_str();
    code == "github_unavailable"
        || code == "github_timeout"
        || code == "github_api_error"
        || error.status >= 500
}

/// Aproximación de `String.prototype.localeCompare` para nombres de carpetas
/// y repositorios: espacio, puntuación, dígitos y letras, sin distinguir
/// mayúsculas salvo para desempatar (primero la minúscula).
pub fn locale_compare(left: &str, right: &str) -> Ordering {
    fn key(character: char) -> (u8, char) {
        let group = if character.is_whitespace() {
            0
        } else if character.is_ascii_digit() {
            2
        } else if character.is_alphabetic() {
            3
        } else {
            1
        };
        (group, character.to_lowercase().next().unwrap_or(character))
    }

    left.chars()
        .map(key)
        .cmp(right.chars().map(key))
        .then_with(|| {
            // Desempate terciario: la minúscula va antes que la mayúscula.
            left.chars()
                .map(char::is_uppercase)
                .cmp(right.chars().map(char::is_uppercase))
        })
}

#[cfg(test)]
mod tests {
    use mergeon_github::GitHubErrorCode;

    use super::*;

    #[test]
    fn groups_a_repository_under_its_parent_folder() {
        assert_eq!(parent_folder_name("/home/dev/projects/demo"), "projects");
        assert_eq!(parent_folder_name("C:\\work\\clients\\portal"), "clients");
        assert_eq!(parent_folder_name("/home/dev/projects/demo/"), "projects");
        // Sin carpeta contenedora cae en el grupo por defecto.
        assert_eq!(parent_folder_name("/demo"), "Workspace");
        assert_eq!(parent_folder_name(""), "Workspace");
    }

    #[test]
    fn formats_repository_sizes_like_the_dashboard() {
        assert_eq!(format_repository_size(0), "0 B");
        assert_eq!(format_repository_size(512), "512 B");
        assert_eq!(format_repository_size(1024), "1.0 KB");
        assert_eq!(format_repository_size(2048), "2.0 KB");
        assert_eq!(format_repository_size(1536), "1.5 KB");
        assert_eq!(format_repository_size(315_392), "308 KB");
        assert_eq!(format_repository_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_repository_size(64 * 1024 * 1024), "64 MB");
        // No hay unidad por encima de GB.
        assert_eq!(format_repository_size(4096 * 1024 * 1024 * 1024), "4096 GB");
    }

    #[test]
    fn joins_the_clone_destination_with_the_platform_separator() {
        assert_eq!(
            join_local_path("/tmp/workspace", "new-repo", false),
            "/tmp/workspace/new-repo"
        );
        assert_eq!(
            join_local_path("/tmp/workspace//", "new-repo", false),
            "/tmp/workspace/new-repo"
        );
        assert_eq!(
            join_local_path("C:\\Projects", "second", true),
            "C:\\Projects\\second"
        );
        assert_eq!(
            join_local_path("C:\\Projects\\", "second", true),
            "C:\\Projects\\second"
        );
        assert_eq!(
            join_local_path("C:/Projects/", "second", true),
            "C:\\Projects\\second"
        );
    }

    #[test]
    fn tells_expired_sessions_from_an_unreachable_github() {
        let expired = GitHubError::new("Session not found", GitHubErrorCode::AuthRequired, 401);
        assert!(is_permanent_auth_error(&expired));

        let by_status = GitHubError::new("Bad credentials", GitHubErrorCode::GithubError, 401);
        assert!(is_permanent_auth_error(&by_status));

        let by_message = GitHubError::new("Session expired", GitHubErrorCode::GithubError, 400);
        assert!(is_permanent_auth_error(&by_message));

        let unreachable = GitHubError::new("timeout", GitHubErrorCode::GithubTimeout, 0);
        assert!(!is_permanent_auth_error(&unreachable));
        assert!(is_transient_auth_error(&unreachable));

        let server = GitHubError::new("Bad gateway", GitHubErrorCode::GithubError, 502);
        assert!(is_transient_auth_error(&server));

        let other = GitHubError::new("Nope", GitHubErrorCode::GithubError, 422);
        assert!(!is_permanent_auth_error(&other));
        assert!(!is_transient_auth_error(&other));
    }

    #[test]
    fn orders_names_ignoring_case() {
        let mut names = vec![
            "website",
            "Demo",
            "api-server",
            "demo",
            "acme_portal",
            "2048",
        ];
        names.sort_by(|left, right| locale_compare(left, right));
        assert_eq!(
            names,
            [
                "2048",
                "acme_portal",
                "api-server",
                "demo",
                "Demo",
                "website"
            ]
        );
    }
}
