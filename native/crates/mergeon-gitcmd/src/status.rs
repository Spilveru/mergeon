//! Estado del repositorio.
//!
//! [`parse_status_summary`] replica el parser de `git status --porcelain -b -u
//! --null` de simple-git (incluidas sus rarezas), porque todo el código de la
//! app está escrito contra esas listas. [`get_structured_status`] es el port de
//! `getStructuredStatus` de `electron/services/git-operations.js`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::runner::{Git, GitError, GitResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    /// Columna X de porcelain (índice).
    pub index: String,
    /// Columna Y de porcelain (working tree).
    pub working_dir: String,
    /// Ruta de origen, solo en renombres.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenamedFile {
    pub from: String,
    pub to: String,
}

/// Equivalente a `StatusSummary` de simple-git.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusSummary {
    pub not_added: Vec<String>,
    pub conflicted: Vec<String>,
    pub created: Vec<String>,
    pub deleted: Vec<String>,
    pub ignored: Option<Vec<String>>,
    pub modified: Vec<String>,
    pub renamed: Vec<RenamedFile>,
    pub files: Vec<FileStatus>,
    pub staged: Vec<String>,
    pub ahead: u32,
    pub behind: u32,
    pub current: Option<String>,
    pub tracking: Option<String>,
    pub detached: bool,
}

impl StatusSummary {
    pub fn is_clean(&self) -> bool {
        self.files.is_empty()
    }
}

fn renamed_file(line: &str) -> RenamedFile {
    let mut parts = line.split('\0');
    let to = parts.next().unwrap_or_default().to_string();
    let from = parts.next().filter(|from| !from.is_empty());
    RenamedFile {
        from: from.map(str::to_string).unwrap_or_else(|| to.clone()),
        to,
    }
}

fn file_status(path: &str, index: char, working_dir: char) -> FileStatus {
    if index == 'R' || working_dir == 'R' {
        // /^(.+)\0(.+)$/: destino, NUL, origen.
        if let Some((to, from)) = path
            .split_once('\0')
            .filter(|(to, from)| !to.is_empty() && !from.is_empty())
        {
            return FileStatus {
                path: to.to_string(),
                index: index.to_string(),
                working_dir: working_dir.to_string(),
                from: Some(from.to_string()),
            };
        }
        return FileStatus {
            path: path.to_string(),
            index: index.to_string(),
            working_dir: working_dir.to_string(),
            from: Some(path.to_string()),
        };
    }
    FileStatus {
        path: path.to_string(),
        index: index.to_string(),
        working_dir: working_dir.to_string(),
        from: None,
    }
}

fn first_number_after(line: &str, keyword: &str) -> u32 {
    line.find(keyword)
        .map(|at| &line[at + keyword.len()..])
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

fn parse_branch_line(result: &mut StatusSummary, line: &str) {
    result.ahead = first_number_after(line, "ahead ");
    result.behind = first_number_after(line, "behind ");

    // /^(.+?(?=(?:\.{3}|\s|$)))/: prefijo no vacío más corto seguido de "...",
    // espacio o fin de línea.
    result.current = None;
    for (at, _) in line
        .char_indices()
        .skip(1)
        .chain(std::iter::once((line.len(), ' ')))
    {
        let rest = &line[at..];
        if rest.is_empty() || rest.starts_with("...") || rest.starts_with(char::is_whitespace) {
            result.current = Some(line[..at].to_string());
            break;
        }
    }

    // /\.{3}(\S*)/
    result.tracking = line.find("...").map(|at| {
        line[at + 3..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect::<String>()
    });

    // /\son\s(\S+?)(?=\.{3}|$)/: "No commits yet on <rama>".
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for window in chars.windows(4) {
        let [(_, a), (_, b), (_, c), (_, d)] = [window[0], window[1], window[2], window[3]];
        if !(a.is_whitespace() && b == 'o' && c == 'n' && d.is_whitespace()) {
            continue;
        }
        let start = window[3].0 + d.len_utf8();
        let word: String = line[start..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        if word.is_empty() {
            continue;
        }
        let name = match word.find("...") {
            // Un nombre vacío antes de "..." no cumple \S+?.
            Some(0) => None,
            Some(at) => Some(word[..at].to_string()),
            None if start + word.len() == line.len() => Some(word),
            None => None,
        };
        if let Some(name) = name {
            result.current = Some(name);
            break;
        }
    }

    result.detached = line.contains("(no branch)");
}

fn apply_entry(result: &mut StatusSummary, index: char, working_dir: char, path: &str) {
    let file = path.to_string();
    match (index, working_dir) {
        (' ', 'A') => result.created.push(file),
        (' ', 'D') => result.deleted.push(file),
        (' ', 'M') => result.modified.push(file),
        ('A', ' ') => {
            result.created.push(file.clone());
            result.staged.push(file);
        }
        ('A', 'M') => {
            result.created.push(file.clone());
            result.staged.push(file.clone());
            result.modified.push(file);
        }
        ('D', ' ') => {
            result.deleted.push(file.clone());
            result.staged.push(file);
        }
        ('M', ' ') | ('M', 'M') => {
            result.modified.push(file.clone());
            result.staged.push(file);
        }
        ('R', ' ') => result.renamed.push(renamed_file(path)),
        ('R', 'M') => {
            let renamed = renamed_file(path);
            result.modified.push(renamed.to.clone());
            result.renamed.push(renamed);
        }
        ('!', '!') => result.ignored.get_or_insert_with(Vec::new).push(file),
        ('?', '?') => result.not_added.push(file),
        ('A', 'A')
        | ('A', 'U')
        | ('D', 'D')
        | ('D', 'U')
        | ('U', 'A')
        | ('U', 'D')
        | ('U', 'U') => result.conflicted.push(file),
        ('#', '#') => parse_branch_line(result, path),
        _ => {}
    }

    if !matches!((index, working_dir), ('#', '#') | ('!', '!')) {
        result.files.push(file_status(path, index, working_dir));
    }
}

fn split_line(result: &mut StatusSummary, line: &str) {
    let trimmed = line.trim();
    let mut chars = trimmed.chars();
    let (first, second, third) = (chars.next(), chars.next(), chars.next());
    // simple-git recorta la línea antes de mirar las columnas, así que " M f"
    // llega como "M f" y se reinterpreta como (índice vacío, working M).
    if third == Some(' ') {
        let (index, working_dir) = (first.unwrap_or(' '), second.unwrap_or(' '));
        let offset = index.len_utf8() + working_dir.len_utf8() + 1;
        apply_entry(result, index, working_dir, &trimmed[offset..]);
    } else if second == Some(' ') {
        let working_dir = first.unwrap_or(' ');
        let offset = working_dir.len_utf8() + 1;
        apply_entry(result, ' ', working_dir, &trimmed[offset..]);
    }
}

/// Parsea la salida de `git status --porcelain -b -u --null`.
pub fn parse_status_summary(text: &str) -> StatusSummary {
    let lines: Vec<&str> = text.split('\0').collect();
    let mut status = StatusSummary::default();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        index += 1;
        if line.is_empty() {
            continue;
        }
        if line.starts_with('R') {
            // Los renombres traen el origen en el siguiente campo NUL.
            let from = lines.get(index).copied().unwrap_or_default();
            index += 1;
            split_line(&mut status, &format!("{line}\0{from}"));
        } else {
            split_line(&mut status, line);
        }
    }
    status
}

/// Tope de bytes para `git status --porcelain -b -u --null` (16 MiB). Si un
/// repositorio tiene un directorio enorme sin ignorar (p. ej. `node_modules`),
/// se reintenta con `-uno` en vez de agotar la memoria.
pub const MAX_STATUS_BYTES: usize = 16 * 1024 * 1024;

impl Git {
    /// `git.status()` de simple-git, acotado para repositorios con miles de
    /// archivos sin seguimiento.
    pub fn status(&self) -> GitResult<StatusSummary> {
        let capped = self.exec_capped(
            ["status", "--porcelain", "-b", "-u", "--null"],
            MAX_STATUS_BYTES,
        )?;
        let output = if capped.exceeded {
            log::warn!(
                "git status -u exceeded {} bytes; falling back to -uno",
                MAX_STATUS_BYTES
            );
            self.exec_capped(
                ["status", "--porcelain", "-b", "-uno", "--null"],
                MAX_STATUS_BYTES,
            )?
            .output
        } else {
            capped.output
        };
        if output.is_task_error() {
            return Err(GitError::from_output(&output));
        }
        Ok(parse_status_summary(&output.stdout_text()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationType {
    #[serde(rename = "rebase")]
    Rebase,
    #[serde(rename = "merge")]
    Merge,
    #[serde(rename = "cherry-pick")]
    CherryPick,
    #[serde(rename = "revert")]
    Revert,
}

impl OperationType {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationType::Rebase => "rebase",
            OperationType::Merge => "merge",
            OperationType::CherryPick => "cherry-pick",
            OperationType::Revert => "revert",
        }
    }
}

/// Operación de varios pasos a medio hacer (rebase, merge, cherry-pick, revert).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationState {
    pub in_progress: bool,
    #[serde(rename = "type")]
    pub kind: Option<OperationType>,
    pub label: String,
    pub can_continue: bool,
    pub can_abort: bool,
    pub can_skip: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictState {
    pub in_conflict: bool,
    pub operation_type: Option<OperationType>,
    pub initial_count: usize,
    pub pending_count: usize,
    pub resolved_count: usize,
    pub pending_files: Vec<String>,
    pub ready_to_continue: bool,
}

/// Forma del estado que consume la interfaz (`getStructuredStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredStatus {
    /// Rama actual. Vacío en el estado "sin datos" (`buildEmptyStatus`).
    pub current: Option<String>,
    pub head_short_hash: Option<String>,
    pub tracking: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub modified: Vec<String>,
    pub created: Vec<String>,
    pub deleted: Vec<String>,
    pub renamed: Vec<RenamedFile>,
    pub staged: Vec<String>,
    pub files: Vec<FileStatus>,
    #[serde(rename = "not_added")]
    pub not_added: Vec<String>,
    pub conflicted: Vec<String>,
    pub is_clean: bool,
    pub operation_state: OperationState,
    pub conflict_state: ConflictState,
}

impl StructuredStatus {
    /// `buildEmptyStatus()`: estado neutro para cuando no se pudo leer el repo.
    pub fn empty() -> Self {
        Self {
            current: Some(String::new()),
            is_clean: true,
            ..Default::default()
        }
    }
}

/// `isMissingHeadError`: el repo aún no tiene commits o la revisión no existe.
pub fn is_missing_head_error(error: &GitError) -> bool {
    let message = error.message.to_lowercase();
    message.contains("unknown revision")
        || message.contains("ambiguous argument")
        || message.contains("bad revision")
        || message.contains("does not have any commits")
        || message.contains("needed a single revision")
}

/// Hash corto de HEAD, o `None` si el repo no tiene commits.
pub fn head_short_hash(repo_path: &Path) -> GitResult<Option<String>> {
    match Git::new(repo_path).revparse(["--short", "HEAD"]) {
        Ok(hash) if hash.is_empty() => Ok(None),
        Ok(hash) => Ok(Some(hash)),
        Err(error) if is_missing_head_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn git_dir_cache() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ruta absoluta del gitdir. Se cachea: en un repo vivo no cambia y ahorra un
/// proceso `git rev-parse --git-dir` en cada lectura de estado. Riesgo asumido:
/// un repo recreado en el mismo path cambiando dir<->file (worktrees) requiere
/// reabrirlo para invalidar la entrada.
pub fn git_dir_path(repo_path: &Path) -> GitResult<PathBuf> {
    if let Some(cached) = git_dir_cache().lock().unwrap().get(repo_path) {
        return Ok(cached.clone());
    }

    let git_dir = Git::new(repo_path).revparse(["--git-dir"])?;
    let candidate = PathBuf::from(&git_dir);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        repo_path.join(candidate)
    };

    remember_git_dir(
        &mut git_dir_cache().lock().unwrap(),
        repo_path.to_path_buf(),
        resolved.clone(),
    );
    Ok(resolved)
}

/// Tope de gitdirs guardados: la caché vive lo que el proceso y cada
/// repositorio que se abre (o se inspecciona desde el inicio) deja el suyo.
/// Volver a preguntarlo cuesta un `rev-parse`, así que al llenarse se vacía.
const MAX_GIT_DIRS: usize = 256;

fn remember_git_dir(cache: &mut HashMap<PathBuf, PathBuf>, repo_path: PathBuf, git_dir: PathBuf) {
    if cache.len() >= MAX_GIT_DIRS && !cache.contains_key(&repo_path) {
        cache.clear();
    }
    cache.insert(repo_path, git_dir);
}

/// Olvida el gitdir cacheado de `repo_path` (tests y repos recreados).
pub fn forget_git_dir_path(repo_path: &Path) {
    git_dir_cache().lock().unwrap().remove(repo_path);
}

/// `detectGitOperationState`: mira los marcadores del gitdir.
pub fn detect_operation_state(repo_path: &Path) -> GitResult<OperationState> {
    let git_dir = git_dir_path(repo_path)?;
    let has_rebase_merge = git_dir.join("rebase-merge").exists();
    let has_rebase_apply = git_dir.join("rebase-apply").exists();

    if has_rebase_merge || has_rebase_apply {
        // Con rebase-merge/ presente, msgnum/end dicen en qué paso va ("3/7").
        let mut label = "Rebase in progress".to_string();
        if has_rebase_merge {
            let read_number = |name: &str| {
                std::fs::read_to_string(git_dir.join("rebase-merge").join(name))
                    .ok()
                    .and_then(|text| text.trim().parse::<u64>().ok())
            };
            // Sin msgnum/end (rebase recién iniciado o formato distinto): label base.
            if let (Some(step), Some(total)) = (read_number("msgnum"), read_number("end"))
                && total > 0
            {
                label = format!("Rebase in progress ({step}/{total})");
            }
        }
        return Ok(OperationState {
            in_progress: true,
            kind: Some(OperationType::Rebase),
            label,
            can_continue: true,
            can_abort: true,
            can_skip: true,
        });
    }

    let single_step = |kind: OperationType, label: &str| OperationState {
        in_progress: true,
        kind: Some(kind),
        label: label.to_string(),
        can_continue: true,
        can_abort: true,
        can_skip: false,
    };

    if git_dir.join("MERGE_HEAD").exists() {
        return Ok(single_step(OperationType::Merge, "Merge in progress"));
    }
    // Un cherry-pick o revert de varios commits deja la secuencia en
    // `sequencer/`: el commit que chocó se puede saltar y seguir con el resto.
    let sequence = |kind: OperationType, label: &str| OperationState {
        can_skip: git_dir.join("sequencer").is_dir(),
        ..single_step(kind, label)
    };
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Ok(sequence(
            OperationType::CherryPick,
            "Cherry-pick in progress",
        ));
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return Ok(sequence(OperationType::Revert, "Revert in progress"));
    }

    Ok(OperationState::default())
}

/// `buildRawConflictState`.
pub fn build_raw_conflict_state(
    operation_state: &OperationState,
    conflicted_files: &[String],
) -> ConflictState {
    let pending_count = conflicted_files.len();
    let ready_to_continue =
        operation_state.in_progress && operation_state.kind.is_some() && pending_count == 0;

    ConflictState {
        in_conflict: pending_count > 0 || ready_to_continue,
        operation_type: operation_state.kind,
        initial_count: pending_count,
        pending_count,
        resolved_count: 0,
        pending_files: conflicted_files.to_vec(),
        ready_to_continue,
    }
}

/// Archivos en conflicto, o lista vacía si no se pudo leer el estado.
pub fn conflict_files(repo_path: &Path) -> Vec<String> {
    Git::new(repo_path)
        .status()
        .map(|status| status.conflicted)
        .unwrap_or_default()
}

/// `getStructuredStatus`.
pub fn get_structured_status(repo_path: &Path) -> GitResult<StructuredStatus> {
    let git = Git::new(repo_path);
    // Se esperan ambas lecturas aunque una falle: evita dejar un proceso de git
    // pendiente, sobre todo al validar rutas que no son repositorios en Windows.
    let (status, operation_state) = std::thread::scope(|scope| {
        let status = scope.spawn(|| git.status());
        let operation_state = detect_operation_state(repo_path);
        (status.join().expect("status thread"), operation_state)
    });
    let status = status?;
    let operation_state = operation_state?;
    // Se resuelve siempre, no solo con HEAD desanclado: la barra superior muestra
    // el commit actual en cualquier rama. Es un `rev-parse`, no recorre el árbol.
    let head_short_hash = head_short_hash(repo_path)?;

    let conflict_state = build_raw_conflict_state(&operation_state, &status.conflicted);
    let is_clean = status.is_clean();

    Ok(StructuredStatus {
        current: status.current,
        head_short_hash,
        tracking: status.tracking,
        ahead: status.ahead,
        behind: status.behind,
        modified: status.modified,
        created: status.created,
        deleted: status.deleted,
        renamed: status.renamed,
        staged: status.staged,
        files: status.files,
        not_added: status.not_added,
        conflicted: status.conflicted,
        is_clean,
        operation_state,
        conflict_state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_git_dir_cache_is_bounded() {
        let mut cache = HashMap::new();
        for index in 0..MAX_GIT_DIRS {
            remember_git_dir(
                &mut cache,
                PathBuf::from(format!("/repo-{index}")),
                PathBuf::from(format!("/repo-{index}/.git")),
            );
        }
        assert_eq!(cache.len(), MAX_GIT_DIRS);

        // Volver a guardar uno que ya está no vacía nada.
        remember_git_dir(
            &mut cache,
            PathBuf::from("/repo-0"),
            PathBuf::from("/repo-0/.git"),
        );
        assert_eq!(cache.len(), MAX_GIT_DIRS);

        // Uno nuevo con la caché llena empieza de cero.
        remember_git_dir(
            &mut cache,
            PathBuf::from("/another"),
            PathBuf::from("/another/.git"),
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.get(Path::new("/another")),
            Some(&PathBuf::from("/another/.git"))
        );
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        Git::new(dir)
            .with_config([
                "user.name=Test",
                "user.email=test@test.local",
                "commit.gpgsign=false",
            ])
            .raw(args)
            .unwrap()
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        dir
    }

    fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
        std::fs::write(dir.join(name), content).unwrap();
        git(dir, &["add", "--all"]);
        git(dir, &["commit", "-m", message]);
    }

    #[test]
    fn parses_the_branch_line() {
        let status = parse_status_summary("## main...origin/main [ahead 2, behind 3]\0");
        assert_eq!(status.current.as_deref(), Some("main"));
        assert_eq!(status.tracking.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 3));
        assert!(!status.detached);
        assert!(status.is_clean());

        let status = parse_status_summary("## No commits yet on feature/login\0");
        assert_eq!(status.current.as_deref(), Some("feature/login"));
        assert_eq!(status.tracking, None);

        let status = parse_status_summary("## HEAD (no branch)\0");
        assert_eq!(status.current.as_deref(), Some("HEAD"));
        assert!(status.detached);

        let status = parse_status_summary("## No commits yet on main...origin/main [gone]\0");
        assert_eq!(status.current.as_deref(), Some("main"));
        assert_eq!(status.tracking.as_deref(), Some("origin/main"));
    }

    #[test]
    fn parses_every_entry_kind() {
        let text = [
            "## main",
            " M worktree.txt",
            "M  staged.txt",
            "MM both.txt",
            "A  added.txt",
            "AM added-then-edited.txt",
            " D removed.txt",
            "D  staged-removal.txt",
            "R  new-name.txt",
            "old-name.txt",
            "?? untracked.txt",
            "UU conflict.txt",
            "",
        ]
        .join("\0");
        let status = parse_status_summary(&text);

        assert_eq!(
            status.modified,
            [
                "worktree.txt",
                "staged.txt",
                "both.txt",
                "added-then-edited.txt"
            ]
        );
        assert_eq!(
            status.staged,
            [
                "staged.txt",
                "both.txt",
                "added.txt",
                "added-then-edited.txt",
                "staged-removal.txt"
            ]
        );
        assert_eq!(status.created, ["added.txt", "added-then-edited.txt"]);
        assert_eq!(status.deleted, ["removed.txt", "staged-removal.txt"]);
        assert_eq!(
            status.renamed,
            [RenamedFile {
                from: "old-name.txt".into(),
                to: "new-name.txt".into()
            }]
        );
        assert_eq!(status.not_added, ["untracked.txt"]);
        assert_eq!(status.conflicted, ["conflict.txt"]);
        assert_eq!(status.files.len(), 10);

        let worktree = &status.files[0];
        assert_eq!(
            (worktree.index.as_str(), worktree.working_dir.as_str()),
            (" ", "M")
        );
        let renamed = status.files.iter().find(|file| file.index == "R").unwrap();
        assert_eq!(renamed.path, "new-name.txt");
        assert_eq!(renamed.from.as_deref(), Some("old-name.txt"));
        assert!(!status.is_clean());
    }

    #[test]
    fn structured_status_of_a_real_repository() {
        let dir = init_repo();
        let path = dir.path();

        let empty = get_structured_status(path).unwrap();
        assert_eq!(empty.current.as_deref(), Some("main"));
        assert_eq!(empty.head_short_hash, None);
        assert!(empty.is_clean);

        commit_file(path, "a.txt", "one\n", "first");
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        std::fs::write(path.join("b.txt"), "new\n").unwrap();
        git(path, &["add", "b.txt"]);
        std::fs::write(path.join("c with space.txt"), "x\n").unwrap();

        let status = get_structured_status(path).unwrap();
        assert!(status.head_short_hash.is_some());
        assert_eq!(status.modified, ["a.txt"]);
        assert_eq!(status.created, ["b.txt"]);
        assert_eq!(status.staged, ["b.txt"]);
        assert_eq!(status.not_added, ["c with space.txt"]);
        assert!(!status.is_clean);
        assert!(!status.operation_state.in_progress);
        assert!(!status.conflict_state.in_conflict);
    }

    #[test]
    fn detects_a_merge_conflict() {
        let dir = init_repo();
        let path = dir.path();
        commit_file(path, "a.txt", "base\n", "base");
        git(path, &["checkout", "-b", "feature"]);
        commit_file(path, "a.txt", "feature\n", "feature");
        git(path, &["checkout", "main"]);
        commit_file(path, "a.txt", "main\n", "main");

        let merge = Git::new(path)
            .with_config(["user.name=Test", "user.email=test@test.local"])
            .exec(["merge", "feature"])
            .unwrap();
        assert!(!merge.success());

        let status = get_structured_status(path).unwrap();
        assert_eq!(status.conflicted, ["a.txt"]);
        assert_eq!(status.operation_state.kind, Some(OperationType::Merge));
        assert_eq!(status.operation_state.label, "Merge in progress");
        assert!(status.operation_state.can_abort && !status.operation_state.can_skip);
        assert!(status.conflict_state.in_conflict);
        assert_eq!(status.conflict_state.pending_files, ["a.txt"]);
        assert!(!status.conflict_state.ready_to_continue);

        git(path, &["add", "a.txt"]);
        let resolved = get_structured_status(path).unwrap();
        assert!(resolved.conflict_state.in_conflict);
        assert!(resolved.conflict_state.ready_to_continue);
        assert_eq!(resolved.conflict_state.pending_count, 0);
    }

    #[test]
    fn serializes_with_the_same_field_names_as_the_electron_app() {
        let json = serde_json::to_value(StructuredStatus::empty()).unwrap();
        assert_eq!(json["current"], "");
        assert_eq!(json["headShortHash"], serde_json::Value::Null);
        assert_eq!(json["isClean"], true);
        assert!(json["not_added"].is_array());
        assert_eq!(json["operationState"]["inProgress"], false);
        assert_eq!(json["operationState"]["type"], serde_json::Value::Null);
        assert_eq!(json["conflictState"]["readyToContinue"], false);
    }

    #[test]
    fn a_path_that_is_not_a_repository_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert!(get_structured_status(dir.path()).is_err());
    }
}
