//! Los archivos de cada repositorio para "Go to file…" de la paleta:
//! `git ls-files -z` (y el commit de HEAD, con el que se abre la pestaña),
//! leídos en segundo plano y guardados por repositorio.
//!
//! Una mutación del repositorio (commit, checkout, cambios en disco…) solo
//! marca su lista como vieja; se vuelve a leer la próxima vez que se abre
//! la paleta, que mientras tanto usa la vieja.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{AppContext as _, Context, Task};
use mergeon_gitcmd::Git;

/// Más archivos que esto no se listan (un monorepo enorme).
pub const MAX_FILES: usize = 200_000;
/// Tope de la salida de `ls-files` (rutas de unos 250 bytes de media).
const MAX_OUTPUT: usize = 64 * 1024 * 1024;
/// Repositorios cuya lista se guarda.
const MAX_REPOSITORIES: usize = 16;

/// Los archivos de un repositorio.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileList {
    /// Rutas relativas, con `/`, en el orden de git.
    pub files: Arc<Vec<String>>,
    /// Commit completo de HEAD; `None` en un repositorio sin commits.
    pub head: Option<String>,
    /// Había más de [`MAX_FILES`].
    pub truncated: bool,
}

/// Lee los archivos de `repo_path` (bloquea: solo en segundo plano).
pub fn list_files(repo_path: &str) -> Result<FileList, String> {
    let git = Git::new(repo_path);
    let output = git
        .exec_capped(["ls-files", "-z"], MAX_OUTPUT)
        .map_err(|error| error.to_string())?;
    if !output.exceeded && !output.output.success() {
        return Err(output.output.stderr_text());
    }
    let mut truncated = output.exceeded;
    let mut files: Vec<String> = output
        .output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    // Con la salida cortada, la última ruta puede estar a medias.
    if output.exceeded {
        files.pop();
    }
    if files.len() > MAX_FILES {
        files.truncate(MAX_FILES);
        truncated = true;
    }
    let head = git
        .revparse(["--verify", "--quiet", "HEAD"])
        .ok()
        .filter(|hash| !hash.is_empty());
    Ok(FileList {
        files: Arc::new(files),
        head,
        truncated,
    })
}

#[derive(Default)]
struct Entry {
    list: Option<FileList>,
    stale: bool,
    generation: u64,
    loading: Option<Task<()>>,
    /// Orden de uso, para soltar la lista más vieja.
    used: u64,
}

/// La caché. Avisa (`cx.notify()`) cuando llega una lista.
#[derive(Default)]
pub struct FileIndex {
    entries: HashMap<String, Entry>,
    clock: u64,
}

impl FileIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// La lista guardada (aunque sea vieja).
    pub fn get(&self, repo_path: &str) -> Option<&FileList> {
        self.entries.get(repo_path)?.list.as_ref()
    }

    pub fn is_loading(&self, repo_path: &str) -> bool {
        self.entries
            .get(repo_path)
            .is_some_and(|entry| entry.loading.is_some())
    }

    /// La lista de un repositorio ya no vale (cambió algo en él).
    pub fn invalidate(&mut self, repo_path: &str) {
        if let Some(entry) = self.entries.get_mut(repo_path) {
            entry.stale = true;
            entry.generation = entry.generation.wrapping_add(1);
        }
    }

    fn apply_loaded(&mut self, path: &str, generation: u64, result: Result<FileList, String>) {
        let Some(entry) = self.entries.get_mut(path) else {
            return;
        };
        entry.loading = None;
        if entry.generation != generation {
            return;
        }
        match result {
            Ok(list) => {
                entry.list = Some(list);
                entry.stale = false;
            }
            Err(error) => log::warn!("git ls-files en {path}: {error}"),
        }
    }

    /// Lee la lista si no la hay o es vieja (y no se está leyendo ya).
    pub fn ensure(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        self.clock += 1;
        let clock = self.clock;
        let entry = self.entries.entry(repo_path.to_string()).or_default();
        entry.used = clock;
        if entry.loading.is_some() || (entry.list.is_some() && !entry.stale) {
            return;
        }
        let generation = entry.generation;
        let path = repo_path.to_string();
        let reading = cx.background_spawn({
            let path = path.clone();
            async move { list_files(&path) }
        });
        entry.loading = Some(cx.spawn(async move |this, cx| {
            let result = reading.await;
            this.update(cx, |this, cx| {
                this.apply_loaded(&path, generation, result);
                cx.notify();
            })
            .ok();
        }));
        self.evict();
    }

    fn evict(&mut self) {
        while self.entries.len() > MAX_REPOSITORIES {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.loading.is_none())
                .min_by_key(|(_, entry)| entry.used)
                .map(|(path, _)| path.clone());
            match oldest {
                Some(path) => {
                    self.entries.remove(&path);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn lists_tracked_files_with_spaces_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir_all(root.join("src/sub dir")).unwrap();
        std::fs::write(root.join("README.md"), "hola\n").unwrap();
        std::fs::write(root.join("src/sub dir/año.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "x\n").unwrap();

        // Sin commits: los archivos del índice, sin HEAD.
        git(root, &["add", "README.md", "src"]);
        let list = list_files(&root.to_string_lossy()).unwrap();
        assert_eq!(*list.files, ["README.md", "src/sub dir/año.rs"]);
        assert_eq!(list.head, None);

        git(root, &["commit", "-q", "-m", "first"]);
        let list = list_files(&root.to_string_lossy()).unwrap();
        assert_eq!(list.files.len(), 2);
        assert_eq!(list.head.as_deref().map(str::len), Some(40));
        assert!(!list.truncated);
    }

    #[test]
    fn a_path_that_is_not_a_repository_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_files(&dir.path().to_string_lossy()).is_err());
    }

    #[test]
    fn invalidate_during_in_flight_load_discards_stale_result_and_keeps_entry_stale() {
        let mut index = FileIndex::new();
        let path = "/repo";
        let initial = FileList {
            files: Arc::new(vec!["a.rs".to_string()]),
            head: Some("1".repeat(40)),
            truncated: false,
        };
        index.entries.insert(
            path.to_string(),
            Entry {
                list: Some(initial.clone()),
                stale: false,
                generation: 0,
                loading: None,
                used: 1,
            },
        );

        // Start a refresh at generation 0, then invalidate before it finishes.
        let started_gen = index.entries[path].generation;
        index.invalidate(path);
        assert!(index.entries[path].stale);
        assert_eq!(index.entries[path].generation, 1);

        let stale_result = FileList {
            files: Arc::new(vec!["old.rs".to_string()]),
            head: Some("2".repeat(40)),
            truncated: false,
        };
        index.apply_loaded(path, started_gen, Ok(stale_result));
        // Stale completion must NOT overwrite cached list or clear `stale`.
        assert_eq!(index.get(path), Some(&initial));
        assert!(index.entries[path].stale);

        // Completion matching current generation updates list and clears `stale`.
        let fresh_result = FileList {
            files: Arc::new(vec!["a.rs".to_string(), "b.rs".to_string()]),
            head: Some("3".repeat(40)),
            truncated: false,
        };
        let current_gen = index.entries[path].generation;
        index.apply_loaded(path, current_gen, Ok(fresh_result.clone()));
        assert_eq!(index.get(path), Some(&fresh_result));
        assert!(!index.entries[path].stale);
    }
}
