//! Comparación de ramas y predicción de conflictos de merge.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

use mergeon_gitcmd::GitError;

use crate::diff::{DiffOptions, DiffPayload, git_diff_payload};
use crate::error::{Cause, Inner, QueryResult, join, read};
use crate::exec;
use crate::paths::normalize_repo_relative_path;
use crate::simple_log::{LogEntry, parse_log_entries, pretty_argument};
use crate::text::{js_trim, parse_count, utf16_prefix};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparedCommit {
    /// Primeros 7 caracteres del hash.
    pub hash: String,
    /// Hash completo (para llevar el commit al historial).
    #[serde(rename = "fullHash", default)]
    pub full_hash: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// Commits que lista la comparación por cada lado. Los totales (`ahead`,
/// `behind`) se cuentan aparte, así que el diálogo puede decir cuántos faltan.
pub const COMPARE_COMMIT_LIMIT: usize = 200;

/// Resultado de `git:compareBranches`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchComparison {
    /// Commits de la segunda rama que no están en la primera (total).
    pub ahead: usize,
    /// Commits de la primera rama que no están en la segunda (total).
    pub behind: usize,
    /// Los [`COMPARE_COMMIT_LIMIT`] más recientes de `ahead`.
    pub ahead_commits: Vec<ComparedCommit>,
    /// Los [`COMPARE_COMMIT_LIMIT`] más recientes de `behind`.
    pub behind_commits: Vec<ComparedCommit>,
    pub files_changed: usize,
    pub insertions: u64,
    pub deletions: u64,
}

impl BranchComparison {
    /// Commits de `ahead` que no vienen en `ahead_commits`.
    pub fn ahead_omitted(&self) -> usize {
        self.ahead.saturating_sub(self.ahead_commits.len())
    }

    /// Commits de `behind` que no vienen en `behind_commits`.
    pub fn behind_omitted(&self) -> usize {
        self.behind.saturating_sub(self.behind_commits.len())
    }
}

fn compared_commits(entries: Vec<LogEntry>) -> Vec<ComparedCommit> {
    entries
        .into_iter()
        .map(|entry| ComparedCommit {
            hash: utf16_prefix(&entry.hash, 7).to_string(),
            full_hash: entry.hash.clone(),
            message: entry.message,
            author: entry.author_name,
            date: entry.date,
        })
        .collect()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DiffStat {
    files: usize,
    insertions: u64,
    deletions: u64,
}

static STAT_SUMMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]+) files? changed\s*((?:, [0-9]+ [^,]+){0,2})").unwrap());
static STAT_INSERTIONS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([0-9]+) i").unwrap());
static STAT_DELETIONS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([0-9]+) d").unwrap());

/// La línea de `git diff --shortstat`
/// (` 3 files changed, 6 insertions(+), 6 deletions(-)`); vacía si no hay cambios.
fn parse_shortstat(stdout: &str) -> DiffStat {
    let Some(captures) = STAT_SUMMARY.captures(stdout) else {
        return DiffStat::default();
    };
    let summary = &captures[2];
    let total = |pattern: &Regex| {
        pattern
            .captures(summary)
            .map_or(0, |found| parse_count(&found[1]))
    };
    DiffStat {
        files: parse_count(&captures[1]) as usize,
        insertions: total(&STAT_INSERTIONS),
        deletions: total(&STAT_DELETIONS),
    }
}

/// `rev-list --left-right --count a...b`: `<solo en a>\t<solo en b>`.
fn parse_left_right_count(stdout: &str) -> (usize, usize) {
    let mut counts = js_trim(stdout)
        .split('\t')
        .map(|count| parse_count(count) as usize);
    (
        counts.next().unwrap_or_default(),
        counts.next().unwrap_or_default(),
    )
}

/*
 * Canal `git:compareBranches`.
 *
 * Antes eran dos `git log a..b` sin límite y un `diff --stat` con una fila por
 * archivo: entre ramas muy separadas eran miles de commits (con su cuerpo) y
 * de filas para acabar mostrando un número. Ahora los totales salen de
 * `rev-list --count` y `diff --shortstat`, los logs se cortan en
 * [`COMPARE_COMMIT_LIMIT`] y las cuatro lecturas van en paralelo.
 */
pub fn compare_branches(
    repo_path: &Path,
    branch1: &str,
    branch2: &str,
) -> QueryResult<BranchComparison> {
    read("Failed to compare branches", || {
        let git = exec::open(repo_path)?;
        let pretty = pretty_argument();
        let limit = format!("--max-count={COMPARE_COMMIT_LIMIT}");
        let log = |range: String| {
            exec::raw(&git, &["log", &limit, &pretty, &range]).map(|out| parse_log_entries(&out))
        };

        let (counts, ahead, behind, stat) = std::thread::scope(|scope| {
            let counts = scope.spawn(|| {
                exec::raw(
                    &git,
                    &[
                        "rev-list",
                        "--left-right",
                        "--count",
                        &format!("{branch1}...{branch2}"),
                    ],
                )
            });
            // Commits de branch2 que no están en branch1, y al revés.
            let ahead = scope.spawn(|| log(format!("{branch1}..{branch2}")));
            let behind = scope.spawn(|| log(format!("{branch2}..{branch1}")));
            let stat = exec::raw(&git, &["diff", "--shortstat", branch1, branch2]);
            (
                join(counts),
                join(ahead),
                join(behind),
                stat.map_err(Cause::from),
            )
        });
        let (behind_total, ahead_total) = parse_left_right_count(&counts?);
        let (ahead, behind, stat) = (ahead?, behind?, parse_shortstat(&stat?));

        Ok(BranchComparison {
            ahead: ahead_total.max(ahead.len()),
            behind: behind_total.max(behind.len()),
            ahead_commits: compared_commits(ahead),
            behind_commits: compared_commits(behind),
            files_changed: stat.files,
            insertions: stat.insertions,
            deletions: stat.deletions,
        })
    })
}

// ---------------------------------------------------------------------------
// Comparación navegable de dos revisiones
// ---------------------------------------------------------------------------

/// Archivos que lista una comparación de revisiones. Los totales (`total`,
/// `insertions`, `deletions`) cuentan todos.
pub const COMPARE_FILE_LIMIT: usize = 1000;

/// Cómo se mide el diff entre las dos revisiones de una comparación.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareMode {
    /// `A...B`: lo que cambió en B desde que se separó de A (diff desde la
    /// base común). Es lo que se revisa antes de un pull request.
    #[default]
    SinceDivergence,
    /// `A B`: diferencia directa entre los dos árboles.
    Direct,
}

impl CompareMode {
    /// Cómo lo escribiría git: `A...B` o `A B`.
    pub fn notation(self, base: &str, head: &str) -> String {
        match self {
            Self::SinceDivergence => format!("{base}...{head}"),
            Self::Direct => format!("{base} {head}"),
        }
    }
}

/// Topes de una comparación de revisiones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompareLimits {
    /// Commits que se listan por lado.
    pub commits: usize,
    /// Archivos que se listan.
    pub files: usize,
}

impl Default for CompareLimits {
    fn default() -> Self {
        Self {
            commits: COMPARE_COMMIT_LIMIT,
            files: COMPARE_FILE_LIMIT,
        }
    }
}

/// Un archivo de la comparación.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparedFile {
    /// En renombres y copias, la ruta de destino.
    pub path: String,
    /// En renombres y copias, la ruta de origen.
    pub old_path: Option<String>,
    /// Primera letra del estado de `--name-status` (`A`, `M`, `D`, `R`, `C`...).
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
}

/// Las dos revisiones de una comparación fijadas a su SHA (si una rama se
/// mueve después, la comparación no cambia), y su base común.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonRevisions {
    /// SHA completo de A.
    pub base: String,
    /// SHA completo de B.
    pub head: String,
    /// `git merge-base A B`; `None` si no tienen historia común.
    pub merge_base: Option<String>,
}

impl ComparisonRevisions {
    /// Commit desde el que se mide el diff en `mode`: la base común o A.
    /// `None` en [`CompareMode::SinceDivergence`] sin historia común.
    pub fn diff_from(&self, mode: CompareMode) -> Option<&str> {
        match mode {
            CompareMode::SinceDivergence => self.merge_base.as_deref(),
            CompareMode::Direct => Some(&self.base),
        }
    }
}

/// Los archivos de una comparación (como mucho `limit`) y sus totales.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonFiles {
    pub files: Vec<ComparedFile>,
    /// Archivos cambiados en total.
    pub total: usize,
    pub insertions: u64,
    pub deletions: u64,
}

impl ComparisonFiles {
    /// Archivos que no vienen en `files` (el "N more" de la lista).
    pub fn omitted(&self) -> usize {
        self.total.saturating_sub(self.files.len())
    }
}

/// Resultado de [`compare_revisions`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionComparison {
    pub revisions: ComparisonRevisions,
    pub mode: CompareMode,
    /// Commits de B que no están en A (total).
    pub ahead: usize,
    /// Commits de A que no están en B (total).
    pub behind: usize,
    pub ahead_commits: Vec<ComparedCommit>,
    pub behind_commits: Vec<ComparedCommit>,
    /// Vacío en [`CompareMode::SinceDivergence`] sin historia común.
    pub files: ComparisonFiles,
}

impl RevisionComparison {
    pub fn has_common_history(&self) -> bool {
        self.revisions.merge_base.is_some()
    }

    pub fn ahead_omitted(&self) -> usize {
        self.ahead.saturating_sub(self.ahead_commits.len())
    }

    pub fn behind_omitted(&self) -> usize {
        self.behind.saturating_sub(self.behind_commits.len())
    }
}

/// El error de pedir el diff "desde la base común" de dos revisiones sin
/// historia común.
pub const NO_COMMON_HISTORY: &str = "These revisions have no common history, so there is no point where they diverged. Use the direct diff to compare them.";

/// SHA completo del commit al que apunta `revision`, o un error claro.
fn resolve_commit(git: &mergeon_gitcmd::Git, revision: &str) -> Inner<String> {
    let revision = js_trim(revision);
    if revision.is_empty() {
        return Err("A revision to compare is missing.".into());
    }
    let output = exec::exec(
        git,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )?;
    let sha = js_trim(&output.stdout_text()).to_string();
    if output.exit_code == Some(0) && !sha.is_empty() {
        return Ok(sha);
    }
    if output.exit_code == Some(1) || output.stderr.is_empty() {
        return Err(format!("Unknown branch or revision '{revision}'.").into());
    }
    Err(GitError::from_output(&output).into())
}

/// `git merge-base A B`: código 1 sin salida es "sin historia común".
fn merge_base(git: &mergeon_gitcmd::Git, base: &str, head: &str) -> Inner<Option<String>> {
    let output = exec::exec(git, &["merge-base", base, head])?;
    let sha = js_trim(&output.stdout_text()).to_string();
    match output.exit_code {
        Some(0) if !sha.is_empty() => Ok(Some(sha)),
        Some(1) if sha.is_empty() => Ok(None),
        _ => Err(GitError::from_output(&output).into()),
    }
}

fn resolve(
    git: &mergeon_gitcmd::Git,
    base_revision: &str,
    head_revision: &str,
) -> Inner<ComparisonRevisions> {
    let (base, head) = std::thread::scope(|scope| {
        let base = scope.spawn(|| resolve_commit(git, base_revision));
        let head = resolve_commit(git, head_revision);
        (join(base), head)
    });
    let (base, head) = (base?, head?);
    let merge_base = merge_base(git, &base, &head)?;
    Ok(ComparisonRevisions {
        base,
        head,
        merge_base,
    })
}

/// Fija dos revisiones (ramas, tags, SHAs, `HEAD~2`...) a su SHA y busca su
/// base común.
pub fn resolve_comparison(
    repo_path: &Path,
    base: &str,
    head: &str,
) -> QueryResult<ComparisonRevisions> {
    read("Failed to compare revisions", || {
        let git = exec::open(repo_path)?;
        resolve(&git, base, head)
    })
}

/*
 * `--name-status -z` y `--numstat -z` de la comparación. Como en el detalle de
 * un commit (ver `diff::parse_name_status_with_stats`), pero se conserva la
 * ruta de origen de renombres y copias (el diff de un renombre necesita las
 * dos) y se suman los totales de todos los archivos, no solo de los listados.
 */
fn parse_compared_files(status_output: &str, stats_output: &str, limit: usize) -> ComparisonFiles {
    let mut stats: std::collections::HashMap<&str, (u64, u64)> = Default::default();
    let (mut insertions, mut deletions) = (0, 0);
    let mut fields = stats_output.split('\0');
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let _source = fields.next();
            fields.next().unwrap_or_default()
        } else {
            path
        };
        // Los binarios traen `-`: cuentan como 0.
        let counts = (parse_count(js_trim(added)), parse_count(js_trim(deleted)));
        insertions += counts.0;
        deletions += counts.1;
        stats.insert(path, counts);
    }

    let mut files = Vec::new();
    let mut total = 0;
    let mut fields = status_output.split('\0');
    while let Some(field) = fields.next() {
        let code = js_trim(field);
        if code.is_empty() {
            continue;
        }
        let status: String = code.chars().take(1).collect();
        let (old_path, path) = if status == "R" || status == "C" {
            (fields.next(), fields.next())
        } else {
            (None, fields.next())
        };
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            continue;
        };
        total += 1;
        if files.len() >= limit {
            continue;
        }
        let (additions, deletions) = stats.get(path).copied().unwrap_or_default();
        files.push(ComparedFile {
            path: path.to_string(),
            old_path: old_path.filter(|old| !old.is_empty()).map(str::to_string),
            status,
            additions,
            deletions,
        });
    }

    ComparisonFiles {
        files,
        total,
        insertions,
        deletions,
    }
}

fn comparison_files(
    git: &mergeon_gitcmd::Git,
    from: &str,
    to: &str,
    limit: usize,
) -> Inner<ComparisonFiles> {
    let (status, stats) = std::thread::scope(|scope| {
        let status = scope.spawn(|| {
            exec::raw(
                git,
                &["diff", "--name-status", "-z", "--find-renames", from, to],
            )
        });
        let stats = exec::raw(
            git,
            &["diff", "--numstat", "-z", "--find-renames", from, to],
        );
        (join(status), stats.map_err(Cause::from))
    });
    Ok(parse_compared_files(&status?, &stats?, limit))
}

/// Solo los archivos de una comparación ya fijada (al cambiar de modo).
pub fn get_comparison_files(
    repo_path: &Path,
    revisions: &ComparisonRevisions,
    mode: CompareMode,
    limit: usize,
) -> QueryResult<ComparisonFiles> {
    read("Failed to compare revisions", || {
        let git = exec::open(repo_path)?;
        match revisions.diff_from(mode) {
            Some(from) => comparison_files(&git, from, &revisions.head, limit),
            None => Ok(ComparisonFiles::default()),
        }
    })
}

/*
 * Comparación navegable entre dos revisiones: las fija a su SHA, cuenta y
 * lista (con tope) los commits de cada lado y los archivos cambiados según
 * `mode`. El modo "desde que se separaron" usa la base común explícita
 * (`merge-base A B`) en vez de `A...B`: es lo mismo con una sola base, y con
 * varias (merges cruzados) el resultado no depende de cuál elija `diff`.
 * Sin historia común, ese modo no tiene archivos (`has_common_history()`
 * es falso) y el modo directo sigue funcionando.
 */
pub fn compare_revisions(
    repo_path: &Path,
    base: &str,
    head: &str,
    mode: CompareMode,
    limits: CompareLimits,
) -> QueryResult<RevisionComparison> {
    read("Failed to compare revisions", || {
        let git = exec::open(repo_path)?;
        let revisions = resolve(&git, base, head)?;
        let (base, head) = (revisions.base.as_str(), revisions.head.as_str());
        let pretty = pretty_argument();
        let limit = format!("--max-count={}", limits.commits);
        let log = |range: String| {
            exec::raw(&git, &["log", &limit, &pretty, &range]).map(|out| parse_log_entries(&out))
        };
        let from = revisions.diff_from(mode);

        let (counts, ahead, behind, files) = std::thread::scope(|scope| {
            let counts = scope.spawn(|| {
                exec::raw(
                    &git,
                    &[
                        "rev-list",
                        "--left-right",
                        "--count",
                        &format!("{base}...{head}"),
                    ],
                )
            });
            let ahead = scope.spawn(|| log(format!("{base}..{head}")));
            let behind = scope.spawn(|| log(format!("{head}..{base}")));
            let files = match from {
                Some(from) => comparison_files(&git, from, head, limits.files),
                None => Ok(ComparisonFiles::default()),
            };
            (join(counts), join(ahead), join(behind), files)
        });
        let (behind_total, ahead_total) = parse_left_right_count(&counts?);
        let (ahead, behind, files) = (ahead?, behind?, files?);

        Ok(RevisionComparison {
            mode,
            ahead: ahead_total.max(ahead.len()),
            behind: behind_total.max(behind.len()),
            ahead_commits: compared_commits(ahead),
            behind_commits: compared_commits(behind),
            files,
            revisions,
        })
    })
}

/// Parche de un archivo de la comparación. Un renombre se pide con sus dos
/// rutas para que git lo reconozca como tal.
pub fn get_comparison_diff(
    repo_path: &Path,
    revisions: &ComparisonRevisions,
    mode: CompareMode,
    file_path: &str,
    old_path: Option<&str>,
    options: &DiffOptions,
) -> QueryResult<DiffPayload> {
    read("Failed to get comparison diff", || {
        let git = exec::open(repo_path)?;
        let Some(from) = revisions.diff_from(mode) else {
            return Err(NO_COMMON_HISTORY.into());
        };
        let path = normalize_repo_relative_path(file_path).map_err(|error| error.to_string())?;
        let old_path = old_path
            .map(normalize_repo_relative_path)
            .transpose()
            .map_err(|error| error.to_string())?
            .filter(|old| *old != path);

        let mut args = vec!["diff", "--find-renames"];
        options.push_args(&mut args);
        args.extend([from, revisions.head.as_str(), "--"]);
        if let Some(old_path) = old_path.as_deref() {
            args.push(old_path);
        }
        args.push(&path);
        git_diff_payload(&git, &args)
    })
}

/// Resultado de `git:checkMergeConflicts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflictCheck {
    pub has_conflicts: bool,
    pub conflicted_files: Vec<String>,
}

/// Un OID de árbol (SHA-1 o SHA-256): la primera línea de `merge-tree --write-tree`.
fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/*
 * Con `-z`, la salida de `merge-tree --write-tree --name-only` es el OID del
 * árbol, las rutas en conflicto (una por campo, sin comillas aunque no sean
 * ASCII) y un campo vacío que la separa de los mensajes informativos
 * ("Auto-merging…", "CONFLICT…"), que no son rutas.
 */
fn conflicted_files_from(stdout: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for path in stdout
        .split('\0')
        .skip(1)
        .take_while(|field| !field.is_empty())
    {
        if !files.iter().any(|file| file == path) {
            files.push(path.to_string());
        }
    }
    files.truncate(100);
    files
}

/// Canal `git:checkMergeConflicts`: predice si mezclar `source` en `target`
/// daría conflictos, sin tocar el working tree.
///
/// La app Electron decidía con el criterio de `git.raw` (error solo si hay
/// stderr), pero `git merge-tree` anuncia los conflictos por stdout con código
/// 1 y stderr vacío, así que nunca detectaba ninguno. Aquí se mira el código de
/// salida: 0 es un merge limpio y 1 con un árbol en stdout son conflictos.
/// Cualquier otra cosa (un ref que no existe también sale con 1, pero sin
/// árbol) es un error de lectura.
pub fn check_merge_conflicts(
    repo_path: &Path,
    source: &str,
    target: &str,
) -> QueryResult<MergeConflictCheck> {
    read("Failed to check merge conflicts", || {
        let git = exec::open(repo_path)?;
        let output = exec::exec(
            &git,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "-z",
                target,
                source,
            ],
        )?;
        let stdout = output.stdout_text();

        if output.exit_code == Some(0) {
            return Ok(MergeConflictCheck::default());
        }
        let has_tree = stdout.split('\0').next().is_some_and(is_object_id);
        if output.exit_code == Some(1) && has_tree {
            return Ok(MergeConflictCheck {
                has_conflicts: true,
                conflicted_files: conflicted_files_from(&stdout),
            });
        }
        Err(GitError::from_output(&output).into())
    })
}

/// Totales de un rango de commits (`git diff --shortstat <desde> <hasta>`):
/// el resumen de la selección múltiple del historial.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeStats {
    pub files: usize,
    pub insertions: u64,
    pub deletions: u64,
}

/// Cuánto cambia de `from` a `to` (archivos, líneas añadidas y quitadas). Una
/// sola lectura acotada: git no lista los archivos, solo cuenta.
pub fn get_range_stats(repo_path: &Path, from: &str, to: &str) -> QueryResult<RangeStats> {
    read("Failed to count the changes of the range", || {
        let git = exec::open(repo_path)?;
        let stat = exec::raw(
            &git,
            &["diff", "--shortstat", "--end-of-options", from, to, "--"],
        )?;
        let stat = parse_shortstat(&stat);
        Ok(RangeStats {
            files: stat.files,
            insertions: stat.insertions,
            deletions: stat.deletions,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;
    use mergeon_gitcmd::Git;

    fn diverged_repo() -> TestRepo {
        let repo = TestRepo::new();
        repo.commit_file("shared.txt", "base\n", "base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.commit_file("feature.txt", "one\ntwo\n", "feature one");
        repo.commit_file("feature.txt", "one\nthree\n", "feature two");
        repo.git(&["checkout", "main"]);
        repo.commit_file("main.txt", "main\n", "main one");
        repo
    }

    #[test]
    fn compares_two_diverged_branches() {
        let repo = diverged_repo();
        let feature_tip = repo.git(&["rev-parse", "feature"]).trim().to_string();

        let comparison = compare_branches(&repo.path, "main", "feature").unwrap();

        assert_eq!(comparison.ahead, 2);
        assert_eq!(comparison.behind, 1);
        let ahead: Vec<_> = comparison
            .ahead_commits
            .iter()
            .map(|commit| commit.message.as_str())
            .collect();
        assert_eq!(ahead, ["feature two", "feature one"]);
        assert_eq!(comparison.ahead_commits[0].hash, feature_tip[..7]);
        assert_eq!(comparison.ahead_commits[0].author, "Test User");
        assert!(comparison.ahead_commits[0].date.contains('T'));
        assert_eq!(comparison.behind_commits[0].message, "main one");
        // main.txt desaparece y feature.txt aparece al ir de main a feature.
        assert_eq!(comparison.files_changed, 2);
        assert_eq!(comparison.insertions, 2);
        assert_eq!(comparison.deletions, 1);

        let json = serde_json::to_value(&comparison).unwrap();
        assert_eq!(json["aheadCommits"][0]["message"], "feature two");
        assert_eq!(json["filesChanged"], 2);
    }

    #[test]
    fn identical_branches_have_nothing_to_compare() {
        let repo = diverged_repo();

        assert_eq!(
            compare_branches(&repo.path, "main", "main").unwrap(),
            BranchComparison::default()
        );
    }

    #[test]
    fn comparing_an_unknown_branch_fails_with_the_read_prefix() {
        let repo = diverged_repo();

        let error = compare_branches(&repo.path, "main", "nope").unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to compare branches: fatal:"),
            "{error}"
        );
    }

    #[test]
    fn counts_the_changes_of_a_range_of_commits() {
        let repo = TestRepo::new();
        let from = repo.commit_file("a.txt", "one\ntwo\n", "a");
        repo.commit_file("a.txt", "one\n2\nthree\n", "b");
        repo.commit_file("c.txt", "c\n", "c");

        let stats = get_range_stats(&repo.path, &from, "HEAD").unwrap();

        assert_eq!(
            stats,
            RangeStats {
                files: 2,
                insertions: 3,
                deletions: 1
            }
        );
        assert!(get_range_stats(&repo.path, "no-such-ref", "HEAD").is_err());
    }

    #[test]
    fn parses_the_shortstat_line_and_the_left_right_counts() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 6 insertions(+), 6 deletions(-)\n"),
            DiffStat {
                files: 3,
                insertions: 6,
                deletions: 6
            }
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 1 insertion(+)\n"),
            DiffStat {
                files: 1,
                insertions: 1,
                deletions: 0
            }
        );
        assert_eq!(
            parse_shortstat(" 2 files changed, 4 deletions(-)\n"),
            DiffStat {
                files: 2,
                insertions: 0,
                deletions: 4
            }
        );
        assert_eq!(parse_shortstat(""), DiffStat::default());
        assert_eq!(parse_left_right_count("3\t250\n"), (3, 250));
        assert_eq!(parse_left_right_count(""), (0, 0));
    }

    /*
     * Entre ramas muy separadas la comparación lista como mucho
     * COMPARE_COMMIT_LIMIT commits por lado, pero los totales y el resumen de
     * archivos son los reales.
     */
    #[test]
    fn lists_at_most_the_limit_but_counts_every_commit_and_file() {
        let repo = diverged_repo();
        let extra = COMPARE_COMMIT_LIMIT + 25;
        // Un archivo nuevo por commit, con un solo `fast-import`. Fechas
        // posteriores a las de los commits base: con un reloj que va hacia
        // atrás, los recuentos de git (`a..b`) pueden equivocarse.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut stream = String::new();
        for index in 0..extra {
            let message = format!("extra {index}");
            let content = format!("{index}\n");
            stream.push_str(&format!(
                "commit refs/heads/feature\ncommitter T <t@e> {} +0000\ndata {}\n{message}\n",
                now + 10 + index as u64,
                message.len()
            ));
            if index == 0 {
                stream.push_str("from refs/heads/feature^0\n");
            }
            stream.push_str(&format!(
                "M 644 inline extra-{index}.txt\ndata {}\n{content}\n",
                content.len()
            ));
        }
        repo.git_stdin(&["fast-import", "--quiet"], &stream);

        let comparison = compare_branches(&repo.path, "main", "feature").unwrap();

        assert_eq!(comparison.ahead, 2 + extra);
        assert_eq!(comparison.ahead_commits.len(), COMPARE_COMMIT_LIMIT);
        assert_eq!(comparison.ahead_omitted(), 2 + extra - COMPARE_COMMIT_LIMIT);
        assert_eq!(
            comparison.ahead_commits[0].message,
            format!("extra {}", extra - 1)
        );
        assert_eq!(comparison.behind, 1);
        assert_eq!(comparison.behind_omitted(), 0);
        // feature.txt (+2) y los extra (+1 cada uno) aparecen; main.txt (-1) se va.
        assert_eq!(comparison.files_changed, 2 + extra);
        assert_eq!(comparison.insertions, 2 + extra as u64);
        assert_eq!(comparison.deletions, 1);
    }

    // --- comparación navegable ------------------------------------------------

    fn paths(files: &ComparisonFiles) -> Vec<(&str, &str)> {
        files
            .files
            .iter()
            .map(|file| (file.status.as_str(), file.path.as_str()))
            .collect()
    }

    /*
     * main añadió main.txt después de que feature se separara. "Desde que se
     * separaron" muestra solo lo de feature; el diff directo además ve
     * main.txt como borrado (feature no lo tiene).
     */
    #[test]
    fn since_divergence_shows_only_the_head_side_and_direct_shows_both() {
        let repo = diverged_repo();
        let base_commit = repo
            .git(&["merge-base", "main", "feature"])
            .trim()
            .to_string();

        let since = compare_revisions(
            &repo.path,
            "main",
            "feature",
            CompareMode::SinceDivergence,
            CompareLimits::default(),
        )
        .unwrap();
        assert_eq!(
            since.revisions.merge_base.as_deref(),
            Some(base_commit.as_str())
        );
        assert_eq!(
            since.revisions.head,
            repo.git(&["rev-parse", "feature"]).trim()
        );
        assert_eq!((since.ahead, since.behind), (2, 1));
        assert_eq!(since.ahead_commits[0].message, "feature two");
        assert_eq!(since.ahead_commits[0].full_hash, since.revisions.head);
        assert_eq!(paths(&since.files), [("A", "feature.txt")]);
        assert_eq!(
            (
                since.files.total,
                since.files.insertions,
                since.files.deletions
            ),
            (1, 2, 0)
        );

        let direct = compare_revisions(
            &repo.path,
            "main",
            "feature",
            CompareMode::Direct,
            CompareLimits::default(),
        )
        .unwrap();
        assert_eq!(
            paths(&direct.files),
            [("A", "feature.txt"), ("D", "main.txt")]
        );
        assert_eq!((direct.files.insertions, direct.files.deletions), (2, 1));
        assert_eq!(direct.files.omitted(), 0);

        // Cambiar de modo sobre la comparación ya fijada da lo mismo.
        assert_eq!(
            get_comparison_files(&repo.path, &since.revisions, CompareMode::Direct, 100).unwrap(),
            direct.files
        );

        let diff = get_comparison_diff(
            &repo.path,
            &since.revisions,
            CompareMode::Direct,
            "main.txt",
            None,
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(diff.diff.contains("deleted file mode"), "{}", diff.diff);
        assert!(diff.diff.contains("-main"));
        assert_eq!(CompareMode::default(), CompareMode::SinceDivergence);
        assert_eq!(CompareMode::SinceDivergence.notation("a", "b"), "a...b");
        assert_eq!(CompareMode::Direct.notation("a", "b"), "a b");
    }

    #[test]
    fn a_rename_keeps_both_paths_and_its_diff_is_a_rename() {
        let repo = TestRepo::new();
        let body = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n";
        repo.commit_file("docs/old name.txt", body, "base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.git(&["mv", "docs/old name.txt", "docs/año nuevo.txt"]);
        repo.write("docs/año nuevo.txt", format!("{body}line 7\n"));
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-m", "rename"]);

        let comparison = compare_revisions(
            &repo.path,
            "main",
            "feature",
            CompareMode::SinceDivergence,
            CompareLimits::default(),
        )
        .unwrap();
        assert_eq!(
            comparison.files.files,
            [ComparedFile {
                path: "docs/año nuevo.txt".into(),
                old_path: Some("docs/old name.txt".into()),
                status: "R".into(),
                additions: 1,
                deletions: 0,
            }]
        );

        let diff = get_comparison_diff(
            &repo.path,
            &comparison.revisions,
            CompareMode::SinceDivergence,
            "docs/año nuevo.txt",
            Some("docs/old name.txt"),
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(
            diff.diff.contains("rename from docs/old name.txt"),
            "{}",
            diff.diff
        );
        assert!(diff.diff.contains("rename to docs/año nuevo.txt"));
        assert!(diff.diff.contains("+line 7"));

        let json = serde_json::to_value(&comparison.files.files[0]).unwrap();
        assert_eq!(json["oldPath"], "docs/old name.txt");
    }

    // Las revisiones se fijan al abrir: si la rama avanza, la comparación ya
    // abierta no cambia; volver a resolver (Refresh) sí ve lo nuevo.
    #[test]
    fn a_pinned_comparison_does_not_move_with_the_branch() {
        let repo = diverged_repo();
        let pinned = resolve_comparison(&repo.path, "main", "feature").unwrap();

        repo.git(&["checkout", "feature"]);
        repo.commit_file("later.txt", "later\n", "later");

        let files =
            get_comparison_files(&repo.path, &pinned, CompareMode::SinceDivergence, 100).unwrap();
        assert_eq!(paths(&files), [("A", "feature.txt")]);

        let refreshed = resolve_comparison(&repo.path, "main", "feature").unwrap();
        assert_ne!(refreshed.head, pinned.head);
        let files = get_comparison_files(&repo.path, &refreshed, CompareMode::SinceDivergence, 100)
            .unwrap();
        assert_eq!(paths(&files), [("A", "feature.txt"), ("A", "later.txt")]);
    }

    #[test]
    fn revisions_without_common_history_only_have_a_direct_diff() {
        let repo = diverged_repo();
        repo.git(&["checkout", "--orphan", "pages"]);
        repo.git(&["rm", "-rf", "--quiet", "."]);
        repo.commit_file("index.html", "<h1>hi</h1>\n", "pages root");
        repo.git(&["checkout", "main"]);

        let since = compare_revisions(
            &repo.path,
            "main",
            "pages",
            CompareMode::SinceDivergence,
            CompareLimits::default(),
        )
        .unwrap();
        assert!(!since.has_common_history());
        assert_eq!(since.files, ComparisonFiles::default());
        assert_eq!((since.ahead, since.behind), (1, 2));

        let error = get_comparison_diff(
            &repo.path,
            &since.revisions,
            CompareMode::SinceDivergence,
            "index.html",
            None,
            &DiffOptions::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            format!("Failed to get comparison diff: {NO_COMMON_HISTORY}")
        );

        let direct =
            get_comparison_files(&repo.path, &since.revisions, CompareMode::Direct, 100).unwrap();
        assert_eq!(
            paths(&direct),
            [("A", "index.html"), ("D", "main.txt"), ("D", "shared.txt")]
        );
    }

    #[test]
    fn an_unknown_revision_fails_with_a_clear_message() {
        let repo = diverged_repo();

        for revision in ["nope", "--all"] {
            let error = compare_revisions(
                &repo.path,
                "main",
                revision,
                CompareMode::Direct,
                CompareLimits::default(),
            )
            .unwrap_err();
            assert_eq!(
                error.message(),
                format!("Failed to compare revisions: Unknown branch or revision '{revision}'.")
            );
        }
        // SHAs abreviados, tags y expresiones de revisión valen.
        let short = repo
            .git(&["rev-parse", "--short", "feature~1"])
            .trim()
            .to_string();
        let resolved = resolve_comparison(&repo.path, "HEAD~1", &short).unwrap();
        assert_eq!(resolved.base, repo.git(&["rev-parse", "main~1"]).trim());
    }

    // Muchos archivos: se listan los primeros `limit`, los totales son todos.
    #[test]
    fn lists_at_most_the_file_limit_but_counts_every_file() {
        let repo = diverged_repo();
        let mut stream = String::from(
            "commit refs/heads/feature\ncommitter T <t@e> 1900000000 +0000\ndata 4\nmany\nfrom refs/heads/feature^0\n",
        );
        for index in 0..30 {
            stream.push_str(&format!(
                "M 644 inline many/file {index:02}.txt\ndata 2\n{}\n\n",
                index % 10
            ));
        }
        repo.git_stdin(&["fast-import", "--quiet"], &stream);

        let comparison = compare_revisions(
            &repo.path,
            "main",
            "feature",
            CompareMode::SinceDivergence,
            CompareLimits {
                commits: 200,
                files: 10,
            },
        )
        .unwrap();
        assert_eq!(comparison.files.files.len(), 10);
        assert_eq!(comparison.files.total, 31);
        assert_eq!(comparison.files.omitted(), 21);
        // feature.txt (+2) y 30 archivos de una línea.
        assert_eq!(comparison.files.insertions, 32);
    }

    #[test]
    fn parses_compared_files_with_renames_binaries_and_a_limit() {
        let status = "M\0src/app.rs\0R090\0old.txt\0new.txt\0A\0img.png\0";
        let stats = "3\t1\tsrc/app.rs\x002\t1\t\0old.txt\0new.txt\x00-\t-\timg.png\0";

        let files = parse_compared_files(status, stats, 2);
        assert_eq!(files.total, 3);
        assert_eq!((files.insertions, files.deletions), (5, 2));
        assert_eq!(
            files.files,
            [
                ComparedFile {
                    path: "src/app.rs".into(),
                    old_path: None,
                    status: "M".into(),
                    additions: 3,
                    deletions: 1,
                },
                ComparedFile {
                    path: "new.txt".into(),
                    old_path: Some("old.txt".into()),
                    status: "R".into(),
                    additions: 2,
                    deletions: 1,
                },
            ]
        );
        assert_eq!(parse_compared_files("", "", 10), ComparisonFiles::default());
    }

    #[test]
    fn branches_that_merge_cleanly_have_no_conflicts() {
        let repo = diverged_repo();

        assert_eq!(
            check_merge_conflicts(&repo.path, "feature", "main").unwrap(),
            MergeConflictCheck {
                has_conflicts: false,
                conflicted_files: vec![]
            }
        );
    }

    #[test]
    fn an_unknown_branch_fails_with_the_read_prefix() {
        let repo = diverged_repo();

        let error = check_merge_conflicts(&repo.path, "nope", "main").unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to check merge conflicts: merge-tree: nope - not something we can merge\n"
        );
    }

    /*
     * La app Electron tenía aquí un defecto: con el criterio de `git.raw`, el
     * código 1 sin stderr con el que merge-tree anuncia los conflictos no era
     * un error, y el diálogo de merge decía "No conflicts". Ahora se decide por
     * código de salida y se listan solo las rutas, no los mensajes.
     */
    #[test]
    fn reports_real_conflicts_with_only_the_conflicted_paths() {
        let repo = diverged_repo();
        repo.git(&["checkout", "feature"]);
        repo.commit_file("shared.txt", "feature\n", "feature edits shared");
        repo.commit_file("año ñandú.txt", "feature\n", "feature adds a unicode file");
        repo.git(&["checkout", "main"]);
        repo.commit_file("shared.txt", "main\n", "main edits shared");
        repo.commit_file("año ñandú.txt", "main\n", "main adds a unicode file");

        let output = Git::new(&repo.path)
            .exec([
                "merge-tree",
                "--write-tree",
                "--name-only",
                "main",
                "feature",
            ])
            .unwrap();
        assert_eq!(output.exit_code, Some(1));
        assert!(output.stdout_text().contains("CONFLICT"));
        assert!(output.stderr.is_empty());

        assert_eq!(
            check_merge_conflicts(&repo.path, "feature", "main").unwrap(),
            MergeConflictCheck {
                has_conflicts: true,
                conflicted_files: vec!["año ñandú.txt".to_string(), "shared.txt".to_string()],
            }
        );
        // La predicción no toca el working tree ni deja un merge a medias.
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert!(!repo.path.join(".git").join("MERGE_HEAD").exists());

        let json =
            serde_json::to_value(check_merge_conflicts(&repo.path, "feature", "main").unwrap())
                .unwrap();
        assert_eq!(json["hasConflicts"], true);
        assert_eq!(json["conflictedFiles"][1], "shared.txt");
    }

    // Antes cualquier error que no pareciera "fatal" se tomaba por conflicto:
    // un repositorio que ya no existe salía como `hasConflicts: true`.
    #[test]
    fn a_missing_directory_fails_as_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");

        assert_eq!(
            check_merge_conflicts(&missing, "feature", "main")
                .unwrap_err()
                .message(),
            "Failed to check merge conflicts: Cannot use simple-git on a directory that does not exist"
        );
        assert!(
            check_merge_conflicts(dir.path(), "feature", "main")
                .unwrap_err()
                .message()
                .starts_with("Failed to check merge conflicts: fatal: not a git repository"),
        );
    }

    #[test]
    fn lists_conflicted_files_from_the_nul_separated_output() {
        let stdout = [
            "a2cabcb7d802362f678b84969dd8a7923538d757",
            "src/app.js",
            "src/app.js",
            "docs/read me.md",
            "",
            "1",
            "src/app.js",
            "Auto-merging",
            "Auto-merging src/app.js\n",
            "1",
            "src/app.js",
            "CONFLICT (contents)",
            "CONFLICT (content): Merge conflict in src/app.js\n",
            "",
        ]
        .join("\0");

        assert_eq!(
            conflicted_files_from(&stdout),
            ["src/app.js", "docs/read me.md"]
        );
        assert!(conflicted_files_from("").is_empty());
        assert!(is_object_id("a2cabcb7d802362f678b84969dd8a7923538d757"));
        assert!(!is_object_id(
            "merge-tree: nope - not something we can merge"
        ));
    }
}
