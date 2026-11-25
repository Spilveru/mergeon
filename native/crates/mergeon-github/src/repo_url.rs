use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Dueño y nombre de un repositorio de GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubRepositoryRef {
    pub owner: String,
    pub repo: String,
}

static HTTPS_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^https://github\.com/([^/]+)/([^/]+?)(?:\.git)?$")
        .expect("regex constante válida")
});

static SSH_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:git@github\.com:|ssh://git@github\.com/)([^/]+)/([^/]+?)(?:\.git)?$")
        .expect("regex constante válida")
});

/// Extrae `owner` y `repo` de la URL de un remoto de GitHub (HTTPS o SSH).
/// Devuelve `None` si la URL está vacía o no es de github.com.
pub fn parse_github_repository_url(repo_url: &str) -> Option<GitHubRepositoryRef> {
    if repo_url.is_empty() {
        return None;
    }

    [&*HTTPS_URL, &*SSH_URL].into_iter().find_map(|pattern| {
        let captures = pattern.captures(repo_url)?;
        Some(GitHubRepositoryRef {
            owner: captures.get(1)?.as_str().to_string(),
            repo: captures.get(2)?.as_str().to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> Option<(String, String)> {
        parse_github_repository_url(url).map(|reference| (reference.owner, reference.repo))
    }

    fn pair(owner: &str, repo: &str) -> Option<(String, String)> {
        Some((owner.to_string(), repo.to_string()))
    }

    #[test]
    fn parses_https_urls_with_and_without_git_suffix() {
        assert_eq!(
            parsed("https://github.com/octocat/alpha.git"),
            pair("octocat", "alpha")
        );
        assert_eq!(
            parsed("https://github.com/octocat/alpha"),
            pair("octocat", "alpha")
        );
        assert_eq!(
            parsed("HTTPS://GitHub.com/Octocat/My.Repo.git"),
            pair("Octocat", "My.Repo")
        );
    }

    #[test]
    fn parses_ssh_urls_in_both_notations() {
        assert_eq!(
            parsed("git@github.com:octocat/alpha.git"),
            pair("octocat", "alpha")
        );
        assert_eq!(
            parsed("ssh://git@github.com/octocat/alpha"),
            pair("octocat", "alpha")
        );
    }

    #[test]
    fn rejects_empty_foreign_and_malformed_urls() {
        assert_eq!(parsed(""), None);
        assert_eq!(parsed("https://gitlab.com/octocat/alpha.git"), None);
        assert_eq!(parsed("https://github.com/octocat"), None);
        assert_eq!(parsed("https://github.com/octocat/alpha/tree/main"), None);
        assert_eq!(parsed("http://github.com/octocat/alpha"), None);
        assert_eq!(parsed("https://github.com/octocat/alpha/"), None);
    }
}
