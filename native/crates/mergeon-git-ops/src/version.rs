//! Versión del git instalado, para las opciones que no existen en todas.

use mergeon_gitcmd::Git;
use std::sync::OnceLock;

/// `major.minor.patch` de `git --version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

/// `git version 2.47.1`, `git version 2.47.1.windows.1`, `git version 2.39.3
/// (Apple Git-145)` → la versión. `None` si la salida no tiene esa forma.
pub fn parse_git_version(output: &str) -> Option<GitVersion> {
    let version = output.trim().strip_prefix("git version ")?;
    let version = version.split_whitespace().next()?;
    let mut numbers = version.split('.').map(|part| {
        // `2.45.0-rc1`, `2.45.GIT`: solo cuentan las cifras del principio.
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let major = numbers.next()??;
    let minor = numbers.next()??;
    let patch = numbers.next().flatten().unwrap_or(0);
    Some(GitVersion::new(major, minor, patch))
}

/// La versión del git que usa la app, leída una sola vez por proceso.
/// Bloquea: llamarla fuera del hilo de la interfaz.
pub fn installed_git_version() -> Option<GitVersion> {
    static VERSION: OnceLock<Option<GitVersion>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        Git::without_cwd()
            .raw(["--version"])
            .ok()
            .as_deref()
            .and_then(parse_git_version)
    })
}

/// Qué opciones de `git stash push` admite el git instalado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StashCapabilities {
    /// `--staged` (git 2.35).
    pub staged_only: bool,
    /// `--pathspec-from-file` en `stash push` (git 2.26).
    pub pathspec_from_file: bool,
}

impl StashCapabilities {
    /// Sin versión conocida se da por bueno `--staged` (si no existe, git lo
    /// dice) y las rutas van tras `--`, que vale en cualquier versión.
    pub fn for_version(version: Option<GitVersion>) -> Self {
        match version {
            Some(version) => Self {
                staged_only: version >= GitVersion::new(2, 35, 0),
                pathspec_from_file: version >= GitVersion::new(2, 26, 0),
            },
            None => Self {
                staged_only: true,
                pathspec_from_file: false,
            },
        }
    }
}

/// [`StashCapabilities`] del git instalado. Bloquea la primera vez (lanza
/// `git --version`): llamarla fuera del hilo de la interfaz.
pub fn stash_capabilities() -> StashCapabilities {
    StashCapabilities::for_version(installed_git_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_banners_of_every_platform() {
        assert_eq!(
            parse_git_version("git version 2.53.0\n"),
            Some(GitVersion::new(2, 53, 0))
        );
        assert_eq!(
            parse_git_version("git version 2.47.1.windows.1"),
            Some(GitVersion::new(2, 47, 1))
        );
        assert_eq!(
            parse_git_version("git version 2.39.3 (Apple Git-145)"),
            Some(GitVersion::new(2, 39, 3))
        );
        assert_eq!(
            parse_git_version("git version 2.45.0-rc1"),
            Some(GitVersion::new(2, 45, 0))
        );
        assert_eq!(
            parse_git_version("git version 3.0"),
            Some(GitVersion::new(3, 0, 0))
        );
        assert_eq!(parse_git_version("hub version 2.14.2"), None);
        assert_eq!(parse_git_version("git version x.y"), None);
    }

    #[test]
    fn stash_options_follow_the_git_version() {
        let old = StashCapabilities::for_version(Some(GitVersion::new(2, 25, 4)));
        assert!(!old.staged_only && !old.pathspec_from_file);
        let middle = StashCapabilities::for_version(Some(GitVersion::new(2, 34, 1)));
        assert!(!middle.staged_only && middle.pathspec_from_file);
        let new = StashCapabilities::for_version(Some(GitVersion::new(2, 35, 0)));
        assert!(new.staged_only && new.pathspec_from_file);
        let unknown = StashCapabilities::for_version(None);
        assert!(unknown.staged_only && !unknown.pathspec_from_file);
    }

    #[test]
    fn reads_the_installed_git() {
        let version = installed_git_version().expect("git is installed");
        assert!(version >= GitVersion::new(2, 0, 0));
    }
}
