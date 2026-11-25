//! Qué partes del working tree no hace falta vigilar porque git las ignora.
//!
//! En linux cada directorio vigilado gasta un watch de inotify, y `target/`,
//! `.venv/` o `build/` pueden tener más carpetas que el resto del repositorio
//! junto: sin podarlos se agotaban los watches del usuario. Aquí se leen los
//! `.gitignore` del repositorio (el de cada directorio, la primera vez que
//! hace falta), `.git/info/exclude` y el `core.excludesFile` global, con las
//! mismas precedencias que git: manda el `.gitignore` más profundo, luego
//! `info/exclude` y por último el global.
//!
//! Un directorio ignorado puede tener archivos versionados (añadidos con
//! `git add -f`): esos sí cambian el estado, así que un directorio con algo
//! versionado dentro no se poda nunca. Lo mismo vale para el respaldo de
//! carpetas de build habituales ([`FALLBACK_PRUNED_DIRECTORIES`]), que se
//! podan aunque no estén en `.gitignore` salvo que tengan algo versionado.
//!
//! `.git` no pasa por aquí: lo decide [`super::classify`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder, gitconfig_excludes_path};

use super::fingerprint::resolve_git_dir;

/// Carpetas de dependencias y de build que no se vigilan aunque el
/// repositorio no las ignore, si no tienen nada versionado dentro.
pub(crate) const FALLBACK_PRUNED_DIRECTORIES: [&str; 4] =
    ["node_modules", "out", "dist", "release"];

/// Las reglas de ignorado de un repositorio, para el recorrido y los eventos
/// del watcher. Las rutas que recibe son relativas a la raíz del repositorio.
pub(crate) struct IgnoreRules {
    root: PathBuf,
    /// `.gitignore` de cada directorio ya consultado (`None`: no tiene).
    per_directory: HashMap<PathBuf, Option<Gitignore>>,
    /// `.git/info/exclude` sobre el `core.excludesFile` global.
    excludes: Gitignore,
    /// Archivos versionados que alguna regla ignora (o que están dentro de
    /// una carpeta del respaldo), y todos sus directorios.
    tracked: HashSet<PathBuf>,
}

impl IgnoreRules {
    /// Lee las reglas de `root`. Si git no responde (no es un repositorio, o
    /// no está instalado) se sigue sin saber qué hay versionado: se poda solo
    /// lo ignorado y el respaldo, como antes.
    pub(crate) fn load(root: &Path) -> Self {
        let mut rules = Self::without_tracked(root);
        rules.tracked = tracked_but_ignored(root);
        rules
    }

    fn without_tracked(root: &Path) -> Self {
        let mut excludes = GitignoreBuilder::new(root);
        if let Some(global) = gitconfig_excludes_path() {
            excludes.add(global);
        }
        // `info/exclude` va después: dentro de un mismo matcher manda lo último.
        excludes.add(resolve_git_dir(root).join("info").join("exclude"));
        Self {
            root: root.to_path_buf(),
            per_directory: HashMap::new(),
            excludes: excludes.build().unwrap_or_else(|_| Gitignore::empty()),
            tracked: HashSet::new(),
        }
    }

    /// Vuelve a leer el `.gitignore` de `directory` (cambió en disco).
    pub(crate) fn reload_gitignore(&mut self, directory: &Path) {
        self.per_directory.remove(directory);
    }

    fn gitignore(&mut self, directory: &Path) -> Option<&Gitignore> {
        let root = &self.root;
        self.per_directory
            .entry(directory.to_path_buf())
            .or_insert_with(|| {
                let path = root.join(directory).join(".gitignore");
                if !path.is_file() {
                    return None;
                }
                let (gitignore, _partial_error) = Gitignore::new(path);
                (!gitignore.is_empty()).then_some(gitignore)
            })
            .as_ref()
    }

    /// Si las reglas ignoran `relative` (solo la ruta, sin mirar sus padres).
    fn is_ignored(&mut self, relative: &Path, is_dir: bool) -> bool {
        // Del directorio más profundo a la raíz: el primero que opina manda.
        for directory in relative.ancestors().skip(1) {
            let Some(gitignore) = self.gitignore(directory) else {
                continue;
            };
            let inside = relative.strip_prefix(directory).unwrap_or(relative);
            match gitignore.matched(inside, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        self.excludes.matched(relative, is_dir).is_ignore()
    }

    fn is_tracked(&self, relative: &Path) -> bool {
        self.tracked.contains(relative)
    }

    /// Si el directorio `relative` no hace falta vigilarlo (ni nada debajo):
    /// lo ignora el repositorio o es del respaldo, y no tiene nada versionado.
    pub(crate) fn should_prune_directory(&mut self, relative: &Path) -> bool {
        if relative.as_os_str().is_empty() || is_git_metadata(relative) || self.is_tracked(relative)
        {
            return false;
        }
        is_fallback(relative) || self.is_ignored(relative, true)
    }

    /// Si lo que no esté versionado dentro del directorio `relative` no
    /// cambia el estado: el directorio está ignorado (aunque tenga algo
    /// versionado), o es del respaldo sin nada versionado.
    fn hides_untracked_contents(&mut self, relative: &Path) -> bool {
        (is_fallback(relative) && !self.is_tracked(relative)) || self.is_ignored(relative, true)
    }

    /// Si un evento en `relative` no puede cambiar el estado del repositorio:
    /// no es nada versionado y está ignorado o dentro de algo ignorado.
    /// `is_dir` es `None` si ya no existe (se borró o se movió).
    pub(crate) fn should_skip_event(&mut self, relative: &Path, is_dir: Option<bool>) -> bool {
        if relative.as_os_str().is_empty() || is_git_metadata(relative) || self.is_tracked(relative)
        {
            return false;
        }
        let parents: Vec<PathBuf> = relative
            .ancestors()
            .skip(1)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .collect();
        if parents
            .iter()
            .any(|parent| self.hides_untracked_contents(parent))
        {
            return true;
        }
        match is_dir {
            Some(true) => self.hides_untracked_contents(relative),
            Some(false) => self.is_ignored(relative, false),
            // Ya no existe: vale lo que diría de él como archivo o como carpeta.
            None => self.is_ignored(relative, false) || self.hides_untracked_contents(relative),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_tracked(root: &Path, tracked: &[&str]) -> Self {
        let mut rules = Self::without_tracked(root);
        rules.tracked = with_parents(tracked.iter().map(PathBuf::from));
        rules
    }
}

fn is_fallback(relative: &Path) -> bool {
    relative
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| FALLBACK_PRUNED_DIRECTORIES.contains(&name))
}

/// `.git` y lo de dentro (también el de un submódulo): no es cosa de estas reglas.
fn is_git_metadata(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| component.as_os_str() == ".git")
}

/// Archivos versionados que las reglas de git (más el respaldo) ignoran, con
/// todos sus directorios. Una sola llamada a git, que suele devolver poco.
fn tracked_but_ignored(root: &Path) -> HashSet<PathBuf> {
    let mut args = vec![
        "ls-files".to_string(),
        "-z".to_string(),
        "--cached".to_string(),
        "--ignored".to_string(),
        "--exclude-standard".to_string(),
    ];
    args.extend(
        FALLBACK_PRUNED_DIRECTORIES
            .iter()
            .map(|directory| format!("--exclude={directory}/")),
    );
    match mergeon_gitcmd::Git::new(root).raw_bytes(&args) {
        Ok(output) => with_parents(
            output
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(String::from_utf8_lossy(path).as_ref())),
        ),
        Err(error) => {
            log::debug!(
                "status watcher: could not list ignored tracked files in {}: {error}",
                root.display()
            );
            HashSet::new()
        }
    }
}

fn with_parents(paths: impl IntoIterator<Item = PathBuf>) -> HashSet<PathBuf> {
    let mut all = HashSet::new();
    for path in paths {
        for ancestor in path.ancestors() {
            if ancestor.as_os_str().is_empty() || !all.insert(ancestor.to_path_buf()) {
                break;
            }
        }
    }
    all
}
