//! Remotos: listado (`git.getRemotes(true)`) y prueba de conexión.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

use mergeon_gitcmd::{
    ControlCode, Git, GitResult, NETWORK_TIMEOUT, OperationContext, current_operation_context,
};

use crate::error::{QueryResult, read};
use crate::exec;
use crate::text::{is_js_whitespace, js_trim, sort_like_object_keys};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRefs {
    pub fetch: String,
    pub push: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub name: String,
    pub refs: RemoteRefs,
}

/// `parseGetRemotesVerbose` de simple-git sobre la salida de `git remote -v`.
pub(crate) fn parse_remotes_verbose(text: &str) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();

    for line in text.split('\n').map(js_trim) {
        if line.is_empty() {
            continue;
        }

        let mut fields = line
            .split(is_js_whitespace)
            .filter(|field| !field.is_empty());
        let name = fields.next().unwrap_or_default();
        let url = fields.next().unwrap_or_default();
        let purpose = fields.next().unwrap_or_default();

        let at = match remotes.iter().position(|remote| remote.name == name) {
            Some(at) => at,
            None => {
                remotes.push(Remote {
                    name: name.to_string(),
                    refs: RemoteRefs::default(),
                });
                remotes.len() - 1
            }
        };

        // `(fetch)` / `(push)`: simple-git se queda con las letras.
        let purpose: String = purpose.chars().filter(char::is_ascii_lowercase).collect();
        match purpose.as_str() {
            "fetch" => remotes[at].refs.fetch = url.to_string(),
            "push" => remotes[at].refs.push = url.to_string(),
            _ => {}
        }
    }

    sort_like_object_keys(&mut remotes, |remote| &remote.name);
    remotes
}

pub(crate) fn list_remotes(git: &Git) -> GitResult<Vec<Remote>> {
    Ok(parse_remotes_verbose(&exec::raw(git, &["remote", "-v"])?))
}

/// Canal `git:getRemotes`.
pub fn get_remotes(repo_path: &Path) -> QueryResult<Vec<Remote>> {
    read("Failed to get remotes", || {
        Ok(list_remotes(&exec::open(repo_path)?)?)
    })
}

/// Resultado de `git:testRemoteConnection`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConnectionTest {
    pub success: bool,
    pub message: String,
}

fn test_connection(repo_path: &Path, url: &str, idle_timeout: Duration) -> RemoteConnectionTest {
    // Dentro de una operación manda su contexto; fuera, el timeout de red: si
    // el remoto no contesta, git se queda esperando indefinidamente.
    let own_timeout = current_operation_context().is_none();
    let listed = exec::open(repo_path).and_then(|git| {
        let git = if own_timeout {
            git.with_context(Some(OperationContext::new(None, idle_timeout)))
        } else {
            git
        };
        exec::raw(&git, &["ls-remote", "--heads", "--tags", url])
    });

    match listed {
        Ok(_) => RemoteConnectionTest {
            success: true,
            message: "Connection successful.".to_string(),
        },
        Err(error) => {
            let message = if error.control == Some(ControlCode::Timeout) {
                // Mensaje del plugin de timeout de simple-git.
                "block timeout reached".to_string()
            } else if error.message.is_empty() {
                "Unable to reach remote repository.".to_string()
            } else {
                error.message
            };
            RemoteConnectionTest {
                success: false,
                message,
            }
        }
    }
}

/// Canal `git:testRemoteConnection`. Nunca falla: el motivo va en `message`.
pub fn test_remote_connection(repo_path: &Path, url: &str) -> RemoteConnectionTest {
    test_connection(repo_path, url, NETWORK_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn parses_fetch_and_push_urls_per_remote() {
        let text = [
            "origin\thttps://example.com/a.git (fetch)",
            "origin\tssh://example.com/a.git (push)",
            "upstream\thttps://example.com/b.git (fetch)",
            "upstream\thttps://example.com/b.git (push)",
            "",
        ]
        .join("\n");

        let remotes = parse_remotes_verbose(&text);

        assert_eq!(
            remotes,
            [
                Remote {
                    name: "origin".into(),
                    refs: RemoteRefs {
                        fetch: "https://example.com/a.git".into(),
                        push: "ssh://example.com/a.git".into()
                    }
                },
                Remote {
                    name: "upstream".into(),
                    refs: RemoteRefs {
                        fetch: "https://example.com/b.git".into(),
                        push: "https://example.com/b.git".into()
                    }
                },
            ]
        );
    }

    #[test]
    fn lists_the_remotes_of_a_repository() {
        let repo = TestRepo::new();
        assert!(get_remotes(&repo.path).unwrap().is_empty());

        let remote = repo.add_bare_origin();
        let remotes = get_remotes(&repo.path).unwrap();

        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].refs.fetch, remote.to_string_lossy());
        assert_eq!(remotes[0].refs.push, remote.to_string_lossy());

        let json = serde_json::to_value(&remotes).unwrap();
        assert_eq!(json[0]["refs"]["fetch"], *remote.to_string_lossy());
    }

    #[test]
    fn get_remotes_fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let error = get_remotes(dir.path()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to get remotes: fatal: not a git repository"),
            "{error}"
        );
    }

    #[test]
    fn a_reachable_remote_passes_the_connection_test() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        let remote = repo.add_bare_origin();
        repo.git(&["push", "origin", "main"]);

        let result = test_remote_connection(&repo.path, &remote.to_string_lossy());

        assert_eq!(
            result,
            RemoteConnectionTest {
                success: true,
                message: "Connection successful.".into()
            }
        );
    }

    #[test]
    fn an_unreachable_remote_reports_the_git_error() {
        let repo = TestRepo::new();
        let missing = repo.root().join("missing.git");

        let result = test_remote_connection(&repo.path, &missing.to_string_lossy());

        assert!(!result.success);
        assert!(result.message.contains("fatal:"), "{}", result.message);
    }

    #[test]
    fn a_remote_that_never_answers_times_out() {
        let repo = TestRepo::new();
        // Acepta la conexión (cola del kernel) y no contesta nunca.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "git://127.0.0.1:{}/repo.git",
            listener.local_addr().unwrap().port()
        );

        let started = std::time::Instant::now();
        let result = test_connection(&repo.path, &url, Duration::from_millis(300));

        assert_eq!(
            result,
            RemoteConnectionTest {
                success: false,
                message: "block timeout reached".into()
            }
        );
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn a_missing_directory_is_reported_in_the_message() {
        let dir = tempfile::tempdir().unwrap();

        let result = test_remote_connection(&dir.path().join("gone"), "https://example.com/x.git");

        assert_eq!(
            result,
            RemoteConnectionTest {
                success: false,
                message: "Cannot use simple-git on a directory that does not exist".into()
            }
        );
    }

    #[test]
    fn a_url_that_is_really_an_option_never_reaches_git() {
        let repo = TestRepo::new();
        let marker = repo.root().join("pwned");
        let url = format!("--upload-pack=touch {}", marker.display());

        let result = test_remote_connection(&repo.path, &url);

        assert!(!result.success);
        assert_eq!(
            result.message,
            "Use of --upload-pack or --receive-pack is not permitted without enabling allowUnsafePack"
        );
        assert!(!marker.exists());
    }
}
