//! Resumen de un repositorio para la pantalla de inicio
//! (`electron/services/repository-summary.js`).

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

use mergeon_gitcmd::{Git, GitResult, get_structured_status};

use crate::error::{Inner, QueryResult, join, read};
use crate::exec;
use crate::fs_util::{read_utf8, stat};
use crate::inspect::is_not_git_repository_message;
use crate::remotes::{Remote, list_remotes};
use crate::text::{js_trim, parse_count};

const README_CANDIDATES: [&str; 4] = ["README.md", "readme.md", "README", "README.txt"];
const MAX_README_BYTES: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidReason {
    MissingPath,
    AccessError,
    NotGitRepository,
    UnknownError,
}

impl InvalidReason {
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidReason::MissingPath => "missing-path",
            InvalidReason::AccessError => "access-error",
            InvalidReason::NotGitRepository => "not-git-repository",
            InvalidReason::UnknownError => "unknown-error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySummary {
    /// Siempre `true`: un repo inválido se describe con `is_valid`, no con un error.
    pub success: bool,
    pub repo_path: String,
    /// Último segmento de la ruta.
    pub name: String,
    pub is_valid: bool,
    pub invalid_reason: Option<InvalidReason>,
    pub current_branch: Option<String>,
    pub changes_count: usize,
    /// Nombres de los remotos.
    pub remotes: Vec<String>,
    pub commit_count: u64,
    pub initial_commit_date: Option<String>,
    pub last_commit_date: Option<String>,
    /// Bytes que ocupan los objetos de `.git`.
    pub repository_size: u64,
    pub readme_path: Option<String>,
    /// `None` si no hay README o pesa demasiado para mostrarlo.
    pub readme_content: Option<String>,
    /// OID completo de HEAD con el que se contaron `commit_count` y las
    /// fechas. `None` sin commits o en un repo no válido.
    #[serde(default)]
    pub head_oid: Option<String>,
}

fn invalid_summary(repo_path: String, name: String, reason: InvalidReason) -> RepositorySummary {
    RepositorySummary {
        success: true,
        repo_path,
        name,
        is_valid: false,
        invalid_reason: Some(reason),
        current_branch: None,
        changes_count: 0,
        remotes: Vec::new(),
        commit_count: 0,
        initial_commit_date: None,
        last_commit_date: None,
        repository_size: 0,
        readme_path: None,
        readme_content: None,
        head_oid: None,
    }
}

// El original también miraba `error.code` (EACCES/EPERM), pero un error de git
// nunca lo trae.
fn infer_invalid_reason(message: &str) -> InvalidReason {
    if is_not_git_repository_message(message) {
        InvalidReason::NotGitRepository
    } else {
        InvalidReason::UnknownError
    }
}

static SIZE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(size|size-pack|size-garbage):\s*([0-9]+)").unwrap());

/*
 * Tamaño del repositorio = objetos de .git (sueltos + packs + basura), que es
 * lo que git reporta en `count-objects`. Antes se recorría el working tree
 * entero con un stat por archivo: en un monorepo eran decenas de miles de
 * llamadas por cada resumen y el dato mezclaba fuentes con artefactos
 * ignorados a medias. `count-objects -v` devuelve KiB.
 */
fn parse_repository_size(output: &str) -> u64 {
    let total_kib: u64 = output
        .split('\n')
        .filter_map(|line| SIZE_LINE.captures(js_trim(line)))
        .map(|captures| parse_count(&captures[2]))
        .sum();

    total_kib * 1024
}

fn repository_size(git: &Git) -> GitResult<u64> {
    Ok(parse_repository_size(&exec::raw(
        git,
        &["count-objects", "-v"],
    )?))
}

fn commit_date(git: &Git, revision: &str) -> GitResult<Option<String>> {
    let output = exec::raw(git, &["show", "-s", "--format=%cI", revision])?;
    let date = js_trim(&output);
    Ok((!date.is_empty()).then(|| date.to_string()))
}

/*
 * Fecha del primer commit sin padres que encuentra el recorrido desde HEAD
 * (el mismo que la primera línea de `rev-list --max-parents=0 HEAD`), en un
 * solo proceso y sin listar las demás raíces: con `-1` git para en cuanto la
 * encuentra. Antes eran `rev-list` y luego `show` del hash, en serie.
 */
fn root_commit_date(git: &Git) -> GitResult<Option<String>> {
    let output = exec::raw(
        git,
        &[
            "log",
            "--max-parents=0",
            "--max-count=1",
            "--format=%cI",
            "HEAD",
        ],
    )?;
    let date = js_trim(&output);
    Ok((!date.is_empty()).then(|| date.to_string()))
}

fn commit_count(git: &Git) -> GitResult<u64> {
    let output = exec::raw(git, &["rev-list", "--count", "HEAD"])?;
    Ok(parse_count(js_trim(&output)))
}

fn head_oid(git: &Git) -> GitResult<Option<String>> {
    let output = exec::exec(git, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    let oid = js_trim(&output.stdout_text()).to_string();
    Ok((output.success() && !oid.is_empty()).then_some(oid))
}

/// La parte del resumen que sale de recorrer el historial de HEAD: la cara
/// (`rev-list --count` recorre todos los commits) y la que solo cambia si
/// cambia HEAD.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct History {
    commit_count: u64,
    initial_commit_date: Option<String>,
    last_commit_date: Option<String>,
}

impl History {
    /// La de un resumen anterior, si se contó con este mismo HEAD.
    fn reusable(previous: Option<&RepositorySummary>, head_oid: Option<&str>) -> Option<Self> {
        let previous = previous.filter(|previous| previous.is_valid)?;
        (head_oid.is_some() && previous.head_oid.as_deref() == head_oid).then(|| History {
            commit_count: previous.commit_count,
            initial_commit_date: previous.initial_commit_date.clone(),
            last_commit_date: previous.last_commit_date.clone(),
        })
    }

    fn read(git: &Git) -> Inner<Self> {
        std::thread::scope(|scope| {
            let count = scope.spawn(|| commit_count(git));
            let initial = scope.spawn(|| root_commit_date(git));
            let last = commit_date(git, "HEAD");
            Ok(History {
                commit_count: join(count)?,
                initial_commit_date: join(initial)?,
                last_commit_date: last?,
            })
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Readme {
    path: Option<String>,
    content: Option<String>,
}

fn read_repository_readme(repo_path: &Path) -> Result<Readme, String> {
    for candidate in README_CANDIDATES {
        let candidate_path = repo_path.join(candidate);
        if !candidate_path.exists() {
            continue;
        }

        if stat(&candidate_path)?.len() > MAX_README_BYTES {
            return Ok(Readme {
                path: Some(candidate.to_string()),
                content: None,
            });
        }

        return Ok(Readme {
            path: Some(candidate.to_string()),
            content: Some(read_utf8(&candidate_path)?),
        });
    }

    Ok(Readme::default())
}

fn repository_summary(
    repo_path: &Path,
    previous: Option<&RepositorySummary>,
) -> Inner<RepositorySummary> {
    let repo_path_text = repo_path.to_string_lossy().into_owned();
    let name = repo_path_text
        .rsplit(['\\', '/'])
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(&repo_path_text)
        .to_string();

    if !repo_path.exists() {
        return Ok(invalid_summary(
            repo_path_text,
            name,
            InvalidReason::MissingPath,
        ));
    }

    let git = exec::open(repo_path)?;

    // El propio get_structured_status valida el repo: un fallo aquí produce el
    // resultado inválido sin gastar un `git status` extra solo para chequear.
    let status = match get_structured_status(repo_path) {
        Ok(status) => status,
        Err(error) => {
            return Ok(invalid_summary(
                repo_path_text,
                name,
                infer_invalid_reason(&error.message),
            ));
        }
    };

    /*
     * Un repo recién creado no tiene HEAD: `rev-list HEAD` falla y la app
     * Electron devolvía un error en vez del resumen. Sin commits no hay nada que
     * contar ni fechar; el resto del resumen (rama inicial, cambios, remotos,
     * README) sigue valiendo. `head_short_hash` ya lo resolvió el estado.
     */
    let has_commits = status.head_short_hash.is_some();
    let head_oid = if has_commits { head_oid(&git)? } else { None };
    let reused = History::reusable(previous, head_oid.as_deref());

    // Las lecturas son independientes: van en paralelo, como el Promise.all
    // del original. El `Git` se crea en este hilo para heredar su contexto.
    let (remotes, repository_size, history, readme) = std::thread::scope(|scope| {
        let remotes = scope.spawn(|| list_remotes(&git));
        let repository_size = scope.spawn(|| repository_size(&git));
        let history = scope.spawn(|| match reused {
            Some(history) => Ok(history),
            None if has_commits => History::read(&git),
            None => Ok(History::default()),
        });
        let readme = read_repository_readme(repo_path);
        (join(remotes), join(repository_size), join(history), readme)
    });
    let remotes: Vec<Remote> = remotes?;
    let repository_size = repository_size?;
    let history = history?;
    let readme = readme?;

    Ok(RepositorySummary {
        success: true,
        repo_path: repo_path_text,
        name,
        is_valid: true,
        invalid_reason: None,
        current_branch: Some(
            status
                .current
                .filter(|branch| !branch.is_empty())
                .unwrap_or_else(|| "HEAD".to_string()),
        ),
        changes_count: status.files.len(),
        remotes: remotes
            .into_iter()
            .map(|remote| remote.name)
            .filter(|name| !name.is_empty())
            .collect(),
        commit_count: history.commit_count,
        initial_commit_date: history.initial_commit_date,
        last_commit_date: history.last_commit_date,
        repository_size,
        readme_path: readme.path,
        readme_content: readme.content,
        head_oid,
    })
}

/// Canal `git:getRepositorySummary`.
pub fn get_repository_summary(repo_path: &Path) -> QueryResult<RepositorySummary> {
    refresh_repository_summary(repo_path, None)
}

/// Resumen nuevo de `repo_path` que reaprovecha el recuento de commits y las
/// fechas de `previous` si se calcularon con el mismo HEAD (`head_oid`): lo
/// demás (estado, remotos, tamaño, README) se vuelve a leer siempre.
pub fn refresh_repository_summary(
    repo_path: &Path,
    previous: Option<&RepositorySummary>,
) -> QueryResult<RepositorySummary> {
    read("Failed to get repository summary", || {
        repository_summary(repo_path, previous)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    /// Bytes que no comprimen, sin depender de un generador externo.
    fn noise(length: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn returns_invalid_for_a_missing_repo_path() {
        let summary =
            get_repository_summary(Path::new("/tmp/does-not-exist-repo-summary")).unwrap();

        assert!(summary.success);
        assert!(!summary.is_valid);
        assert_eq!(summary.invalid_reason, Some(InvalidReason::MissingPath));
        assert_eq!(summary.commit_count, 0);
        assert_eq!(summary.name, "does-not-exist-repo-summary");
        assert_eq!(summary.repo_path, "/tmp/does-not-exist-repo-summary");
    }

    #[test]
    fn returns_not_git_repository_for_an_existing_directory_without_git_initialized() {
        let root = tempfile::tempdir().unwrap();
        let repo_path = root.path().join("plain-directory");
        std::fs::create_dir_all(&repo_path).unwrap();

        let summary = get_repository_summary(&repo_path).unwrap();

        assert!(summary.success);
        assert!(!summary.is_valid);
        assert_eq!(
            summary.invalid_reason,
            Some(InvalidReason::NotGitRepository)
        );
        assert_eq!(summary.current_branch, None);
    }

    #[test]
    fn a_path_that_is_a_file_fails_like_the_original() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();

        let error = get_repository_summary(&file).unwrap_err();

        assert_eq!(
            error.message(),
            "Failed to get repository summary: Cannot use simple-git on a directory that does not exist"
        );
    }

    #[test]
    fn returns_summary_data_for_a_valid_repo_without_remotes() {
        let repo = TestRepo::new();
        repo.write("README.md", "# Demo\n\n- item\n");
        repo.empty_commit("initial commit");

        let summary = get_repository_summary(&repo.path).unwrap();

        assert!(summary.is_valid);
        assert_eq!(summary.invalid_reason, None);
        assert_eq!(summary.name, "repo");
        assert_eq!(summary.current_branch.as_deref(), Some("main"));
        assert_eq!(summary.changes_count, 1);
        assert!(summary.remotes.is_empty());
        assert_eq!(summary.commit_count, 1);
        assert!(summary.initial_commit_date.is_some());
        assert_eq!(summary.initial_commit_date, summary.last_commit_date);
        assert_eq!(summary.readme_path.as_deref(), Some("README.md"));
        assert!(summary.readme_content.unwrap().contains("# Demo"));
    }

    #[test]
    fn returns_remotes_and_commit_count_for_a_valid_repo_with_history() {
        let repo = TestRepo::new();
        repo.empty_commit("initial commit");
        repo.empty_commit("second commit");
        repo.add_bare_origin();

        let summary = get_repository_summary(&repo.path).unwrap();

        assert!(summary.is_valid);
        assert_eq!(summary.remotes, ["origin"]);
        assert_eq!(summary.commit_count, 2);
        assert_eq!(summary.readme_path, None);
        assert_eq!(summary.readme_content, None);
    }

    #[test]
    fn measures_the_git_object_store_instead_of_walking_the_working_tree() {
        let repo = TestRepo::new();

        // Un objeto que no comprime, para que .git pese algo medible en cualquier
        // plataforma: en Windows `count-objects` redondea a KiB por tamaño real y un
        // commit vacío queda en 0.
        repo.write("tracked.bin", noise(64 * 1024));
        repo.git(&["add", "tracked.bin"]);
        repo.git(&["commit", "-m", "initial commit"]);
        // Archivo grande sin versionar: no debe contar, solo los objetos de .git.
        repo.write("big.bin", vec![0u8; 4 * 1024 * 1024]);

        let summary = get_repository_summary(&repo.path).unwrap();

        assert!(summary.repository_size >= 64 * 1024);
        assert!(summary.repository_size < 1024 * 1024);
    }

    #[test]
    fn an_oversized_readme_is_named_but_not_loaded() {
        let repo = TestRepo::new();
        repo.empty_commit("initial commit");
        repo.write("README.md", "x".repeat(MAX_README_BYTES as usize + 1));

        let summary = get_repository_summary(&repo.path).unwrap();

        assert_eq!(summary.readme_path.as_deref(), Some("README.md"));
        assert_eq!(summary.readme_content, None);
    }

    // La app Electron fallaba aquí (`rev-list --count HEAD` sin HEAD).
    #[test]
    fn a_repository_without_commits_is_summarized_with_zero_commits() {
        let repo = TestRepo::new();
        repo.write("README.md", "# Nuevo\n");
        repo.add_bare_origin();

        let summary = get_repository_summary(&repo.path).unwrap();

        assert!(summary.is_valid);
        assert_eq!(summary.invalid_reason, None);
        assert_eq!(summary.current_branch.as_deref(), Some("main"));
        assert_eq!(summary.commit_count, 0);
        assert_eq!(summary.initial_commit_date, None);
        assert_eq!(summary.last_commit_date, None);
        assert_eq!(summary.head_oid, None);
        assert_eq!(summary.changes_count, 1);
        assert_eq!(summary.remotes, ["origin"]);
        assert_eq!(summary.readme_path.as_deref(), Some("README.md"));
    }

    /*
     * Contar los commits recorre todo el historial: si HEAD no se movió desde
     * el resumen anterior, el recuento y las fechas se reaprovechan, y lo
     * demás (cambios, README...) se vuelve a leer.
     */
    #[test]
    fn reuses_the_history_of_a_previous_summary_with_the_same_head() {
        let repo = TestRepo::new();
        let first_commit = repo.empty_commit("initial commit");
        let first = get_repository_summary(&repo.path).unwrap();
        assert_eq!(first.head_oid.as_deref(), Some(first_commit.as_str()));
        assert_eq!(first.changes_count, 0);

        // Un HEAD igual: no se recuenta (un recuento falso lo delata).
        let stale = RepositorySummary {
            commit_count: 999,
            ..first.clone()
        };
        repo.write("new.txt", "x\n");
        let refreshed = refresh_repository_summary(&repo.path, Some(&stale)).unwrap();
        assert_eq!(refreshed.commit_count, 999);
        assert_eq!(refreshed.initial_commit_date, first.initial_commit_date);
        assert_eq!(refreshed.changes_count, 1);

        // HEAD se movió: se cuenta de nuevo.
        let second_commit = repo.empty_commit("second commit");
        let moved = refresh_repository_summary(&repo.path, Some(&stale)).unwrap();
        assert_eq!(moved.commit_count, 2);
        assert_eq!(moved.head_oid.as_deref(), Some(second_commit.as_str()));
        assert_eq!(moved.initial_commit_date, first.initial_commit_date);

        // Sin HEAD conocido en el anterior tampoco se reaprovecha nada.
        let unknown = RepositorySummary {
            head_oid: None,
            ..stale
        };
        assert_eq!(
            refresh_repository_summary(&repo.path, Some(&unknown))
                .unwrap()
                .commit_count,
            2
        );
    }

    #[test]
    fn the_initial_date_is_the_root_commit_date() {
        let repo = TestRepo::new();
        let dated = |date: &str, message: &str| {
            Git::new(&repo.path)
                .with_env("GIT_COMMITTER_DATE", date)
                .raw(["commit", "--allow-empty", "-m", message])
                .unwrap();
        };
        dated("2020-01-01T00:00:00Z", "root");
        dated("2021-01-01T00:00:00Z", "middle");
        dated("2022-01-01T00:00:00Z", "tip");

        let summary = get_repository_summary(&repo.path).unwrap();
        assert_eq!(summary.commit_count, 3);
        assert_eq!(
            summary.initial_commit_date.as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
        assert_eq!(
            summary.last_commit_date.as_deref(),
            Some("2022-01-01T00:00:00Z")
        );
    }

    #[test]
    fn sums_the_sizes_reported_by_count_objects() {
        let output = "count: 3\nsize: 12\nin-pack: 40\npacks: 1\nsize-pack: 100\nprune-packable: 0\ngarbage: 0\nsize-garbage: 2\n";
        assert_eq!(parse_repository_size(output), 114 * 1024);
        assert_eq!(parse_repository_size(""), 0);
    }

    #[test]
    fn serializes_with_the_wire_names() {
        let summary =
            get_repository_summary(Path::new("/tmp/does-not-exist-repo-summary")).unwrap();
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["isValid"], false);
        assert_eq!(json["invalidReason"], "missing-path");
        assert_eq!(json["repoPath"], "/tmp/does-not-exist-repo-summary");
        assert_eq!(json["currentBranch"], serde_json::Value::Null);
        assert_eq!(json["repositorySize"], 0);
        assert_eq!(
            InvalidReason::NotGitRepository.as_str(),
            "not-git-repository"
        );
    }
}
