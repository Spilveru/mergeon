//! Acceso a git para las operaciones: creación del ejecutor, comandos con el
//! guard de argumentos y los pocos resultados de simple-git que Mergeon usa.

use mergeon_gitcmd::{ControlCode, Git, GitError, GitResult, current_operation_context};
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

use crate::guard;
use crate::messages::js_trim;

/// Error de una operación: lo que en el original llega al `catch`, sea de git
/// o del sistema de archivos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationError {
    /// Para errores de git: stdout + stderr sin recortar.
    pub message: String,
    /// Motivo de control que traía el propio error (cancelación / timeout).
    pub control: Option<ControlCode>,
}

impl OperationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            control: None,
        }
    }

    /// `getGitOperationControlCode(error)`: primero el contexto de la operación
    /// en curso, después lo que diga el propio error.
    pub fn control_code(&self) -> Option<ControlCode> {
        current_operation_context()
            .and_then(|context| context.control_code())
            .or(self.control)
    }
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OperationError {}

impl From<GitError> for OperationError {
    fn from(error: GitError) -> Self {
        Self {
            message: error.message,
            control: error.control,
        }
    }
}

impl From<std::io::Error> for OperationError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub(crate) type OperationOutcome<T> = Result<T, OperationError>;

#[cfg(test)]
thread_local! {
    /// Entorno extra para los git que crea este crate en el hilo del test.
    static TEST_ENV: std::cell::RefCell<Vec<(String, String)>> =
        std::cell::RefCell::new(crate::tests::support::hermetic_env());
}

/// Ejecuta `task` añadiendo `variables` al entorno de los git de este hilo.
#[cfg(test)]
pub(crate) fn with_test_env<T>(variables: &[(&str, &str)], task: impl FnOnce() -> T) -> T {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_ENV.with(|env| env.borrow_mut().truncate(self.0));
        }
    }

    let previous = TEST_ENV.with(|env| {
        let mut env = env.borrow_mut();
        let previous = env.len();
        env.extend(
            variables
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        );
        previous
    });
    let _restore = Restore(previous);
    task()
}

#[cfg(test)]
fn apply_test_env(git: Git) -> Git {
    TEST_ENV.with(|env| {
        env.borrow()
            .iter()
            .fold(git, |git, (key, value)| git.with_env(key, value))
    })
}

#[cfg(not(test))]
fn apply_test_env(git: Git) -> Git {
    git
}

/// `getGit(repoPath)`: git sobre el repositorio, con el contexto de operación
/// del hilo actual.
pub(crate) fn git(repo_path: &Path) -> Git {
    apply_test_env(Git::new(repo_path))
}

/// `withAuthenticatedGit`: como [`git`], autenticando con el token de GitHub si lo hay.
pub(crate) fn authenticated_git(repo_path: &Path, auth_token: Option<&str>) -> Git {
    git(repo_path).with_github_token(auth_token)
}

/// `git.raw([...])` pasando antes por el guard de argumentos.
pub(crate) fn raw<S: AsRef<str>>(git: &Git, arguments: &[S]) -> GitResult<String> {
    guard::check_arguments(arguments)?;
    git.raw(arguments.iter().map(AsRef::as_ref))
}

/// Como [`raw`], pero falla con cualquier código de salida distinto de 0
/// aunque git no haya escrito nada en stderr. El error lleva stdout + stderr.
pub(crate) fn exec_checked<S: AsRef<str>>(git: &Git, arguments: &[S]) -> GitResult<String> {
    guard::check_arguments(arguments)?;
    let output = git.exec(arguments.iter().map(AsRef::as_ref))?;
    if output.success() {
        Ok(output.stdout_text())
    } else {
        Err(GitError::from_output(&output))
    }
}

/// [`exec_checked`] escribiendo `stdin` en la entrada de git.
pub(crate) fn exec_with_stdin_checked<S: AsRef<str>>(
    git: &Git,
    arguments: &[S],
    stdin: &[u8],
) -> GitResult<String> {
    guard::check_arguments(arguments)?;
    let output = git.exec_with_stdin(arguments.iter().map(AsRef::as_ref), stdin)?;
    if output.success() {
        Ok(output.stdout_text())
    } else {
        Err(GitError::from_output(&output))
    }
}

/// Un remoto de `git.getRemotes(true)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteInfo {
    pub name: String,
    pub fetch: String,
    pub push: String,
}

/// Parser de `git remote -v` de simple-git (`parseGetRemotesVerbose`).
pub(crate) fn parse_remotes_verbose(text: &str) -> Vec<RemoteInfo> {
    let mut remotes: Vec<RemoteInfo> = Vec::new();
    for line in text
        .split('\n')
        .map(js_trim)
        .filter(|line| !line.is_empty())
    {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let url = parts.next();
        let purpose = parts.next();

        let at = match remotes.iter().position(|remote| remote.name == name) {
            Some(at) => at,
            None => {
                remotes.push(RemoteInfo {
                    name: name.to_string(),
                    fetch: String::new(),
                    push: String::new(),
                });
                remotes.len() - 1
            }
        };

        if let (Some(url), Some(purpose)) = (url, purpose) {
            let purpose: String = purpose.chars().filter(char::is_ascii_lowercase).collect();
            match purpose.as_str() {
                "fetch" => remotes[at].fetch = url.to_string(),
                "push" => remotes[at].push = url.to_string(),
                _ => {}
            }
        }
    }
    remotes
}

/// `git.getRemotes(true)`.
pub(crate) fn get_remotes_verbose(git: &Git) -> GitResult<Vec<RemoteInfo>> {
    Ok(parse_remotes_verbose(&raw(git, &["remote", "-v"])?))
}

static COMMIT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([^\s]+)( \([^)]+\))? ([^\]]+)").expect("constant regex"));

/// Hash que simple-git extrae de la salida de `git commit` (`parseCommitResult`):
/// lo que sigue al nombre de rama en la línea `[rama hash] asunto`. Vacío si
/// git no creó ningún commit.
pub(crate) fn parse_commit_hash(stdout: &str) -> String {
    stdout
        .split('\n')
        .map(js_trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| COMMIT_LINE.captures(line))
        .filter_map(|captures| captures.get(3))
        .next_back()
        .map(|commit| commit.as_str().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_listing() {
        let remotes = parse_remotes_verbose(
            "origin\thttps://example.com/a.git (fetch)\norigin\tgit@example.com:a.git (push)\nbackup\t/tmp/backup.git (fetch)\nbackup\t/tmp/backup.git (push)\n",
        );
        assert_eq!(
            remotes,
            [
                RemoteInfo {
                    name: "origin".into(),
                    fetch: "https://example.com/a.git".into(),
                    push: "git@example.com:a.git".into(),
                },
                RemoteInfo {
                    name: "backup".into(),
                    fetch: "/tmp/backup.git".into(),
                    push: "/tmp/backup.git".into(),
                },
            ]
        );
        assert!(parse_remotes_verbose("").is_empty());
    }

    #[test]
    fn authenticated_git_keeps_the_token_out_of_argv_and_config() {
        let directory = tempfile::tempdir().unwrap();
        let authed = authenticated_git(directory.path(), Some("gho_secret_token"));

        // El token nunca aparece en Debug ni en `-c http.extraHeader`.
        let debug = format!("{authed:?}");
        assert!(!debug.contains("gho_secret_token"), "{debug}");

        let header = raw(
            &authed,
            &[
                "config",
                "--get-urlmatch",
                "http.extraHeader",
                "https://github.com/acme/web.git",
            ],
        )
        .unwrap();
        assert_eq!(header, "");
    }

    #[test]
    fn parses_the_commit_line() {
        let hash = "5b86821b1503c870b427f196e73c82526d979a93";
        assert_eq!(
            parse_commit_hash(&format!(
                "[main {hash}] message\n 1 file changed, 1 insertion(+)\n"
            )),
            hash
        );
        assert_eq!(
            parse_commit_hash(&format!("[main (root-commit) {hash}] first\n")),
            hash
        );
        assert_eq!(
            parse_commit_hash("On branch main\nnothing to commit, working tree clean\n"),
            ""
        );
    }
}
