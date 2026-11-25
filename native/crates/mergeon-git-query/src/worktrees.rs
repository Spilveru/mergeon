//! Worktrees del repositorio (`git worktree list --porcelain -z`) y si cada
//! uno tiene cambios sin commitear.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Inner, QueryResult, read};
use crate::exec::{self, Capped};

/// Un worktree enlazado al repositorio (o el principal).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// Ruta absoluta, tal como la da git (con `/` también en Windows).
    pub path: String,
    /// Commit en el que está (`None` en un repositorio bare o sin commits).
    pub head: Option<String>,
    /// Rama desprotegida, sin `refs/heads/` (`None` si está desanclado).
    pub branch: Option<String>,
    /// El worktree principal (el primero de la lista).
    pub is_main: bool,
    /// Es el worktree del repositorio que se consultó.
    pub is_current: bool,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    /// Motivo del bloqueo, si se dio uno.
    pub lock_reason: Option<String>,
    /// Su carpeta ya no existe: `git worktree prune` lo quitaría.
    pub prunable: bool,
    pub prunable_reason: Option<String>,
}

impl Worktree {
    /// Nombre corto para la interfaz: la última carpeta de la ruta.
    pub fn name(&self) -> &str {
        self.path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.path)
    }
}

/// Parsea `git worktree list --porcelain`, con `-z` (atributos terminados en
/// NUL y registros separados por un NUL vacío) o sin él (líneas y registros
/// separados por una línea vacía).
pub fn parse_worktree_list(output: &str, nul_separated: bool) -> Vec<Worktree> {
    let separator = if nul_separated { '\0' } else { '\n' };
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in output.split(separator) {
        let line = if nul_separated {
            line
        } else {
            line.strip_suffix('\r').unwrap_or(line)
        };
        if line.is_empty() {
            worktrees.extend(current.take());
            continue;
        }
        let (key, value) = match line.split_once(' ') {
            Some((key, value)) => (key, Some(value)),
            None => (line, None),
        };
        if key == "worktree" {
            worktrees.extend(current.take());
            current = Some(Worktree {
                path: value.unwrap_or_default().to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(worktree) = current.as_mut() else {
            continue;
        };
        match key {
            "HEAD" => worktree.head = value.map(str::to_string),
            "branch" => {
                worktree.branch = value.map(|branch| {
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string()
                });
            }
            "bare" => worktree.bare = true,
            "detached" => worktree.detached = true,
            "locked" => {
                worktree.locked = true;
                worktree.lock_reason = value.map(str::to_string).filter(|text| !text.is_empty());
            }
            "prunable" => {
                worktree.prunable = true;
                worktree.prunable_reason =
                    value.map(str::to_string).filter(|text| !text.is_empty());
            }
            _ => {}
        }
    }
    worktrees.extend(current);
    if let Some(main) = worktrees.first_mut() {
        main.is_main = true;
    }
    // Un HEAD de ceros es un repositorio sin commits.
    for worktree in &mut worktrees {
        if worktree
            .head
            .as_deref()
            .is_some_and(|head| head.bytes().all(|byte| byte == b'0'))
        {
            worktree.head = None;
        }
    }
    worktrees
}

/// Forma comparable de una ruta: canónica si existe, con `/` y, en Windows,
/// sin distinguir mayúsculas.
pub fn comparable_path(path: &Path) -> String {
    let resolved = mergeon_gitcmd::paths::canonicalize(path)
        .unwrap_or_else(|_| mergeon_gitcmd::paths::strip_verbatim_prefix(path.to_path_buf()));
    let text = resolved.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/');
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// Los worktrees del repositorio, el principal primero. Marca cuál es el de
/// `repo_path`.
pub fn get_worktrees(repo_path: &Path) -> QueryResult<Vec<Worktree>> {
    read("Failed to list worktrees", || worktrees(repo_path))
}

fn worktrees(repo_path: &Path) -> Inner<Vec<Worktree>> {
    let git = exec::open(repo_path)?;
    // `-z` llegó con git 2.36; antes, el formato por líneas (rutas con saltos
    // de línea aparte, es el mismo).
    let output = exec::exec(&git, &["worktree", "list", "--porcelain", "-z"])?;
    let mut list = if output.success() {
        parse_worktree_list(&output.stdout_text(), true)
    } else {
        parse_worktree_list(
            &exec::raw(&git, &["worktree", "list", "--porcelain"])?,
            false,
        )
    };
    let toplevel = exec::raw(&git, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|top| top.trim().to_string())
        .filter(|top| !top.is_empty())
        .unwrap_or_else(|| repo_path.to_string_lossy().into_owned());
    let current = comparable_path(Path::new(&toplevel));
    for worktree in &mut list {
        worktree.is_current = comparable_path(Path::new(&worktree.path)) == current;
    }
    Ok(list)
}

/// Si el worktree de `path` tiene algo sin commitear (preparado, modificado o
/// sin seguir; los submódulos no cuentan). Barato: git se corta en cuanto
/// escribe el primer byte de `status`.
pub fn worktree_has_changes(path: &Path) -> QueryResult<bool> {
    read("Failed to read the worktree status", || {
        let git = exec::open(path)?;
        let status = exec::raw_capped(
            &git,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=all",
            ],
            0,
        )?;
        Ok(match status {
            Capped::TooLarge(_) => true,
            Capped::Complete(output) => !output.is_empty(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn parses_the_porcelain_list_with_and_without_nul() {
        let text = "worktree /repo\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\nworktree /repo-feature x\nHEAD 2222222222222222222222222222222222222222\ndetached\nlocked on a usb drive\n\nworktree /gone\nHEAD 3333333333333333333333333333333333333333\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n\n";
        let nul = text.replace('\n', "\0");
        for (output, nul_separated) in [(text.to_string(), false), (nul, true)] {
            let list = parse_worktree_list(&output, nul_separated);
            assert_eq!(list.len(), 3);
            assert!(list[0].is_main && !list[1].is_main);
            assert_eq!(list[0].branch.as_deref(), Some("main"));
            assert_eq!(list[1].path, "/repo-feature x");
            assert_eq!(list[1].name(), "repo-feature x");
            assert!(list[1].detached && list[1].branch.is_none());
            assert!(list[1].locked);
            assert_eq!(list[1].lock_reason.as_deref(), Some("on a usb drive"));
            assert!(list[2].prunable);
            assert_eq!(
                list[2].prunable_reason.as_deref(),
                Some("gitdir file points to non-existent location")
            );
        }
    }

    #[test]
    fn a_bare_repository_has_no_head() {
        let list = parse_worktree_list("worktree /srv/repo.git\0bare\0\0", true);
        assert_eq!(list.len(), 1);
        assert!(list[0].bare && list[0].head.is_none());
    }

    /// Principal, uno con rama, uno desanclado y bloqueado, y uno cuya
    /// carpeta se borró (prunable). Rutas con espacios y no ASCII.
    #[test]
    fn lists_real_worktrees() {
        let repo = TestRepo::new();
        let head = repo.commit_file("a.txt", "a\n", "a");
        let feature = repo.root().join("wt feature ñ");
        let detached = repo.root().join("wt-detached");
        let gone = repo.root().join("wt-gone");
        repo.git(&[
            "worktree",
            "add",
            "-b",
            "feature",
            &feature.to_string_lossy(),
        ]);
        repo.git(&[
            "worktree",
            "add",
            "--detach",
            &detached.to_string_lossy(),
            "HEAD",
        ]);
        repo.git(&[
            "worktree",
            "lock",
            "--reason",
            "external disk",
            &detached.to_string_lossy(),
        ]);
        repo.git(&["worktree", "add", "-b", "old", &gone.to_string_lossy()]);
        std::fs::remove_dir_all(&gone).unwrap();

        let list = get_worktrees(&repo.path).unwrap();

        assert_eq!(list.len(), 4);
        assert!(list[0].is_main && list[0].is_current);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        let by_name = |name: &str| {
            list.iter()
                .find(|worktree| worktree.name() == name)
                .unwrap()
        };
        let feature = by_name("wt feature ñ");
        assert_eq!(feature.branch.as_deref(), Some("feature"));
        assert_eq!(feature.head.as_deref(), Some(head.as_str()));
        assert!(!feature.is_current);
        let detached = by_name("wt-detached");
        assert!(detached.detached && detached.locked);
        assert_eq!(detached.lock_reason.as_deref(), Some("external disk"));
        assert!(by_name("wt-gone").prunable);

        // Desde el worktree enlazado, el actual es él.
        let from_feature = get_worktrees(&repo.root().join("wt feature ñ")).unwrap();
        assert!(
            from_feature
                .iter()
                .any(|worktree| worktree.is_current && worktree.name() == "wt feature ñ")
        );
    }

    #[test]
    fn tells_whether_a_worktree_has_changes() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "a\n", "a");
        assert!(!worktree_has_changes(&repo.path).unwrap());
        repo.write("new.txt", "untracked\n");
        assert!(worktree_has_changes(&repo.path).unwrap());
        std::fs::remove_file(repo.path.join("new.txt")).unwrap();
        repo.write("a.txt", "changed\n");
        assert!(worktree_has_changes(&repo.path).unwrap());
        assert!(worktree_has_changes(&repo.root().join("missing")).is_err());
    }
}
