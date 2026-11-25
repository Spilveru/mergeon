//! Historial de un archivo siguiendo sus renombres (`git log --follow`) y el
//! parche de ese archivo en una de sus revisiones.
//!
//! No tiene canal en la app Electron: es la lectura de la pestaña "File
//! history".

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::diff::{DiffOptions, DiffPayload, git_diff_payload};
use crate::error::{Inner, QueryResult, read};
use crate::exec;
use crate::paths::normalize_repository_file_path;
use crate::text::js_trim;

/// Revisiones por página.
pub const FILE_HISTORY_PAGE_SIZE: u32 = 100;

/// Marca de cabecera de cada revisión en la salida de `git log`. Con `-z` las
/// rutas van entre NUL y literales, así que una ruta podría contener casi
/// cualquier cosa; la cabecera solo se busca donde toca un estado o una
/// cabecera nueva, nunca donde toca una ruta.
const HEADER_MARK: &str = "\u{1e}mergeon-file-history\u{1f}";
const FIELD_SEPARATOR: char = '\u{1f}';

/// Opciones de [`get_file_history`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileHistoryOptions {
    /// Revisión desde la que se recorre la historia (`HEAD` si falta). La
    /// ruta pedida es la que tenía el archivo en esa revisión.
    pub revision: Option<String>,
    /// Revisiones que se saltan (las de las páginas ya cargadas).
    pub skip: Option<u32>,
    /// Tamaño de la página; ausente o 0 es [`FILE_HISTORY_PAGE_SIZE`].
    pub max_count: Option<u32>,
}

/// Una revisión del archivo: un commit que lo tocó.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRevision {
    /// Hash completo.
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    /// Fecha de autor en ISO 8601 estricto.
    pub date: String,
    pub subject: String,
    /// Hashes completos de los padres; vacío en el commit raíz.
    pub parents: Vec<String>,
    /// Letra de `--name-status` (`A`, `M`, `D`, `R`, `C`, `T`). Vacía si git
    /// no dio estado (un merge, por ejemplo).
    pub status: String,
    /// Similitud de un renombre o una copia (`R087` → 87).
    pub similarity: Option<u8>,
    /// Ruta que tenía el archivo en esta revisión.
    pub path: String,
    /// En un renombre o una copia, la ruta de origen.
    pub previous_path: Option<String>,
}

impl FileRevision {
    pub fn is_rename(&self) -> bool {
        self.status == "R"
    }
}

/// Una página del historial de un archivo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryPage {
    /// De la más nueva a la más vieja.
    pub revisions: Vec<FileRevision>,
    /// Hay más revisiones después de esta página.
    pub has_more: bool,
}

fn page_size(value: Option<u32>) -> u32 {
    value
        .filter(|count| *count != 0)
        .unwrap_or(FILE_HISTORY_PAGE_SIZE)
}

fn format_argument() -> String {
    let fields = ["%H", "%h", "%an", "%ae", "%aI", "%P", "%s"].join(&FIELD_SEPARATOR.to_string());
    format!("--format={HEADER_MARK}{fields}")
}

/// `R087` → (`R`, 87).
fn split_status(code: &str) -> (String, Option<u8>) {
    let mut chars = code.chars();
    let letter: String = chars.next().map(String::from).unwrap_or_default();
    let digits = chars.as_str();
    let similarity = (!digits.is_empty())
        .then(|| digits.parse::<u8>().ok())
        .flatten();
    (letter, similarity)
}

fn parse_header(header: &str) -> Option<FileRevision> {
    let mut fields = header.splitn(7, FIELD_SEPARATOR);
    let hash = fields.next()?;
    let short_hash = fields.next()?;
    let author = fields.next()?;
    let email = fields.next()?;
    let date = fields.next()?;
    let parents = fields.next()?;
    // El asunto va el último: no puede llevar NUL, pero sí cualquier otra cosa.
    let subject = fields.next()?;
    Some(FileRevision {
        hash: hash.to_string(),
        short_hash: short_hash.to_string(),
        author: author.to_string(),
        email: email.to_string(),
        date: date.to_string(),
        subject: subject.to_string(),
        parents: parents
            .split(' ')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
        ..FileRevision::default()
    })
}

/*
 * Salida de `git log --follow -z --name-status --format=<marca><campos>`:
 *
 *   <marca><campos>\0\n<estado>\0<ruta>\0[<destino>\0]<marca><campos>\0...
 *
 * Cada cabecera termina en NUL; el bloque de `--name-status` de esa revisión
 * empieza con un salto de línea y trae `<estado>\0<ruta>\0`, o
 * `<estado>\0<origen>\0<destino>\0` en renombres y copias. Un commit sin
 * estado (un merge) no trae bloque.
 *
 * `path` es la ruta que se pidió: la que tiene el archivo en la revisión de
 * partida. Recorriendo hacia atrás, un renombre cambia la ruta que tenían las
 * revisiones anteriores, y así se sabe qué ruta tenía en las que no traen
 * estado.
 */
pub(crate) fn parse_file_history(output: &str, path: &str) -> Vec<FileRevision> {
    let mut revisions: Vec<FileRevision> = Vec::new();
    let mut current_path = path.to_string();
    // La revisión abierta todavía no recibió su estado.
    let mut pending: Option<FileRevision> = None;

    let mut tokens = output.split('\0');
    while let Some(token) = tokens.next() {
        let token = token.trim_start_matches('\n');
        if token.is_empty() {
            continue;
        }
        if let Some(header) = token.strip_prefix(HEADER_MARK) {
            if let Some(mut open) = pending.take() {
                open.path = current_path.clone();
                revisions.push(open);
            }
            pending = parse_header(header);
            continue;
        }

        // Un estado: le siguen una o dos rutas.
        let (status, similarity) = split_status(js_trim(token));
        let first = tokens.next().unwrap_or_default().to_string();
        let (previous, target) = if status == "R" || status == "C" {
            let target = tokens.next().unwrap_or_default().to_string();
            (Some(first), target)
        } else {
            (None, first)
        };
        let Some(mut open) = pending.take() else {
            // Un estado sin cabecera: solo puede ser el segundo de una misma
            // revisión. Se ignora; la primera entrada ya dijo la ruta.
            continue;
        };
        open.status = status;
        open.similarity = similarity;
        open.path = if target.is_empty() {
            current_path.clone()
        } else {
            target
        };
        if open.status == "R"
            && let Some(previous) = previous.as_ref().filter(|previous| !previous.is_empty())
        {
            current_path = previous.clone();
        }
        open.previous_path = previous.filter(|previous| !previous.is_empty());
        revisions.push(open);
    }
    if let Some(mut open) = pending.take() {
        open.path = current_path;
        revisions.push(open);
    }
    revisions
}

/// `true` si el repositorio no tiene ningún commit en HEAD (recién creado).
fn head_is_unborn(git: &mergeon_gitcmd::Git) -> bool {
    exec::exec(git, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .map(|output| output.exit_code != Some(0))
        .unwrap_or(false)
}

fn file_history(
    repo_path: &Path,
    file_path: &str,
    options: &FileHistoryOptions,
) -> Inner<FileHistoryPage> {
    let git = exec::open(repo_path)?;
    let normalized_path = normalize_repository_file_path(repo_path, file_path)?;
    let skip = options.skip.unwrap_or(0) as usize;
    let page = page_size(options.max_count) as usize;

    /*
     * `--skip` no vale con `--follow`: git sigue el renombre al calcular el
     * diff de cada revisión que enseña, y las que se salta no lo calculan, así
     * que la página siguiente perdía el nombre anterior y salía vacía. Se pide
     * desde el principio hasta el final de la página (más una, para saber si
     * hay más) y se descartan las ya vistas.
     */
    let mut args = vec![
        "log".to_string(),
        "--follow".to_string(),
        "-z".to_string(),
        "--name-status".to_string(),
        "--date=iso-strict".to_string(),
        format!("--max-count={}", skip + page + 1),
        format_argument(),
    ];
    if let Some(revision) = options
        .revision
        .as_deref()
        .map(js_trim)
        .filter(|revision| !revision.is_empty())
    {
        args.push("--end-of-options".to_string());
        args.push(revision.to_string());
    }
    args.push("--".to_string());
    args.push(normalized_path.clone());

    let output = match exec::raw(&git, &args) {
        Ok(output) => output,
        // Un repositorio sin commits no tiene historia, no es un error.
        Err(_) if options.revision.is_none() && head_is_unborn(&git) => String::new(),
        Err(error) => return Err(error.into()),
    };

    let mut revisions = parse_file_history(&output, &normalized_path);
    let has_more = revisions.len() > skip + page;
    revisions.truncate(skip + page);
    let revisions = if skip >= revisions.len() {
        Vec::new()
    } else {
        revisions.split_off(skip)
    };
    Ok(FileHistoryPage {
        revisions,
        has_more,
    })
}

/// Historial de `file_path` siguiendo sus renombres: una página de
/// revisiones, de la más nueva a la más vieja, cada una con la ruta que tenía
/// el archivo en ella. Un archivo sin commits devuelve una página vacía.
pub fn get_file_history(
    repo_path: &Path,
    file_path: &str,
    options: &FileHistoryOptions,
) -> QueryResult<FileHistoryPage> {
    read("Failed to get file history", || {
        file_history(repo_path, file_path, options)
    })
}

/// Parche de un archivo en una de sus revisiones. A diferencia de
/// `get_commit_diff`, si la revisión lo renombró (`previous_path`) se piden
/// las dos rutas y git enseña el renombre con sus cambios, no un archivo nuevo
/// entero.
pub fn get_file_revision_diff(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
    previous_path: Option<&str>,
    options: &DiffOptions,
) -> QueryResult<DiffPayload> {
    read("Failed to get commit diff", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;
        let previous = previous_path
            .filter(|previous| !previous.is_empty())
            .map(|previous| normalize_repository_file_path(repo_path, previous))
            .transpose()?
            .filter(|previous| *previous != normalized_path);

        // Como `get_commit_diff`: sin cabecera del commit y, en un merge, lo
        // que trajo a la rama.
        let mut args = vec![
            "show".to_string(),
            "--format=".to_string(),
            "--first-parent".to_string(),
            "-M".to_string(),
        ];
        let mut extra = Vec::new();
        options.push_args(&mut extra);
        args.extend(extra.into_iter().map(str::to_string));
        args.push("--end-of-options".to_string());
        args.push(commit_hash.to_string());
        args.push("--".to_string());
        if let Some(previous) = previous {
            args.push(previous);
        }
        args.push(normalized_path);

        git_diff_payload(&git, &args)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn subjects(page: &FileHistoryPage) -> Vec<&str> {
        page.revisions
            .iter()
            .map(|revision| revision.subject.as_str())
            .collect()
    }

    fn history(repo: &TestRepo, path: &str) -> FileHistoryPage {
        get_file_history(&repo.path, path, &FileHistoryOptions::default()).unwrap()
    }

    fn rename(repo: &TestRepo, from: &str, to: &str, message: &str) -> String {
        if let Some(parent) = repo.path.join(to).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        repo.git(&["mv", "--", from, to]);
        repo.git(&["commit", "-m", message]);
        repo.head()
    }

    #[test]
    fn parses_headers_statuses_and_renames() {
        let header = |hash: &str, subject: &str| {
            format!(
                "{HEADER_MARK}{hash}\u{1f}{}\u{1f}Ann\u{1f}ann@x\u{1f}2026-01-01T00:00:00Z\u{1f}p1 p2\u{1f}{subject}\0",
                &hash[..3]
            )
        };
        let output = [
            header("aaaaaaa", "edit c"),
            "\nM\0c.txt\0".to_string(),
            header("bbbbbbb", "rename"),
            "\nR091\0b.txt\0c.txt\0".to_string(),
            header("ccccccc", "merge"),
            header("ddddddd", "add b"),
            "\nA\0b.txt\0".to_string(),
        ]
        .concat();

        let revisions = parse_file_history(&output, "c.txt");

        type Shown<'a> = (&'a str, &'a str, Option<u8>, &'a str, Option<&'a str>);
        let shown: Vec<Shown> = revisions
            .iter()
            .map(|revision| {
                (
                    revision.subject.as_str(),
                    revision.status.as_str(),
                    revision.similarity,
                    revision.path.as_str(),
                    revision.previous_path.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("edit c", "M", None, "c.txt", None),
                ("rename", "R", Some(91), "c.txt", Some("b.txt")),
                // Sin estado: la ruta que tenía en ese punto de la historia.
                ("merge", "", None, "b.txt", None),
                ("add b", "A", None, "b.txt", None),
            ]
        );
        assert_eq!(revisions[0].parents, ["p1", "p2"]);
        assert_eq!(revisions[0].short_hash, "aaa");
        assert_eq!(revisions[0].email, "ann@x");
    }

    #[test]
    fn a_path_that_looks_like_a_header_or_a_status_stays_a_path() {
        let output = format!(
            "{HEADER_MARK}h\u{1f}h\u{1f}A\u{1f}a@x\u{1f}d\u{1f}\u{1f}s\0\nR100\0M\0{}weird\0",
            "\u{1e}"
        );
        let revisions = parse_file_history(&output, "\u{1e}weird");
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].previous_path.as_deref(), Some("M"));
        assert_eq!(revisions[0].path, "\u{1e}weird");
        assert!(revisions[0].parents.is_empty());
    }

    // a → b → c con cambios entre medias: cada revisión lleva la ruta que
    // tenía entonces y los renombres dicen de dónde vienen.
    #[test]
    fn follows_chained_renames() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "create a");
        repo.commit_file("a.txt", "one\ntwo\n", "edit a");
        rename(&repo, "a.txt", "b.txt", "a to b");
        repo.commit_file("b.txt", "one\ntwo\nthree\n", "edit b");
        rename(&repo, "b.txt", "dir/c.txt", "b to c");
        repo.commit_file("other.txt", "noise\n", "unrelated");

        let page = history(&repo, "dir/c.txt");

        assert_eq!(
            subjects(&page),
            ["b to c", "edit b", "a to b", "edit a", "create a"]
        );
        let paths: Vec<(&str, &str, Option<&str>)> = page
            .revisions
            .iter()
            .map(|revision| {
                (
                    revision.status.as_str(),
                    revision.path.as_str(),
                    revision.previous_path.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            paths,
            [
                ("R", "dir/c.txt", Some("b.txt")),
                ("M", "b.txt", None),
                ("R", "b.txt", Some("a.txt")),
                ("M", "a.txt", None),
                ("A", "a.txt", None),
            ]
        );
        assert_eq!(page.revisions[0].similarity, Some(100));
        assert!(page.revisions[0].is_rename());
        assert!(!page.has_more);
        assert_eq!(page.revisions[0].hash.len(), 40);
        assert_eq!(page.revisions[0].author, "Test User");
        assert!(page.revisions[0].date.starts_with("20"));
    }

    // `--skip` rompe `--follow`: la segunda página tiene que seguir por el
    // nombre anterior.
    #[test]
    fn pages_keep_following_the_renames() {
        let repo = TestRepo::new();
        for index in 0..3 {
            repo.commit_file("a.txt", &format!("{index}\n"), &format!("a{index}"));
        }
        rename(&repo, "a.txt", "b.txt", "a to b");
        for index in 0..2 {
            repo.commit_file("b.txt", &format!("b{index}\n"), &format!("b{index}"));
        }
        rename(&repo, "b.txt", "c.txt", "b to c");
        repo.commit_file("c.txt", "c\n", "c0");

        let page = |skip: u32| {
            get_file_history(
                &repo.path,
                "c.txt",
                &FileHistoryOptions {
                    skip: Some(skip),
                    max_count: Some(3),
                    ..FileHistoryOptions::default()
                },
            )
            .unwrap()
        };

        let first = page(0);
        assert_eq!(subjects(&first), ["c0", "b to c", "b1"]);
        assert!(first.has_more);
        let second = page(3);
        assert_eq!(subjects(&second), ["b0", "a to b", "a2"]);
        assert_eq!(second.revisions[0].path, "b.txt");
        assert_eq!(second.revisions[2].path, "a.txt");
        assert!(second.has_more);
        let third = page(6);
        assert_eq!(subjects(&third), ["a1", "a0"]);
        assert!(!third.has_more);
        assert!(page(9).revisions.is_empty());
    }

    #[test]
    fn a_deleted_and_recreated_file_shows_both_lives() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "first life\n", "create");
        repo.git(&["rm", "-q", "file.txt"]);
        repo.git(&["commit", "-m", "delete"]);
        repo.commit_file("file.txt", "second life\n", "recreate");

        let page = history(&repo, "file.txt");

        let shown: Vec<(&str, &str)> = page
            .revisions
            .iter()
            .map(|revision| (revision.subject.as_str(), revision.status.as_str()))
            .collect();
        assert_eq!(shown, [("recreate", "A"), ("delete", "D"), ("create", "A")]);

        // Un archivo que ya no existe también tiene historia.
        repo.git(&["rm", "-q", "file.txt"]);
        repo.git(&["commit", "-m", "delete again"]);
        assert_eq!(history(&repo, "file.txt").revisions[0].status, "D");
    }

    #[test]
    fn merges_keep_both_sides_of_the_history() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit_file("file.txt", "base\nside\n", "side edit");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit_file("file.txt", "main\nbase\n", "main edit");
        repo.git(&["merge", "-q", "--no-edit", "side"]);
        let merge = repo.head();

        let page = history(&repo, "file.txt");

        let mut shown = subjects(&page);
        shown.sort_unstable();
        assert_eq!(shown, ["base", "main edit", "side edit"]);
        assert!(page.revisions.iter().all(|revision| revision.hash != merge));

        // Desde una revisión concreta: solo lo que alcanza esa revisión.
        let side = get_file_history(
            &repo.path,
            "file.txt",
            &FileHistoryOptions {
                revision: Some("side".into()),
                ..FileHistoryOptions::default()
            },
        )
        .unwrap();
        assert_eq!(subjects(&side), ["side edit", "base"]);
    }

    #[test]
    fn paths_with_spaces_and_unicode_are_literal() {
        let repo = TestRepo::new();
        repo.commit_file("docs/año nuevo.md", "uno\n", "crear");
        rename(&repo, "docs/año nuevo.md", "docs/日本 語.md", "renombrar");
        repo.commit_file("docs/日本 語.md", "uno\ndos\n", "editar");

        let page = history(&repo, "docs/日本 語.md");

        assert_eq!(subjects(&page), ["editar", "renombrar", "crear"]);
        assert_eq!(page.revisions[1].path, "docs/日本 語.md");
        assert_eq!(
            page.revisions[1].previous_path.as_deref(),
            Some("docs/año nuevo.md")
        );
        assert_eq!(page.revisions[2].path, "docs/año nuevo.md");

        // Separadores de Windows.
        let windows = history(&repo, "docs\\日本 語.md");
        assert_eq!(windows.revisions.len(), 3);
    }

    #[test]
    fn a_file_without_history_has_an_empty_page() {
        let repo = TestRepo::new();
        // Sin commits todavía.
        repo.write("new.txt", "x\n");
        let page = history(&repo, "new.txt");
        assert!(page.revisions.is_empty());
        assert!(!page.has_more);

        repo.commit_file("other.txt", "y\n", "first");
        assert!(history(&repo, "new.txt").revisions.is_empty());
        assert!(history(&repo, "never/existed.txt").revisions.is_empty());
    }

    #[test]
    fn rejects_paths_outside_and_unknown_revisions() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "x\n", "first");

        let error =
            get_file_history(&repo.path, "../x", &FileHistoryOptions::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to get file history: The file path must stay inside the repository."
        );

        let error = get_file_history(
            &repo.path,
            "file.txt",
            &FileHistoryOptions {
                revision: Some("--all".into()),
                ..FileHistoryOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            error.message().starts_with("Failed to get file history: "),
            "{error}"
        );
    }

    #[test]
    fn the_revision_diff_of_a_rename_shows_the_rename() {
        let repo = TestRepo::new();
        repo.commit_file("old name.txt", "one\ntwo\nthree\nfour\n", "create");
        repo.write("old name.txt", "one\ntwo\nthree\nfour\nfive\n");
        repo.git(&["add", "--", "old name.txt"]);
        rename(&repo, "old name.txt", "new name.txt", "rename and edit");
        let hash = repo.head();

        let diff = get_file_revision_diff(
            &repo.path,
            &hash,
            "new name.txt",
            Some("old name.txt"),
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(
            diff.diff.contains("rename from old name.txt"),
            "{}",
            diff.diff
        );
        assert!(diff.diff.contains("rename to new name.txt"));
        assert!(diff.diff.contains("+five"));
        assert!(!diff.diff.contains("+one"));

        // Sin la ruta anterior sale como archivo nuevo.
        let alone = get_file_revision_diff(
            &repo.path,
            &hash,
            "new name.txt",
            None,
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(alone.diff.contains("new file mode"));
        assert!(alone.diff.contains("+one"));
    }
}
