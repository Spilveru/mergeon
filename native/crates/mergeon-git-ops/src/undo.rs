//! Deshacer y rehacer operaciones (propio, sin original).
//!
//! Dos mitades:
//!
//! - **Captura** ([`RefProbe`] → [`RefCapture`]): lo que la cola de mutaciones
//!   lee justo antes y justo después de una operación, dentro de su turno (nada
//!   puede colarse entre medias): HEAD, la rama en la que está, los refs y las
//!   claves de config que la operación va a tocar y, en un descarte, una copia
//!   recuperable de lo que se va a perder ([`back_up_discard`]).
//! - **Pasos** ([`UndoStep`]): lo que devuelve una operación atrás o la
//!   repite. Cada paso comprueba antes que el repositorio sigue como lo dejó la
//!   operación (HEAD donde estaba, el ref con el mismo valor, el stash en la
//!   lista...) y, si no, no toca nada ([`Code::UndoStale`]). Nada se fuerza:
//!   los movimientos de HEAD usan `reset --soft`, `--mixed` o `--keep` (que se
//!   niega antes de pisar cambios locales, [`Code::UndoBlocked`]), el checkout
//!   no lleva `-f`, los refs se escriben con `update-ref` y su valor anterior
//!   esperado, y los archivos solo se restauran donde no hay nada.
//!
//! Un tag anotado borrado se recrea apuntando el ref a su mismo objeto tag
//! (que git conserva como objeto suelto hasta que `gc` poda los inalcanzables,
//! por defecto dos semanas): queda idéntico, con su mensaje, autor y firma.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use mergeon_gitcmd::{FileStatus, Git, get_structured_status};

use crate::exec::{OperationError, exec_checked, exec_with_stdin_checked, git};
use crate::messages::js_trim;
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, finalize_mutation_result,
};
use crate::stash::resolve_stash_ref;
use crate::types::{Code, MutationResult, Operation, Outcome};

/// Lo que se lee antes y después de una operación.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefProbe {
    /// Revisiones a resolver (`refs/heads/x`, `refs/tags/v1`, `stash@{2}`...).
    pub refs: Vec<String>,
    /// Claves de config (`branch.x.remote`, `branch.x.merge`).
    pub configs: Vec<String>,
    /// Copia de lo que va a descartar la operación (solo antes).
    pub discard: Option<DiscardProbe>,
}

impl RefProbe {
    /// Solo HEAD y la rama actual.
    pub fn head() -> Self {
        Self::default()
    }

    pub fn with_ref(mut self, reference: impl Into<String>) -> Self {
        self.refs.push(reference.into());
        self
    }

    /// `refs/heads/<rama>` y su upstream (`branch.<rama>.remote` / `.merge`).
    pub fn with_branch(self, branch: &str) -> Self {
        let mut probe = self.with_ref(format!("refs/heads/{branch}"));
        probe.configs.push(format!("branch.{branch}.remote"));
        probe.configs.push(format!("branch.{branch}.merge"));
        probe
    }

    pub fn with_discard(mut self, discard: DiscardProbe) -> Self {
        self.discard = Some(discard);
        self
    }
}

/// Qué descartar y dónde guardar la copia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardProbe {
    /// Las rutas que se van a descartar; `None`, todo lo que tenga cambios en
    /// el working tree (lo mismo que elige `discard_all_operation`).
    pub paths: Option<Vec<String>>,
    /// Carpeta nueva (o vacía) para la copia. Si al final no se guarda nada,
    /// se borra.
    pub directory: PathBuf,
    /// Bytes como mucho; por encima no se guarda copia.
    pub limit: u64,
}

/// HEAD, la rama actual, refs y claves de config en un momento dado.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefSnapshot {
    /// SHA del commit de HEAD; `None` en una rama sin commits.
    pub head: Option<String>,
    /// `refs/heads/<rama>`; `None` con HEAD desanclado.
    pub head_ref: Option<String>,
    /// Solo las revisiones que existían.
    pub refs: BTreeMap<String, String>,
    /// Solo las claves con valor.
    pub configs: BTreeMap<String, String>,
}

impl RefSnapshot {
    /// La rama actual sin `refs/heads/`.
    pub fn branch(&self) -> Option<&str> {
        self.head_ref
            .as_deref()
            .and_then(|reference| reference.strip_prefix("refs/heads/"))
    }

    pub fn rev(&self, name: &str) -> Option<&str> {
        self.refs.get(name).map(String::as_str)
    }

    pub fn config(&self, key: &str) -> Option<&str> {
        self.configs.get(key).map(String::as_str)
    }

    /// Dónde está HEAD, como destino de un checkout.
    pub fn head_target(&self) -> Option<HeadTarget> {
        match (self.branch(), self.head.as_deref()) {
            (Some(branch), _) => Some(HeadTarget::Branch(branch.to_string())),
            (None, Some(head)) => Some(HeadTarget::Detached(head.to_string())),
            (None, None) => None,
        }
    }
}

/// Lo leído antes y después de una operación.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefCapture {
    pub before: RefSnapshot,
    pub after: RefSnapshot,
    /// Resultado de la copia, si se pidió.
    pub discard: Option<DiscardBackupOutcome>,
}

/// Un archivo, enlace o carpeta sin seguimiento que se copió.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFile {
    /// Ruta relativa al repositorio, con `/`.
    pub path: String,
    pub kind: SavedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedKind {
    /// Contenido en `copy` (con sus permisos).
    File { copy: PathBuf },
    /// Enlace simbólico a `target`.
    Symlink { target: PathBuf },
    /// Carpeta (vacía o no): se vuelve a crear.
    Directory,
}

/// La copia de un descarte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardBackup {
    pub directory: PathBuf,
    /// Parche índice → working tree de los archivos con seguimiento.
    pub patch: Option<PathBuf>,
    /// Lo que no tiene seguimiento, carpetas antes que su contenido.
    pub untracked: Vec<SavedFile>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscardBackupOutcome {
    Saved(DiscardBackup),
    /// No había nada que guardar.
    Empty,
    /// Pasaba del límite (bytes medidos hasta parar): no se guardó nada.
    TooLarge(u64),
    /// No se pudo guardar (disco, permisos...).
    Failed(String),
}

// ----------------------------------------------------------------------
// Captura
// ----------------------------------------------------------------------

fn rev_parse(git: &Git, revision: &str) -> Option<String> {
    let output = git
        .exec(["rev-parse", "--verify", "--quiet", revision])
        .ok()?;
    output
        .success()
        .then(|| js_trim(&output.stdout_text()).to_string())
        .filter(|sha| !sha.is_empty())
}

fn symbolic_head(git: &Git) -> Option<String> {
    let output = git.exec(["symbolic-ref", "--quiet", "HEAD"]).ok()?;
    output
        .success()
        .then(|| js_trim(&output.stdout_text()).to_string())
        .filter(|reference| !reference.is_empty())
}

fn config_value(git: &Git, key: &str) -> Option<String> {
    let output = git.exec(["config", "--get", key]).ok()?;
    output
        .success()
        .then(|| js_trim(&output.stdout_text()).to_string())
        .filter(|value| !value.is_empty())
}

/// Lee HEAD, la rama actual y lo que pide `probe` (sin la copia del descarte).
pub fn read_ref_snapshot(repo_path: &Path, probe: &RefProbe) -> RefSnapshot {
    let git = git(repo_path);
    let mut snapshot = RefSnapshot {
        head: rev_parse(&git, "HEAD"),
        head_ref: symbolic_head(&git),
        ..Default::default()
    };
    for reference in &probe.refs {
        if let Some(sha) = rev_parse(&git, reference) {
            snapshot.refs.insert(reference.clone(), sha);
        }
    }
    for key in &probe.configs {
        if let Some(value) = config_value(&git, key) {
            snapshot.configs.insert(key.clone(), value);
        }
    }
    snapshot
}

fn is_untracked(file: &FileStatus) -> bool {
    file.working_dir == "?" || file.working_dir == "??" || file.index == "?"
}

fn has_working_tree_changes(file: &FileStatus) -> bool {
    !file.working_dir.is_empty() && file.working_dir != " " && file.working_dir != "U"
}

/// Lo que `discard_all_operation` / `discard_changes_operation` descartarían:
/// las rutas con cambios en el working tree, separadas en con y sin
/// seguimiento.
fn discard_targets(
    repo_path: &Path,
    paths: Option<&[String]>,
) -> Result<(Vec<String>, Vec<String>), OperationError> {
    let status = get_structured_status(repo_path)?;
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for file in &status.files {
        if !has_working_tree_changes(file) {
            continue;
        }
        if let Some(paths) = paths
            && !paths.contains(&file.path)
        {
            continue;
        }
        if is_untracked(file) {
            untracked.push(file.path.clone());
        } else {
            tracked.push(file.path.clone());
        }
    }
    Ok((tracked, untracked))
}

/// Recorre `relative` (archivo, enlace o carpeta) y lo añade a `out`.
fn collect_untracked(
    repo_path: &Path,
    relative: &str,
    out: &mut Vec<(String, std::fs::Metadata)>,
) -> std::io::Result<()> {
    let relative = relative.trim_end_matches('/');
    let absolute = repo_path.join(relative);
    let metadata = match std::fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let is_dir = metadata.is_dir();
    out.push((relative.to_string(), metadata));
    if is_dir {
        let mut children: Vec<String> = std::fs::read_dir(&absolute)?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        children.sort();
        for child in children {
            collect_untracked(repo_path, &format!("{relative}/{child}"), out)?;
        }
    }
    Ok(())
}

/// Parches de muchas rutas, por tandas (sin pasar del límite de la línea de
/// órdenes de Windows).
const PATHS_PER_DIFF: usize = 200;

/// Guarda lo que un descarte va a perder: el parche índice → working tree de
/// los archivos con seguimiento (`git diff --binary`, que también cubre
/// binarios) y una copia byte a byte de lo que no tiene seguimiento (archivos,
/// enlaces y carpetas enteras, como las borra el descarte). Se llama dentro
/// del turno de la cola, justo antes de descartar.
pub fn back_up_discard(repo_path: &Path, probe: &DiscardProbe) -> DiscardBackupOutcome {
    let outcome = back_up_discard_inner(repo_path, probe);
    if !matches!(outcome, DiscardBackupOutcome::Saved(_)) {
        let _ = std::fs::remove_dir_all(&probe.directory);
    }
    outcome
}

fn back_up_discard_inner(repo_path: &Path, probe: &DiscardProbe) -> DiscardBackupOutcome {
    let (tracked, untracked) = match discard_targets(repo_path, probe.paths.as_deref()) {
        Ok(targets) => targets,
        Err(error) => return DiscardBackupOutcome::Failed(error.message),
    };
    if tracked.is_empty() && untracked.is_empty() {
        return DiscardBackupOutcome::Empty;
    }

    let mut entries = Vec::new();
    for path in &untracked {
        if let Err(error) = collect_untracked(repo_path, path, &mut entries) {
            return DiscardBackupOutcome::Failed(error.to_string());
        }
    }
    let mut bytes: u64 = entries
        .iter()
        .filter(|(_, metadata)| metadata.is_file())
        .map(|(_, metadata)| metadata.len())
        .sum();
    if bytes > probe.limit {
        return DiscardBackupOutcome::TooLarge(bytes);
    }

    let mut patch = Vec::new();
    if !tracked.is_empty() {
        let runner = git(repo_path).with_env("GIT_LITERAL_PATHSPECS", "1");
        for chunk in tracked.chunks(PATHS_PER_DIFF) {
            let remaining = probe.limit.saturating_sub(bytes + patch.len() as u64);
            let mut arguments = vec![
                "diff".to_string(),
                "--binary".to_string(),
                "--ignore-submodules=all".to_string(),
                "--".to_string(),
            ];
            arguments.extend(chunk.iter().cloned());
            let limit = usize::try_from(remaining.saturating_add(1)).unwrap_or(usize::MAX);
            let output = match runner.exec_capped(&arguments, limit) {
                Ok(output) => output,
                Err(error) => return DiscardBackupOutcome::Failed(error.message),
            };
            if output.exceeded {
                return DiscardBackupOutcome::TooLarge(bytes + patch.len() as u64 + remaining + 1);
            }
            if !output.output.success() {
                return DiscardBackupOutcome::Failed(output.output.stderr_text());
            }
            patch.extend_from_slice(&output.output.stdout);
        }
    }
    bytes += patch.len() as u64;
    if bytes > probe.limit {
        return DiscardBackupOutcome::TooLarge(bytes);
    }

    match write_backup(repo_path, probe, &patch, &entries) {
        Ok(backup) => DiscardBackupOutcome::Saved(DiscardBackup { bytes, ..backup }),
        Err(error) => DiscardBackupOutcome::Failed(error.to_string()),
    }
}

fn write_backup(
    repo_path: &Path,
    probe: &DiscardProbe,
    patch: &[u8],
    entries: &[(String, std::fs::Metadata)],
) -> std::io::Result<DiscardBackup> {
    std::fs::create_dir_all(&probe.directory)?;
    let patch_path = if patch.is_empty() {
        None
    } else {
        let path = probe.directory.join("changes.patch");
        std::fs::write(&path, patch)?;
        Some(path)
    };
    let files_dir = probe.directory.join("files");
    let mut untracked = Vec::new();
    for (index, (relative, metadata)) in entries.iter().enumerate() {
        let absolute = repo_path.join(relative);
        let kind = if metadata.file_type().is_symlink() {
            SavedKind::Symlink {
                target: std::fs::read_link(&absolute)?,
            }
        } else if metadata.is_dir() {
            SavedKind::Directory
        } else {
            std::fs::create_dir_all(&files_dir)?;
            let copy = files_dir.join(index.to_string());
            // `fs::copy` conserva los permisos (el bit de ejecución).
            std::fs::copy(&absolute, &copy)?;
            SavedKind::File { copy }
        };
        untracked.push(SavedFile {
            path: relative.clone(),
            kind,
        });
    }
    Ok(DiscardBackup {
        directory: probe.directory.clone(),
        patch: patch_path,
        untracked,
        bytes: 0,
    })
}

/// Bytes que ocuparía la copia de un descarte, sin guardarla: el tamaño de
/// los archivos implicados (con y sin seguimiento). Una estimación barata
/// para avisar antes de descartar algo enorme.
pub fn estimate_discard_backup_size(repo_path: &Path, paths: Option<&[String]>) -> u64 {
    let Ok((tracked, untracked)) = discard_targets(repo_path, paths) else {
        return 0;
    };
    let mut entries = Vec::new();
    for path in tracked.iter().chain(&untracked) {
        let _ = collect_untracked(repo_path, path, &mut entries);
    }
    entries
        .iter()
        .filter(|(_, metadata)| metadata.is_file())
        .map(|(_, metadata)| metadata.len())
        .sum()
}

/// `Some(upstream)` si mover la rama `branch` de `from` a `to` dejaría fuera
/// commits que ya están en su upstream (deshacer algo publicado: luego haría
/// falta un force push). `None` si no tiene upstream o no toca nada publicado.
pub fn published_upstream(repo_path: &Path, branch: &str, from: &str, to: &str) -> Option<String> {
    let git = git(repo_path);
    let upstream_ref = format!("{branch}@{{upstream}}");
    let upstream = rev_parse(&git, &upstream_ref)?;
    let name = git
        .exec([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            &upstream_ref,
        ])
        .ok()
        .filter(|output| output.success())
        .map(|output| js_trim(&output.stdout_text()).to_string())
        .unwrap_or_else(|| upstream.chars().take(7).collect());
    let bases = git
        .exec(["merge-base", "--all", from, &upstream])
        .ok()
        .filter(|output| output.success())
        .map(|output| output.stdout_text())
        .unwrap_or_default();
    // Lo común a `from` y al upstream son los ancestros de sus bases: si
    // todas están en `to`, no se pierde nada publicado.
    let dropped = bases
        .split_whitespace()
        .any(|base| !is_ancestor(&git, base, to));
    dropped.then_some(name)
}

fn is_ancestor(git: &Git, ancestor: &str, descendant: &str) -> bool {
    git.exec(["merge-base", "--is-ancestor", ancestor, descendant])
        .is_ok_and(|output| output.success())
}

// ----------------------------------------------------------------------
// Pasos
// ----------------------------------------------------------------------

/// Cómo mueve HEAD un paso de deshacer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResetMode {
    /// `--soft`: los cambios del commit deshecho quedan preparados.
    Soft,
    /// `--mixed`: índice al destino, el working tree no se toca.
    Mixed,
    /// `--keep`: como `--hard`, pero se niega si pisaría cambios locales.
    Keep,
}

impl ResetMode {
    fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Keep => "--keep",
        }
    }
}

/// Dónde está HEAD.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HeadTarget {
    Branch(String),
    Detached(String),
}

impl HeadTarget {
    pub fn label(&self) -> String {
        match self {
            Self::Branch(branch) => branch.clone(),
            Self::Detached(sha) => sha.chars().take(7).collect(),
        }
    }
}

/// Un parche, en memoria o en un archivo de la copia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchData {
    Text(String),
    File(PathBuf),
}

/// Un paso de deshacer o rehacer. Comprueba antes que el repositorio sigue
/// como lo dejó la operación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoStep {
    /// HEAD (en `branch`, o desanclado) de `from` a `to` con `git reset`.
    MoveHead {
        branch: Option<String>,
        from: String,
        to: String,
        mode: ResetMode,
    },
    /// Checkout de `from` a `to`, sin forzar.
    Checkout {
        from: HeadTarget,
        to: HeadTarget,
    },
    /// Crea la rama (que no debe existir) en `sha`, con su upstream.
    CreateBranch {
        name: String,
        sha: String,
        upstream: Option<(String, String)>,
    },
    /// Borra la rama si sigue en `sha`.
    DeleteBranch {
        name: String,
        sha: String,
    },
    RenameBranch {
        from: String,
        to: String,
    },
    /// Crea el tag (que no debe existir) apuntando a `sha` (un commit o, en
    /// un tag anotado, su objeto tag).
    CreateTag {
        name: String,
        sha: String,
    },
    /// Borra el tag si sigue apuntando a `sha`.
    DeleteTag {
        name: String,
        sha: String,
    },
    /// Vuelve a poner el stash `sha` en la lista (`git stash store`).
    StashStore {
        sha: String,
    },
    /// Quita de la lista el stash `sha`.
    StashDrop {
        sha: String,
    },
    /// Aplica el stash `sha`; con `pop`, además lo quita.
    StashApply {
        sha: String,
        pop: bool,
    },
    /// Quita del working tree lo que aplicó el stash `sha`.
    StashUnapply {
        sha: String,
    },
    /// Aplica un parche al working tree (`reverse`: lo quita).
    ApplyPatch {
        patch: PatchData,
        reverse: bool,
    },
    /// Restaura archivos sin seguimiento donde ya no hay nada.
    RestoreFiles {
        files: Vec<SavedFile>,
    },
    /// Vuelve a borrar archivos sin seguimiento si siguen iguales a la copia.
    RemoveFiles {
        files: Vec<SavedFile>,
    },
}

/// Por qué no se pudo dar un paso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepError {
    /// El repositorio ya no está como lo dejó la operación.
    Stale(String),
    /// Git se negó para no perder cambios locales.
    Blocked(String),
    Failed(OperationError),
}

impl From<OperationError> for StepError {
    fn from(error: OperationError) -> Self {
        Self::Failed(error)
    }
}

impl From<mergeon_gitcmd::GitError> for StepError {
    fn from(error: mergeon_gitcmd::GitError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<std::io::Error> for StepError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.into())
    }
}

type StepResult = Result<(), StepError>;

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Archivos que git nombra en "Entry 'x' not uptodate", "would be
/// overwritten" y similares.
fn files_in_git_error(message: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    // Tras "would be overwritten", git lista los archivos con un tabulador.
    let mut listed = false;
    for raw_line in message.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("error: Entry '")
            && let Some(end) = rest.find('\'')
        {
            files.push(rest[..end].to_string());
        }
        if raw_line.contains("would be overwritten") {
            listed = true;
        } else if listed {
            match raw_line.strip_prefix('\t') {
                Some(path) => files.push(path.trim().to_string()),
                None => listed = false,
            }
        }
    }
    files.dedup();
    files
}

fn blocked_by_local_changes(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("not uptodate")
        || message.contains("would be overwritten")
        || message.contains("local changes")
        || message.contains("cannot merge")
}

fn local_changes_message(error: &mergeon_gitcmd::GitError) -> StepError {
    if !blocked_by_local_changes(&error.message) {
        return StepError::Failed(error.clone().into());
    }
    let files = files_in_git_error(&error.message);
    let which = match files.as_slice() {
        [] => "your local changes".to_string(),
        [one] => format!("your local changes to {one}"),
        [first, rest @ ..] => format!(
            "your local changes to {first} and {} other file{}",
            rest.len(),
            if rest.len() == 1 { "" } else { "s" }
        ),
    };
    StepError::Blocked(format!(
        "it would overwrite {which}. Commit, stash or discard them first."
    ))
}

fn check_head(git: &Git, branch: Option<&str>, at: &str) -> StepResult {
    let head = rev_parse(git, "HEAD");
    let head_ref = symbolic_head(git);
    let expected_ref = branch.map(|branch| format!("refs/heads/{branch}"));
    if head_ref != expected_ref {
        let now = head_ref
            .as_deref()
            .and_then(|reference| reference.strip_prefix("refs/heads/"))
            .map_or_else(|| "a detached HEAD".to_string(), |b| format!("'{b}'"));
        return Err(StepError::Stale(format!(
            "the repository is now on {now}, not where the operation left it."
        )));
    }
    if head.as_deref() != Some(at) {
        return Err(StepError::Stale(format!(
            "HEAD moved since (it was {}, now {}).",
            short(at),
            head.as_deref().map_or_else(|| "nothing".to_string(), short)
        )));
    }
    Ok(())
}

fn move_head(git: &Git, branch: Option<&str>, from: &str, to: &str, mode: ResetMode) -> StepResult {
    check_head(git, branch, from)?;
    if rev_parse(git, &format!("{to}^{{commit}}")).is_none() {
        return Err(StepError::Stale(format!(
            "commit {} no longer exists.",
            short(to)
        )));
    }
    exec_checked(git, &["reset", "--quiet", mode.flag(), to])
        .map(|_| ())
        .map_err(|error| local_changes_message(&error))
}

fn check_target(git: &Git, target: &HeadTarget) -> StepResult {
    let head_ref = symbolic_head(git);
    let ok = match target {
        HeadTarget::Branch(branch) => head_ref.as_deref() == Some(&format!("refs/heads/{branch}")),
        HeadTarget::Detached(sha) => {
            head_ref.is_none() && rev_parse(git, "HEAD").as_deref() == Some(sha.as_str())
        }
    };
    if ok {
        Ok(())
    } else {
        Err(StepError::Stale(format!(
            "HEAD is no longer at {}.",
            match target {
                HeadTarget::Branch(branch) => format!("'{branch}'"),
                HeadTarget::Detached(sha) => short(sha),
            }
        )))
    }
}

fn checkout(git: &Git, from: &HeadTarget, to: &HeadTarget) -> StepResult {
    check_target(git, from)?;
    let result = match to {
        HeadTarget::Branch(branch) => {
            if rev_parse(git, &format!("refs/heads/{branch}")).is_none() {
                return Err(StepError::Stale(format!(
                    "branch '{branch}' no longer exists."
                )));
            }
            exec_checked(git, &["checkout", "--quiet", branch.as_str(), "--"])
        }
        HeadTarget::Detached(sha) => exec_checked(git, &["checkout", "--quiet", "--detach", sha]),
    };
    result
        .map(|_| ())
        .map_err(|error| local_changes_message(&error))
}

fn create_ref(git: &Git, reference: &str, sha: &str, what: &str) -> StepResult {
    if rev_parse(git, reference).is_some() {
        return Err(StepError::Stale(format!("{what} already exists.")));
    }
    if rev_parse(git, &format!("{sha}^{{object}}")).is_none() {
        return Err(StepError::Stale(format!(
            "{what} can't be recreated: object {} is gone.",
            short(sha)
        )));
    }
    // Valor anterior vacío: falla si alguien la crea entre medias.
    exec_checked(git, &["update-ref", reference, sha, ""])?;
    Ok(())
}

fn check_ref(git: &Git, reference: &str, sha: &str, what: &str) -> StepResult {
    match rev_parse(git, reference) {
        Some(current) if current == sha => Ok(()),
        Some(_) => Err(StepError::Stale(format!("{what} moved since."))),
        None => Err(StepError::Stale(format!("{what} no longer exists."))),
    }
}

fn create_branch(
    git: &Git,
    name: &str,
    sha: &str,
    upstream: Option<&(String, String)>,
) -> StepResult {
    create_ref(
        git,
        &format!("refs/heads/{name}"),
        sha,
        &format!("Branch '{name}'"),
    )?;
    if let Some((remote, merge)) = upstream {
        exec_checked(git, &["config", &format!("branch.{name}.remote"), remote])?;
        exec_checked(git, &["config", &format!("branch.{name}.merge"), merge])?;
    }
    Ok(())
}

fn delete_branch(git: &Git, name: &str, sha: &str) -> StepResult {
    let what = format!("Branch '{name}'");
    check_ref(git, &format!("refs/heads/{name}"), sha, &what)?;
    if symbolic_head(git).as_deref() == Some(&format!("refs/heads/{name}")) {
        return Err(StepError::Stale(format!("{what} is checked out.")));
    }
    // `-D` también quita su configuración (upstream).
    exec_checked(git, &["branch", "-D", name])?;
    Ok(())
}

fn rename_branch(git: &Git, from: &str, to: &str) -> StepResult {
    if rev_parse(git, &format!("refs/heads/{from}")).is_none() {
        return Err(StepError::Stale(format!(
            "branch '{from}' no longer exists."
        )));
    }
    if rev_parse(git, &format!("refs/heads/{to}")).is_some() {
        return Err(StepError::Stale(format!("branch '{to}' already exists.")));
    }
    exec_checked(git, &["branch", "-m", from, to])?;
    Ok(())
}

fn delete_tag(git: &Git, name: &str, sha: &str) -> StepResult {
    let reference = format!("refs/tags/{name}");
    let what = format!("Tag '{name}'");
    check_ref(git, &reference, sha, &what)?;
    exec_checked(git, &["update-ref", "-d", &reference, sha])?;
    Ok(())
}

fn stash_in_list(repo_path: &Path, sha: &str) -> Option<String> {
    resolve_stash_ref(repo_path, sha).filter(|reference| reference.starts_with("stash@{"))
}

fn stash_store(repo_path: &Path, git: &Git, sha: &str) -> StepResult {
    if stash_in_list(repo_path, sha).is_some() {
        return Err(StepError::Stale(format!(
            "stash {} is already in the list.",
            short(sha)
        )));
    }
    if rev_parse(git, &format!("{sha}^{{commit}}")).is_none() {
        return Err(StepError::Stale(format!("stash {} is gone.", short(sha))));
    }
    // El asunto del commit del stash es el mensaje que tenía en la lista
    // ("On main: …", "WIP on main: …").
    let subject = exec_checked(git, &["log", "-1", "--format=%s", sha])?;
    let subject = js_trim(&subject).to_string();
    exec_checked(git, &["stash", "store", "-m", subject.as_str(), sha])?;
    Ok(())
}

fn stash_drop(repo_path: &Path, git: &Git, sha: &str) -> StepResult {
    let Some(reference) = stash_in_list(repo_path, sha) else {
        return Err(StepError::Stale(format!(
            "stash {} is no longer in the list.",
            short(sha)
        )));
    };
    exec_checked(git, &["stash", "drop", "--quiet", reference.as_str()])?;
    Ok(())
}

fn stash_apply(repo_path: &Path, git: &Git, sha: &str, pop: bool) -> StepResult {
    let Some(reference) = stash_in_list(repo_path, sha) else {
        return Err(StepError::Stale(format!(
            "stash {} is no longer in the list.",
            short(sha)
        )));
    };
    let subcommand = if pop { "pop" } else { "apply" };
    // Con `--index` vuelve también lo que estaba preparado; si el índice no
    // se puede reconstruir, git se niega sin tocar nada y se aplica sin él.
    let with_index = exec_checked(git, &["stash", subcommand, "--index", reference.as_str()]);
    match with_index {
        Ok(_) => Ok(()),
        Err(error) if error.message.contains("--index") => {
            exec_checked(git, &["stash", subcommand, reference.as_str()])
                .map(|_| ())
                .map_err(|error| local_changes_message(&error))
        }
        Err(error) => Err(local_changes_message(&error)),
    }
}

fn hash_object(git: &Git, path: &Path) -> Option<String> {
    let text = path.to_string_lossy().into_owned();
    let output = git
        .exec(["hash-object", "--no-filters", "--", text.as_str()])
        .ok()?;
    output
        .success()
        .then(|| js_trim(&output.stdout_text()).to_string())
}

/// Quita del working tree lo que aplicó el stash `sha`: el parche de lo que
/// se sigue al revés (atómico: si algo cambió después, no se toca nada), el
/// índice de esas rutas otra vez como HEAD y los archivos sin seguimiento
/// que trajo, solo si siguen idénticos.
fn stash_unapply(repo_path: &Path, git: &Git, sha: &str) -> StepResult {
    if rev_parse(git, &format!("{sha}^{{commit}}")).is_none() {
        return Err(StepError::Stale(format!("stash {} is gone.", short(sha))));
    }
    // Lo que no se seguía (tercer padre, si lo hay): debe seguir igual.
    let mut untracked: Vec<(String, String)> = Vec::new();
    let third = format!("{sha}^3");
    if rev_parse(git, &third).is_some() {
        let listing = exec_checked(git, &["ls-tree", "-r", "-z", third.as_str()])?;
        for entry in listing.split('\0').filter(|entry| !entry.is_empty()) {
            let Some((meta, path)) = entry.split_once('\t') else {
                continue;
            };
            let blob = meta.split_whitespace().nth(2).unwrap_or_default();
            untracked.push((path.to_string(), blob.to_string()));
        }
    }
    for (path, blob) in &untracked {
        let absolute = repo_path.join(path);
        if hash_object(git, &absolute).as_deref() != Some(blob.as_str()) {
            return Err(StepError::Blocked(format!(
                "{path} changed after the stash was applied."
            )));
        }
    }

    let base = format!("{sha}^1");
    let patch = git.raw_bytes([
        "diff",
        "--binary",
        "--ignore-submodules=all",
        base.as_str(),
        sha,
    ])?;
    let names = exec_checked(
        git,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            base.as_str(),
            sha,
        ],
    )?;
    let paths: Vec<&str> = names.split('\0').filter(|name| !name.is_empty()).collect();
    if !patch.is_empty() {
        apply_patch_bytes(git, &patch, true)?;
    }
    if !paths.is_empty() {
        let mut stdin = Vec::new();
        for path in &paths {
            stdin.extend_from_slice(path.as_bytes());
            stdin.push(0);
        }
        exec_with_stdin_checked(
            git,
            &[
                "reset",
                "--quiet",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &stdin,
        )?;
    }
    for (path, _) in &untracked {
        remove_file(&repo_path.join(path))?;
    }
    Ok(())
}

fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn apply_patch_file(git: &Git, file: &Path, reverse: bool) -> StepResult {
    let file = file.to_string_lossy().into_owned();
    let mut check = vec!["apply", "--check", "--whitespace=nowarn"];
    let mut apply = vec!["apply", "--whitespace=nowarn"];
    if reverse {
        check.push("--reverse");
        apply.push("--reverse");
    }
    check.push(&file);
    apply.push(&file);
    if let Err(error) = exec_checked(git, &check) {
        let detail = error
            .message
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("error:"))
            .unwrap_or("the files changed since")
            .to_string();
        return Err(StepError::Blocked(format!(
            "the changed files no longer match ({detail})."
        )));
    }
    exec_checked(git, &apply)?;
    Ok(())
}

fn apply_patch_bytes(git: &Git, patch: &[u8], reverse: bool) -> StepResult {
    let directory = tempfile::Builder::new().prefix("mergeon-undo-").tempdir()?;
    let file = directory.path().join("undo.patch");
    std::fs::write(&file, patch)?;
    apply_patch_file(git, &file, reverse)
}

fn same_content(left: &Path, right: &Path) -> std::io::Result<bool> {
    let (left_meta, right_meta) = (std::fs::metadata(left)?, std::fs::metadata(right)?);
    if left_meta.len() != right_meta.len() {
        return Ok(false);
    }
    let (mut left, mut right) = (std::fs::File::open(left)?, std::fs::File::open(right)?);
    let (mut a, mut b) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let read = left.read(&mut a)?;
        if read == 0 {
            return Ok(true);
        }
        right.read_exact(&mut b[..read])?;
        if a[..read] != b[..read] {
            return Ok(false);
        }
    }
}

fn restore_files(repo_path: &Path, files: &[SavedFile]) -> StepResult {
    // Primero se comprueba todo: no se escribe nada si algo ya existe.
    for file in files {
        if matches!(file.kind, SavedKind::Directory) {
            continue;
        }
        if std::fs::symlink_metadata(repo_path.join(&file.path)).is_ok() {
            return Err(StepError::Blocked(format!(
                "{} exists again; move it away to restore the discarded copy.",
                file.path
            )));
        }
    }
    for file in files {
        let target = repo_path.join(&file.path);
        match &file.kind {
            SavedKind::Directory => std::fs::create_dir_all(&target)?,
            SavedKind::File { copy } => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(copy, &target)?;
            }
            SavedKind::Symlink { target: link } => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                make_symlink(link, &target)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_symlink(link: &Path, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link, at)
}

#[cfg(not(unix))]
fn make_symlink(link: &Path, at: &Path) -> std::io::Result<()> {
    // En Windows git escribe los enlaces como archivos con la ruta.
    std::fs::write(at, link.to_string_lossy().as_bytes())
}

fn remove_files(repo_path: &Path, files: &[SavedFile]) -> StepResult {
    for file in files {
        let target = repo_path.join(&file.path);
        let unchanged = match &file.kind {
            SavedKind::Directory => true,
            SavedKind::File { copy } => same_content(&target, copy).unwrap_or(false),
            SavedKind::Symlink { target: link } => {
                std::fs::read_link(&target).is_ok_and(|current| current == *link)
            }
        };
        if !unchanged {
            return Err(StepError::Blocked(format!(
                "{} changed after it was restored.",
                file.path
            )));
        }
    }
    for file in files {
        if !matches!(file.kind, SavedKind::Directory) {
            remove_file(&repo_path.join(&file.path))?;
        }
    }
    // Las carpetas, de la más honda a la más alta, si quedaron vacías.
    for file in files.iter().rev() {
        if matches!(file.kind, SavedKind::Directory) {
            let _ = std::fs::remove_dir(repo_path.join(&file.path));
        }
    }
    Ok(())
}

fn run_step(repo_path: &Path, git: &Git, step: &UndoStep) -> StepResult {
    match step {
        UndoStep::MoveHead {
            branch,
            from,
            to,
            mode,
        } => move_head(git, branch.as_deref(), from, to, *mode),
        UndoStep::Checkout { from, to } => checkout(git, from, to),
        UndoStep::CreateBranch {
            name,
            sha,
            upstream,
        } => create_branch(git, name, sha, upstream.as_ref()),
        UndoStep::DeleteBranch { name, sha } => delete_branch(git, name, sha),
        UndoStep::RenameBranch { from, to } => rename_branch(git, from, to),
        UndoStep::CreateTag { name, sha } => create_ref(
            git,
            &format!("refs/tags/{name}"),
            sha,
            &format!("Tag '{name}'"),
        ),
        UndoStep::DeleteTag { name, sha } => delete_tag(git, name, sha),
        UndoStep::StashStore { sha } => stash_store(repo_path, git, sha),
        UndoStep::StashDrop { sha } => stash_drop(repo_path, git, sha),
        UndoStep::StashApply { sha, pop } => stash_apply(repo_path, git, sha, *pop),
        UndoStep::StashUnapply { sha } => stash_unapply(repo_path, git, sha),
        UndoStep::ApplyPatch { patch, reverse } => match patch {
            PatchData::Text(text) => apply_patch_bytes(git, text.as_bytes(), *reverse),
            PatchData::File(file) => apply_patch_file(git, file, *reverse),
        },
        UndoStep::RestoreFiles { files } => restore_files(repo_path, files),
        UndoStep::RemoveFiles { files } => remove_files(repo_path, files),
    }
}

/// Deshace (o, con `redo`, rehace) una operación dando sus pasos en orden.
/// `label` es lo que se deshace ("commit 'Fix login'"): va en el mensaje.
///
/// - Todo bien: `completed`, "Undid commit 'Fix login'.".
/// - El primer paso ve que el repositorio cambió por fuera:
///   [`Code::UndoStale`] (`blocked`), sin tocar nada.
/// - Git se niega por cambios locales: [`Code::UndoBlocked`] (`blocked`),
///   sin tocar nada.
/// - Falla un paso posterior: `failed`, y el mensaje dice que quedó a medias.
pub fn undo_operation(
    repo_path: &Path,
    label: &str,
    steps: &[UndoStep],
    redo: bool,
) -> MutationResult {
    let operation = if redo {
        Operation::Redo
    } else {
        Operation::Undo
    };
    let verb = if redo { "redo" } else { "undo" };
    let git = git(repo_path);
    for (index, step) in steps.iter().enumerate() {
        let Err(error) = run_step(repo_path, &git, step) else {
            continue;
        };
        if let StepError::Failed(error) = &error
            && error.control_code().is_some()
        {
            return build_mutation_error_result(
                repo_path,
                operation,
                error,
                MutationExtra::default(),
            );
        }
        let halfway = index > 0;
        let payload = match error {
            StepError::Stale(reason) if !halfway => MutationPayload::failure(
                operation,
                Outcome::Blocked,
                Code::UndoStale,
                format!("Can't {verb} {label}: {reason}"),
            ),
            StepError::Blocked(reason) if !halfway => MutationPayload::failure(
                operation,
                Outcome::Blocked,
                Code::UndoBlocked,
                format!("Can't {verb} {label}: {reason}"),
            ),
            StepError::Stale(reason) | StepError::Blocked(reason) => MutationPayload::failure(
                operation,
                Outcome::Partial,
                Code::PartialFailure,
                format!("The {verb} of {label} stopped halfway: {reason}"),
            ),
            StepError::Failed(error) => {
                let detail = js_trim(&error.message).to_string();
                MutationPayload::failure(
                    operation,
                    if halfway {
                        Outcome::Partial
                    } else {
                        Outcome::Failed
                    },
                    if halfway {
                        Code::PartialFailure
                    } else {
                        Code::GitError
                    },
                    if halfway {
                        format!("The {verb} of {label} stopped halfway: {detail}")
                    } else {
                        format!("Can't {verb} {label}: {detail}")
                    },
                )
            }
        };
        return finalize_mutation_result(repo_path, payload);
    }
    let done = if redo { "Redid" } else { "Undid" };
    finalize_mutation_result(
        repo_path,
        MutationPayload::completed(operation, format!("{done} {label}.")),
    )
}
