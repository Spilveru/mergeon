//! Clasificación de las rutas que llegan del watcher y motivos de aviso.

use serde::{Deserialize, Serialize};

use super::ignore_rules::FALLBACK_PRUNED_DIRECTORIES;

/// Qué significa un cambio en una ruta para el estado del repositorio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WatchPathKind {
    /// Mueve HEAD: `.git/HEAD`, `.git/packed-refs`, `.git/refs/heads/**`.
    #[serde(rename = "head")]
    Head,
    #[serde(rename = "working-tree")]
    WorkingTree,
    /// Tags y ramas remotas: `.git/refs/tags/**`, `.git/refs/remotes/**`
    /// (`git tag`, `git fetch` o `git remote remove` hechos fuera de la app).
    #[serde(rename = "refs")]
    Refs,
    /// `.git/config`: remotos añadidos, quitados o con otra URL.
    #[serde(rename = "config")]
    Config,
    /// Stashes: `.git/refs/stash` y `.git/logs/refs/stash` (el reflog, que es
    /// lo que cambia con cada `git stash push` / `drop`).
    #[serde(rename = "stash")]
    Stash,
    #[serde(rename = "ignore")]
    Ignore,
}

impl WatchPathKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WatchPathKind::Head => "head",
            WatchPathKind::WorkingTree => "working-tree",
            WatchPathKind::Refs => "refs",
            WatchPathKind::Config => "config",
            WatchPathKind::Stash => "stash",
            WatchPathKind::Ignore => "ignore",
        }
    }
}

/// `reason` de `git:status-changed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WatchReason {
    #[serde(rename = "fs-watch-head")]
    FsWatchHead,
    #[serde(rename = "fs-watch-working-tree")]
    FsWatchWorkingTree,
    #[serde(rename = "poll-head")]
    PollHead,
    #[serde(rename = "poll-working-tree")]
    PollWorkingTree,
    /// Cambiaron tags o ramas remotas (ver [`WatchPathKind::Refs`]).
    #[serde(rename = "fs-watch-refs")]
    FsWatchRefs,
    /// Cambió `.git/config` (ver [`WatchPathKind::Config`]).
    #[serde(rename = "fs-watch-config")]
    FsWatchConfig,
    #[serde(rename = "poll-refs")]
    PollRefs,
    #[serde(rename = "poll-config")]
    PollConfig,
    /// Cambió la lista de stashes (ver [`WatchPathKind::Stash`]).
    #[serde(rename = "fs-watch-stash")]
    FsWatchStash,
    #[serde(rename = "poll-stash")]
    PollStash,
}

impl WatchReason {
    pub fn as_str(self) -> &'static str {
        match self {
            WatchReason::FsWatchHead => "fs-watch-head",
            WatchReason::FsWatchWorkingTree => "fs-watch-working-tree",
            WatchReason::PollHead => "poll-head",
            WatchReason::PollWorkingTree => "poll-working-tree",
            WatchReason::FsWatchRefs => "fs-watch-refs",
            WatchReason::FsWatchConfig => "fs-watch-config",
            WatchReason::PollRefs => "poll-refs",
            WatchReason::PollConfig => "poll-config",
            WatchReason::FsWatchStash => "fs-watch-stash",
            WatchReason::PollStash => "poll-stash",
        }
    }

    /// Cambio de metadatos (tags, ramas remotas o configuración): no toca el
    /// working tree ni HEAD y se avisa aparte, sin fundirse con esos.
    pub fn is_metadata(self) -> bool {
        self.is_refs_change() || self.is_config_change() || self.is_stash_change()
    }

    /// Cambió la lista de stashes: hay que releerlos.
    pub fn is_stash_change(self) -> bool {
        matches!(self, WatchReason::FsWatchStash | WatchReason::PollStash)
    }

    /// Cambiaron tags o ramas remotas: hay que releer tags y remotos.
    pub fn is_refs_change(self) -> bool {
        matches!(self, WatchReason::FsWatchRefs | WatchReason::PollRefs)
    }

    /// Cambió `.git/config`: hay que releer los remotos.
    pub fn is_config_change(self) -> bool {
        matches!(self, WatchReason::FsWatchConfig | WatchReason::PollConfig)
    }

    /// El cambio pudo mover HEAD: hay que refrescar también las ramas.
    pub fn is_head_change(self) -> bool {
        matches!(self, WatchReason::FsWatchHead | WatchReason::PollHead)
    }

    fn is_fs_watch(self) -> bool {
        matches!(
            self,
            WatchReason::FsWatchHead
                | WatchReason::FsWatchWorkingTree
                | WatchReason::FsWatchRefs
                | WatchReason::FsWatchConfig
                | WatchReason::FsWatchStash
        )
    }
}

fn normalize_watch_path(filename: &str) -> String {
    filename.replace('\\', "/")
}

fn is_git_index_path(normalized_path: &str) -> bool {
    normalized_path == ".git/index" || normalized_path.ends_with("/.git/index")
}

fn is_git_config_path(normalized_path: &str) -> bool {
    normalized_path == ".git/config" || normalized_path.ends_with("/.git/config")
}

fn is_git_stash_path(normalized_path: &str) -> bool {
    [".git/refs/stash", ".git/logs/refs/stash"]
        .iter()
        .any(|stash| normalized_path == *stash || normalized_path.ends_with(&format!("/{stash}")))
}

fn is_git_refs_path(normalized_path: &str) -> bool {
    ["refs/tags", "refs/remotes"].iter().any(|refs| {
        normalized_path == format!(".git/{refs}")
            || normalized_path.starts_with(&format!(".git/{refs}/"))
            || normalized_path.ends_with(&format!("/.git/{refs}"))
            || normalized_path.contains(&format!("/.git/{refs}/"))
    })
}

fn is_git_metadata_head_path(normalized_path: &str) -> bool {
    normalized_path == ".git/HEAD"
        || normalized_path.ends_with("/.git/HEAD")
        || normalized_path == ".git/packed-refs"
        || normalized_path.ends_with("/.git/packed-refs")
        || normalized_path.starts_with(".git/refs/heads/")
        || normalized_path.contains("/.git/refs/heads/")
}

/// Clasifica una ruta relativa al repositorio. Una ruta vacía (el watcher no
/// supo decir qué cambió) cuenta como cambio del working tree.
pub fn classify_watch_path(filename: &str) -> WatchPathKind {
    if filename.is_empty() {
        return WatchPathKind::WorkingTree;
    }

    let normalized_path = normalize_watch_path(filename);

    if is_git_metadata_head_path(&normalized_path) {
        return WatchPathKind::Head;
    }

    // El índice cambia con `git add` / `git reset` hechos fuera de la app: sin
    // esto lo preparado en la terminal no se veía hasta reiniciar (QA 0.12.0).
    // Las lecturas de la app usan `--no-optional-locks` y no lo reescriben, así
    // que vigilarlo no crea un bucle. `index.lock` sigue ignorado.
    if is_git_index_path(&normalized_path) {
        return WatchPathKind::WorkingTree;
    }

    // Remotos y tags creados o borrados en la terminal (`git remote add`,
    // `git tag`, `git fetch`). Las lecturas de la app no escriben ni la
    // configuración ni las refs, así que vigilarlos no crea un bucle; sus
    // mutaciones (fetch, crear un tag…) se descartan como las demás.
    // `config.lock` y `*.lock` de refs no cuentan hasta el renombrado final.
    if is_git_config_path(&normalized_path) {
        return WatchPathKind::Config;
    }
    if is_git_refs_path(&normalized_path) && !normalized_path.ends_with(".lock") {
        return WatchPathKind::Refs;
    }
    // Stashes hechos o borrados en la terminal (`git stash`, `git stash drop`).
    if is_git_stash_path(&normalized_path) {
        return WatchPathKind::Stash;
    }

    if normalized_path == ".git"
        || normalized_path.starts_with(".git/")
        || normalized_path.contains("/.git/")
    {
        return WatchPathKind::Ignore;
    }

    // Lo que ignora el repositorio (`.gitignore`, `node_modules`…) ya lo
    // descarta la vigilancia, que sabe qué está versionado: un `dist/`
    // versionado sí cambia el estado.
    WatchPathKind::WorkingTree
}

/// `packed-refs` puede llevar también tags y ramas remotas (`git pack-refs`,
/// borrar un tag empaquetado, un fetch grande): su cambio cuenta además como
/// cambio de refs.
pub fn is_packed_refs_path(filename: &str) -> bool {
    let normalized_path = normalize_watch_path(filename);
    normalized_path == ".git/packed-refs" || normalized_path.ends_with("/.git/packed-refs")
}

pub fn should_ignore_watch_path(filename: &str) -> bool {
    classify_watch_path(filename) == WatchPathKind::Ignore
}

/// Qué directorios ni siquiera se vigilan, mirando solo la ruta.
/// `classify_watch_path` filtra eventos ya recibidos; esto poda el recorrido
/// antes de crear los watchers, que es lo que importa en un monorepo:
/// node_modules puede tener más carpetas que el resto del repositorio junto.
/// De `.git` solo interesa lo que cambia HEAD, el índice, la configuración,
/// las refs de tags y remotos y el stash (con su reflog en `logs/refs/stash`).
///
/// El watcher real usa además las reglas del repositorio (`.gitignore` y lo
/// que está versionado): ver [`super::ignore_rules`].
pub fn should_prune_watch_path(relative_path: &str) -> bool {
    let normalized_path = normalize_watch_path(relative_path);
    if normalized_path.is_empty() {
        return false;
    }

    prune_git_metadata(&normalized_path).unwrap_or_else(|| {
        normalized_path
            .split('/')
            .any(|segment| FALLBACK_PRUNED_DIRECTORIES.contains(&segment))
    })
}

/// Para una ruta dentro de `.git` (o de un `.git` anidado), si se poda.
/// `None` si la ruta no está dentro de un `.git`.
pub(crate) fn prune_git_metadata(relative_path: &str) -> Option<bool> {
    let normalized_path = normalize_watch_path(relative_path);
    let segments: Vec<&str> = normalized_path.split('/').collect();
    let git_index = segments.iter().position(|segment| *segment == ".git")?;
    let inside = segments[git_index + 1..].join("/");
    Some(
        !(inside.is_empty()
            || inside == "index"
            || inside == "HEAD"
            || inside == "packed-refs"
            || inside == "refs"
            || inside == "config"
            || inside == "logs"
            || inside == "logs/refs"
            || inside == "logs/refs/stash"
            || ["refs/heads", "refs/tags", "refs/remotes"]
                .iter()
                .any(|refs| inside == *refs || inside.starts_with(&format!("{refs}/")))),
    )
}

/// Junta el motivo pendiente con uno nuevo dentro de la misma ventana de
/// debounce: un cambio de HEAD se impone y `fs-watch` gana a `poll`.
pub(crate) fn merge_queued_reason(current: Option<WatchReason>, next: WatchReason) -> WatchReason {
    let Some(current) = current else {
        return next;
    };
    if current == next {
        return current;
    }

    let from_fs_watch = current.is_fs_watch() || next.is_fs_watch();
    if current.is_head_change() || next.is_head_change() {
        if from_fs_watch {
            WatchReason::FsWatchHead
        } else {
            WatchReason::PollHead
        }
    } else if from_fs_watch {
        WatchReason::FsWatchWorkingTree
    } else {
        WatchReason::PollWorkingTree
    }
}
