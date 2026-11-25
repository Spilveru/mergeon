//! Huella de lo que mueve HEAD, para detectar en modo poll un cambio de rama
//! o un commit hechos fuera de la app.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::util::resolve_path;

/// Directorio git de `repo_path`: `.git`, o el que indique un archivo `.git`
/// (`gitdir: ...`, como en worktrees y submódulos).
pub(crate) fn resolve_git_dir(repo_path: &Path) -> PathBuf {
    let dot_git_path = repo_path.join(".git");

    match fs::metadata(&dot_git_path) {
        Ok(metadata) if metadata.is_dir() => return dot_git_path,
        Ok(metadata) if metadata.is_file() => match fs::read(&dot_git_path) {
            Ok(raw_dot_git) => {
                let raw_dot_git = String::from_utf8_lossy(&raw_dot_git);
                let git_dir = raw_dot_git.lines().find_map(|line| {
                    let prefix = line.get(..7)?;
                    if !prefix.eq_ignore_ascii_case("gitdir:") {
                        return None;
                    }
                    Some(line[7..].trim()).filter(|target| !target.is_empty())
                });
                if let Some(git_dir) = git_dir {
                    return resolve_path(&repo_path.join(git_dir));
                }
            }
            Err(error) => warn_unresolved(repo_path, &error),
        },
        Ok(_) => {}
        Err(error) => warn_unresolved(repo_path, &error),
    }

    dot_git_path
}

fn warn_unresolved(repo_path: &Path, error: &std::io::Error) {
    if error.kind() != ErrorKind::NotFound {
        log::warn!(
            "Failed to resolve .git directory, falling back to default path {}: {error}",
            repo_path.display()
        );
    }
}

fn collect_head_metadata_entries(root_path: &Path, entries: &mut Vec<String>) {
    match fs::metadata(root_path) {
        Ok(metadata) if metadata.is_file() => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .unwrap_or_default();
            entries.push(format!(
                "{}:{}.{:09}:{}",
                root_path.display(),
                modified.as_secs(),
                modified.subsec_nanos(),
                metadata.len()
            ));
            return;
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return,
        Err(_) => {
            entries.push(format!("{}:missing", root_path.display()));
            return;
        }
    }

    let Ok(children) = fs::read_dir(root_path) else {
        return;
    };
    for child in children.flatten() {
        collect_head_metadata_entries(&child.path(), entries);
    }
}

/// Huella de `HEAD`, `packed-refs` y `refs/heads/**` (ruta, mtime y tamaño de
/// cada archivo). Solo sirve para comparar dos lecturas. `None` sin ruta.
pub fn get_head_fingerprint(repo_path: &Path) -> Option<String> {
    if repo_path.as_os_str().is_empty() {
        return None;
    }

    let git_dir = resolve_git_dir(repo_path);
    let metadata_paths = [
        git_dir.join("HEAD"),
        git_dir.join("packed-refs"),
        git_dir.join("refs").join("heads"),
    ];

    let mut entries = Vec::new();
    for metadata_path in &metadata_paths {
        collect_head_metadata_entries(metadata_path, &mut entries);
    }

    entries.sort();
    Some(entries.join("|"))
}

/// Directorio común de un worktree enlazado (`commondir`), donde viven las
/// refs compartidas; el propio `git_dir` en un repositorio normal.
fn resolve_common_dir(git_dir: &Path) -> PathBuf {
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(raw) if !raw.trim().is_empty() => resolve_path(&git_dir.join(raw.trim())),
        _ => git_dir.to_path_buf(),
    }
}

/// Huella de todo lo que puede cambiar el historial que muestra la vista de
/// commits: `HEAD`, `packed-refs` y `refs/**` entero (ramas, remotas, tags,
/// stash), por ruta, mtime y tamaño, sin lanzar git. Sirve para saber al
/// volver a un repositorio oculto (que no tiene watcher) si alguien lo cambió
/// mientras tanto. `None` sin ruta.
pub fn get_refs_fingerprint(repo_path: &Path) -> Option<String> {
    if repo_path.as_os_str().is_empty() {
        return None;
    }

    let git_dir = resolve_git_dir(repo_path);
    let common_dir = resolve_common_dir(&git_dir);
    let mut metadata_paths = vec![
        git_dir.join("HEAD"),
        common_dir.join("packed-refs"),
        common_dir.join("refs"),
    ];
    if common_dir != git_dir {
        // Las refs propias del worktree (`refs/bisect`...) y su HEAD.
        metadata_paths.push(git_dir.join("refs"));
    }

    let mut entries = Vec::new();
    for metadata_path in &metadata_paths {
        collect_head_metadata_entries(metadata_path, &mut entries);
    }

    entries.sort();
    Some(entries.join("|"))
}

/// Huella de los metadatos que no mueven HEAD pero sí cambian lo que se ve:
/// tags y ramas remotas por un lado y la configuración (remotos) por otro.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataFingerprint {
    /// `packed-refs`, `refs/tags/**` y `refs/remotes/**`.
    pub refs: String,
    /// `config`.
    pub config: String,
    /// `refs/stash` y su reflog `logs/refs/stash`.
    pub stash: String,
}

/// [`MetadataFingerprint`] del repositorio (por ruta, mtime y tamaño, sin
/// lanzar git). `None` sin ruta.
pub fn get_metadata_fingerprint(repo_path: &Path) -> Option<MetadataFingerprint> {
    if repo_path.as_os_str().is_empty() {
        return None;
    }

    let git_dir = resolve_git_dir(repo_path);
    let common_dir = resolve_common_dir(&git_dir);
    let collect = |paths: &[PathBuf]| {
        let mut entries = Vec::new();
        for path in paths {
            collect_head_metadata_entries(path, &mut entries);
        }
        entries.sort();
        entries.join("|")
    };
    Some(MetadataFingerprint {
        refs: collect(&[
            common_dir.join("packed-refs"),
            common_dir.join("refs").join("tags"),
            common_dir.join("refs").join("remotes"),
        ]),
        config: collect(&[common_dir.join("config")]),
        stash: collect(&[
            common_dir.join("refs").join("stash"),
            common_dir.join("logs").join("refs").join("stash"),
        ]),
    })
}

#[cfg(test)]
mod refs_fingerprint_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn changes_with_commits_branches_and_tags_but_not_with_file_edits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let initial = get_refs_fingerprint(repo).unwrap();

        // Editar el working tree no toca el historial.
        fs::write(repo.join("file.txt"), "hello").unwrap();
        assert_eq!(get_refs_fingerprint(repo).unwrap(), initial);

        git(repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
        let after_commit = get_refs_fingerprint(repo).unwrap();
        assert_ne!(after_commit, initial);

        git(repo, &["tag", "v1"]);
        let after_tag = get_refs_fingerprint(repo).unwrap();
        assert_ne!(after_tag, after_commit);

        assert_eq!(get_refs_fingerprint(Path::new("")), None);
    }

    #[test]
    fn the_metadata_fingerprint_follows_tags_remotes_and_config_separately() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let initial = get_metadata_fingerprint(repo).unwrap();

        // Un commit no cambia ni tags, ni remotos, ni la configuración.
        git(repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
        assert_eq!(get_metadata_fingerprint(repo).unwrap(), initial);

        git(repo, &["tag", "v1"]);
        let after_tag = get_metadata_fingerprint(repo).unwrap();
        assert_ne!(after_tag.refs, initial.refs);
        assert_eq!(after_tag.config, initial.config);

        // Esperar a que el mtime pueda cambiar en sistemas de archivos gruesos.
        std::thread::sleep(std::time::Duration::from_millis(20));
        git(
            repo,
            &["remote", "add", "upstream", "https://example.com/x.git"],
        );
        let after_remote = get_metadata_fingerprint(repo).unwrap();
        assert_ne!(after_remote.config, after_tag.config);
        assert_eq!(after_remote.refs, after_tag.refs);

        assert_eq!(get_metadata_fingerprint(Path::new("")), None);
    }
}
