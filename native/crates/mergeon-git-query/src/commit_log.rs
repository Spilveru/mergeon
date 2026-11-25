//! Historial de commits (`electron/services/commit-log.js`).

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{Inner, QueryResult, read};
use crate::exec;
use crate::text::js_trim;

pub(crate) const FIELD_SEPARATOR: char = '\u{1f}';
pub(crate) const COMMIT_SEPARATOR: char = '\u{1e}';

const DEFAULT_MAX_COUNT: u32 = 100;

/// Opciones de `git:getCommits`. Un `max_count` ausente o 0 son 100 commits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitLogOptions {
    pub max_count: Option<u32>,
    pub skip: Option<u32>,
}

/// Opciones de `git:searchCommits`. Los filtros se combinan (todos deben
/// cumplirse); un `max_count` ausente o 0 son 100 commits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitSearchOptions {
    pub max_count: Option<u32>,
    /// Resultados que se saltan: la página siguiente empieza en `skip`.
    pub skip: Option<u32>,
    /// En el mensaje (texto fijo, sin distinguir mayúsculas).
    pub text: Option<String>,
    pub author: Option<String>,
    /// Si resuelve a un commit (y no hay filtros), ese commit.
    pub hash: Option<String>,
    /// Solo commits que tocan esta ruta (`-- <ruta>`; vale un pathspec).
    pub path: Option<String>,
    /// Solo la historia de esta rama o revisión, en vez de `--all`.
    pub branch: Option<String>,
    /// Desde esta fecha (`--since`; cualquier fecha que entienda git).
    pub since: Option<String>,
    /// Hasta esta fecha (`--until`).
    pub until: Option<String>,
    /// Commits cuyo diff añade o quita este texto (`-S`).
    pub code: Option<String>,
    /// `code` es una expresión regular sobre las líneas cambiadas (`-G`).
    pub code_regex: bool,
}

impl CommitSearchOptions {
    /// Hay algún filtro además del texto, el autor y el hash.
    pub fn has_filters(&self) -> bool {
        [
            &self.path,
            &self.branch,
            &self.since,
            &self.until,
            &self.code,
        ]
        .into_iter()
        .any(|value| !js_trim(value.as_deref().unwrap_or_default()).is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    /// Hash abreviado.
    pub hash: String,
    pub full_hash: String,
    pub message: String,
    pub author: String,
    pub email: String,
    /// Fecha de autor en ISO 8601 estricto.
    pub date: String,
    pub body: String,
    /// Decoración con rutas completas (`HEAD -> refs/heads/main, refs/tags/v1`).
    pub refs: String,
    /// Hashes completos de los padres separados por espacio; vacío en el commit raíz.
    pub parents: String,
}

fn max_count(value: Option<u32>) -> u32 {
    value
        .filter(|count| *count != 0)
        .unwrap_or(DEFAULT_MAX_COUNT)
}

fn pretty_argument() -> String {
    let format = ["%H", "%h", "%an", "%ae", "%aI", "%s", "%b", "%D", "%P"]
        .join(&FIELD_SEPARATOR.to_string());
    format!("--pretty=format:{format}{COMMIT_SEPARATOR}")
}

pub(crate) fn parse_commit_log_output(output: &str) -> Result<Vec<Commit>, String> {
    let mut commits = Vec::new();

    for entry in output.split(COMMIT_SEPARATOR) {
        let entry = js_trim(entry);
        if entry.is_empty() {
            continue;
        }

        let fields: Vec<&str> = entry.split(FIELD_SEPARATOR).collect();
        let &[
            full_hash,
            short_hash,
            author,
            email,
            date,
            message,
            body,
            refs,
            parents,
        ] = fields.as_slice()
        else {
            return Err(format!(
                "Unexpected commit log entry shape: expected 9 fields, received {}",
                fields.len()
            ));
        };

        commits.push(Commit {
            hash: short_hash.to_string(),
            full_hash: full_hash.to_string(),
            message: message.to_string(),
            author: author.to_string(),
            email: email.to_string(),
            date: date.to_string(),
            body: body.to_string(),
            refs: refs.to_string(),
            parents: parents.to_string(),
        });
    }

    Ok(commits)
}

fn commit_log(repo_path: &Path, options: &CommitLogOptions) -> Inner<Vec<Commit>> {
    let git = exec::open(repo_path)?;
    let skip = options.skip.unwrap_or(0);

    let mut args = vec![
        "log".to_string(),
        "--all".to_string(),
        /*
         * Orden por fecha, con los hijos siempre antes que sus padres. Con
         * `--topo-order` git muestra una línea de historia entera antes de
         * pasar a otra: si `development` tenía un commit más nuevo que el
         * último "Merge branch 'development'" de otra rama, salía primero toda
         * la cadena de merges y los commits de `development` que trajo cada uno
         * quedaban esperando abajo, cada uno con su carril abierto (un abanico
         * de carriles; `git log --graph` dibuja lo mismo). Por fecha, cada
         * commit aparece junto al merge que lo trajo.
         */
        "--date-order".to_string(),
        // Ruta completa de cada ref (refs/heads/…, refs/remotes/…): con el formato
        // corto una rama local con barra (`feature/login`) es indistinguible de una
        // remota, y el cliente tenía que adivinar el tipo.
        "--decorate=full".to_string(),
        format!("--max-count={}", max_count(options.max_count)),
        "--date=iso-strict".to_string(),
        pretty_argument(),
    ];

    if skip > 0 {
        args.push(format!("--skip={skip}"));
    }

    let output = exec::raw(&git, &args)?;
    Ok(parse_commit_log_output(&output)?)
}

/// Canal `git:getCommits`. Un solo proceso git por llamada.
pub fn get_commits(repo_path: &Path, options: &CommitLogOptions) -> QueryResult<Vec<Commit>> {
    read("Failed to get commits", || commit_log(repo_path, options))
}

/*
 * Búsqueda de commits en el propio git (no en el cliente): así no hay límite
 * artificial de "los últimos N commits cargados". `text` y `author` van como
 * patrones fixed-string insensibles a mayúsculas; si `hash` viene (y no hay
 * filtros), se intenta resolver ese commit exacto primero. Los filtros de
 * ruta, rama, fechas y contenido se suman, y `max_count` / `skip` paginan:
 * git deja de recorrer el historial al llenar la página.
 */
fn search_commit_log(repo_path: &Path, options: &CommitSearchOptions) -> Inner<Vec<Commit>> {
    let git = exec::open(repo_path)?;
    let field = |value: &Option<String>| js_trim(value.as_deref().unwrap_or_default()).to_string();
    let text = field(&options.text);
    let author = field(&options.author);
    let hash = field(&options.hash);
    let branch = field(&options.branch);
    let since = field(&options.since);
    let until = field(&options.until);
    let code = field(&options.code);
    let path = field(&options.path).replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path).to_string();
    let filtered = options.has_filters();

    if !hash.is_empty() && !filtered {
        // El hash viene de la caja de búsqueda: tras `--end-of-options` git lo
        // trata siempre como revisión, nunca como opción. La app Electron lo
        // pasaba en posición de opción, y buscar `--all` devolvía el último
        // commit de cualquier rama.
        let exact = exec::raw(
            &git,
            &[
                "log",
                "-1",
                "--decorate=full",
                "--date=iso-strict",
                &pretty_argument(),
                "--end-of-options",
                &hash,
            ],
        )
        .map_err(|error| error.message)
        .and_then(|output| parse_commit_log_output(&output));

        // Si el texto parecía un hash pero no resuelve, se sigue como búsqueda
        // de mensaje.
        if let Ok(commits) = exact
            && !commits.is_empty()
        {
            return Ok(commits);
        }
    }

    let grep_text = if !text.is_empty() {
        text.as_str()
    } else if author.is_empty() {
        hash.as_str()
    } else {
        ""
    };
    if author.is_empty() && grep_text.is_empty() && !filtered {
        return Ok(Vec::new());
    }

    // Una rama que no existe sale con un mensaje claro (git diría "ambiguous
    // argument") y nunca se lee como opción.
    if !branch.is_empty() {
        let output = exec::exec(
            &git,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{branch}^{{commit}}"),
            ],
        )?;
        if output.exit_code != Some(0) {
            return Err(format!("Unknown branch or revision '{branch}'.").into());
        }
    }

    let mut args = vec!["log".to_string()];
    if branch.is_empty() {
        args.push("--all".to_string());
    }
    args.extend([
        "--decorate=full".to_string(),
        "--date=iso-strict".to_string(),
        format!("--max-count={}", max_count(options.max_count)),
        pretty_argument(),
        "--regexp-ignore-case".to_string(),
        "--fixed-strings".to_string(),
    ]);
    if let Some(skip) = options.skip.filter(|skip| *skip > 0) {
        args.push(format!("--skip={skip}"));
    }
    if !author.is_empty() {
        args.push(format!("--author={author}"));
    }
    if !grep_text.is_empty() {
        args.push(format!("--grep={grep_text}"));
    }
    if !since.is_empty() {
        args.push(format!("--since={}", day_bound(&since, "00:00:00")));
    }
    if !until.is_empty() {
        args.push(format!("--until={}", day_bound(&until, "23:59:59")));
    }
    if !code.is_empty() {
        // `-S` cuenta apariciones del texto literal; `-G` busca la regex en
        // las líneas añadidas o quitadas. `--regexp-ignore-case` vale también
        // para los dos.
        args.push(if options.code_regex {
            format!("-G{code}")
        } else {
            format!("-S{code}")
        });
    }
    args.push("--end-of-options".to_string());
    if !branch.is_empty() {
        args.push(branch);
    }
    args.push("--".to_string());
    if !path.is_empty() {
        args.push(path);
    }

    let output = exec::raw(&git, &args)?;
    Ok(parse_commit_log_output(&output)?)
}

/*
 * Una fecha sola (`2024-03-10`) git la lee con la hora actual: "después del
 * 10 de marzo" dejaba fuera lo de esa mañana. Se fija al principio del día
 * (desde) o al final (hasta); lo demás (`7 days ago`, con hora) va tal cual.
 */
fn day_bound(value: &str, time: &str) -> String {
    let is_day = value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if is_day {
        format!("{value} {time}")
    } else {
        value.to_string()
    }
}

/// Canal `git:searchCommits`: busca en todo el historial (o en una rama) por
/// mensaje, autor o hash, con filtros de ruta, fechas y contenido.
pub fn search_commits(repo_path: &Path, options: &CommitSearchOptions) -> QueryResult<Vec<Commit>> {
    read("Failed to search commits", || {
        search_commit_log(repo_path, options)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;
    use mergeon_gitcmd::Git;

    fn log(repo: &TestRepo, max_count: u32) -> Vec<Commit> {
        let options = CommitLogOptions {
            max_count: Some(max_count),
            skip: None,
        };
        get_commits(&repo.path, &options).unwrap()
    }

    #[test]
    fn returns_root_commits_with_empty_parents() {
        let repo = TestRepo::new();
        let root_hash = repo.commit_file("file.txt", "base\n", "root commit");

        let commits = log(&repo, 1);

        assert_eq!(commits[0].full_hash, root_hash);
        assert_eq!(commits[0].parents, "");
    }

    /*
     * Una rama que hace merge de `development` una y otra vez mientras
     * `development` sigue por delante (tiene un commit más nuevo que el último
     * merge). Con `--topo-order` salía primero toda la cadena de merges y los
     * commits de `development` quedaban abajo, cada uno con su carril abierto;
     * por fecha cada uno sale justo después del merge que lo trajo.
     */
    #[test]
    fn shows_each_merged_commit_next_to_its_merge_even_when_the_branch_moved_on() {
        let repo = TestRepo::new();
        let mut time = 1_756_700_000;
        let mut at = |args: &[&str]| {
            time += 60;
            let date = format!("@{time}");
            Git::new(&repo.path)
                .with_env("GIT_AUTHOR_DATE", &date)
                .with_env("GIT_COMMITTER_DATE", &date)
                .raw(args)
                .unwrap();
        };
        at(&["commit", "--allow-empty", "-m", "init"]);
        repo.git(&["branch", "development"]);
        for i in 1..=3 {
            repo.git(&["checkout", "-q", "development"]);
            at(&["commit", "--allow-empty", "-m", &format!("dev {i}")]);
            repo.git(&["checkout", "-q", "main"]);
            at(&[
                "merge",
                "--no-ff",
                "-m",
                &format!("Merge branch 'development' {i}"),
                "development",
            ]);
        }
        repo.git(&["checkout", "-q", "development"]);
        at(&["commit", "--allow-empty", "-m", "dev 4"]);
        repo.git(&["checkout", "-q", "main"]);
        at(&["commit", "--allow-empty", "-m", "main 4"]);

        let messages: Vec<String> = log(&repo, 20)
            .into_iter()
            .map(|commit| commit.message)
            .collect();

        assert_eq!(
            messages,
            [
                "main 4",
                "dev 4",
                "Merge branch 'development' 3",
                "dev 3",
                "Merge branch 'development' 2",
                "dev 2",
                "Merge branch 'development' 1",
                "dev 1",
                "init",
            ]
        );
    }

    /*
     * Sin `--decorate=full` git entrega los nombres cortos y una rama local con
     * barra es indistinguible de una remota. El cliente lo adivinaba y se
     * equivocaba: `feature/login` salía como rama remota.
     */
    #[test]
    fn decorates_refs_with_their_full_path_so_local_and_remote_are_distinguishable() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "root commit");
        repo.git(&["branch", "feature/login"]);
        repo.git(&["tag", "v1.0"]);

        let commits = log(&repo, 1);
        let commit = &commits[0];

        assert!(commit.refs.contains("refs/heads/feature/login"));
        assert!(commit.refs.contains("refs/heads/main"));
        assert!(commit.refs.contains("refs/tags/v1.0"));
    }

    #[test]
    fn returns_a_single_parent_hash_for_normal_commits() {
        let repo = TestRepo::new();
        let base_hash = repo.commit_file("file.txt", "base\n", "root commit");
        let child_hash = repo.commit_file("file.txt", "next\n", "child commit");

        let commits = log(&repo, 2);
        let child = commits
            .iter()
            .find(|commit| commit.full_hash == child_hash)
            .unwrap();

        assert_eq!(child.parents, base_hash);
    }

    #[test]
    fn returns_multiple_parent_hashes_for_merge_commits() {
        let repo = TestRepo::new();
        repo.commit_file("base.txt", "base\n", "root commit");
        repo.git(&["checkout", "-b", "feature"]);
        let feature_hash = repo.commit_file("feature.txt", "feature\n", "feature commit");
        repo.git(&["checkout", "main"]);
        let main_hash = repo.commit_file("main.txt", "main\n", "main commit");
        repo.git(&["merge", "feature", "--no-ff", "-m", "merge feature"]);

        let merge_hash = repo.head();
        let commits = log(&repo, 10);
        let merge = commits
            .iter()
            .find(|commit| commit.full_hash == merge_hash)
            .unwrap();

        assert_eq!(merge.parents, format!("{main_hash} {feature_hash}"));
    }

    #[test]
    fn fills_every_field_and_pages_with_skip() {
        let repo = TestRepo::new();
        let first = repo.commit_file("file.txt", "a\n", "first commit");
        repo.write("file.txt", "b\n");
        repo.git(&[
            "commit",
            "-am",
            "second commit\n\nBody line one\nBody line two",
        ]);
        let second = repo.head();

        let commits = get_commits(&repo.path, &CommitLogOptions::default()).unwrap();
        assert_eq!(commits.len(), 2);
        let newest = &commits[0];
        assert_eq!(newest.full_hash, second);
        assert!(second.starts_with(&newest.hash) && newest.hash.len() < second.len());
        assert_eq!(newest.message, "second commit");
        assert_eq!(newest.author, "Test User");
        assert_eq!(newest.email, "test@example.com");
        assert_eq!(newest.body, "Body line one\nBody line two\n");
        assert!(newest.date.contains('T'), "{}", newest.date);
        assert_eq!(newest.parents, first);

        // Con skip se salta el más reciente; max_count 0 equivale al valor por defecto.
        let options = CommitLogOptions {
            max_count: Some(0),
            skip: Some(1),
        };
        let paged = get_commits(&repo.path, &options).unwrap();
        assert_eq!(paged.len(), 1);
        assert_eq!(paged[0].full_hash, first);
    }

    #[test]
    fn an_empty_repository_has_no_commits() {
        let repo = TestRepo::new();
        assert!(log(&repo, 10).is_empty());
    }

    #[test]
    fn fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let error = get_commits(dir.path(), &CommitLogOptions::default()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to get commits: fatal: not a git repository"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_directory_fails_like_simple_git() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");

        let error = get_commits(&missing, &CommitLogOptions::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to get commits: Cannot use simple-git on a directory that does not exist"
        );

        let error = search_commits(&missing, &CommitSearchOptions::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to search commits: Cannot use simple-git on a directory that does not exist"
        );
    }

    #[test]
    fn rejects_entries_with_an_unexpected_shape() {
        let error = parse_commit_log_output("abc\u{1f}def\u{1e}").unwrap_err();
        assert_eq!(
            error,
            "Unexpected commit log entry shape: expected 9 fields, received 2"
        );
    }

    #[test]
    fn searches_commits_by_message_text_case_insensitively() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "Add login form");
        let fix_hash = repo.commit_file("file.txt", "b\n", "Fix login validation");
        repo.commit_file("file.txt", "c\n", "Update docs");

        let options = CommitSearchOptions {
            text: Some("fix login".into()),
            ..Default::default()
        };
        let results = search_commits(&repo.path, &options).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].full_hash, fix_hash);
    }

    #[test]
    fn searches_commits_by_author() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "first commit");
        repo.git(&["config", "user.name", "Other Person"]);
        let other_hash = repo.commit_file("file.txt", "b\n", "second commit");

        let options = CommitSearchOptions {
            author: Some("other".into()),
            ..Default::default()
        };
        let results = search_commits(&repo.path, &options).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].full_hash, other_hash);
    }

    #[test]
    fn resolves_an_exact_commit_when_the_query_is_a_hash() {
        let repo = TestRepo::new();
        let first_hash = repo.commit_file("file.txt", "a\n", "first commit");
        repo.commit_file("file.txt", "b\n", "second commit");

        let options = CommitSearchOptions {
            hash: Some(first_hash[..8].to_string()),
            ..Default::default()
        };
        let results = search_commits(&repo.path, &options).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].full_hash, first_hash);
    }

    #[test]
    fn falls_back_to_a_message_search_when_the_hash_does_not_resolve() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "first commit");
        let hash = repo.commit_file("file.txt", "b\n", "bump deadbeef dependency");

        let options = CommitSearchOptions {
            hash: Some("deadbeef".into()),
            ..Default::default()
        };
        let results = search_commits(&repo.path, &options).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].full_hash, hash);
    }

    #[test]
    fn an_option_like_hash_is_never_read_as_an_option() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "first commit");
        repo.git(&["checkout", "-b", "other"]);
        repo.commit_file("file.txt", "b\n", "commit on another branch");
        repo.git(&["checkout", "main"]);

        for hash in ["--all", "--branches", "-5", "--format=%H"] {
            let options = CommitSearchOptions {
                hash: Some(hash.to_string()),
                ..Default::default()
            };
            // Sin un commit que se llame así ni un mensaje que lo contenga, nada.
            assert!(
                search_commits(&repo.path, &options).unwrap().is_empty(),
                "{hash}"
            );
        }
    }

    /*
     * "Amend last commit" precarga el commit de HEAD buscándolo como hash.
     * `get_commits` con `max_count: 1` lee `--all` y, con un stash guardado,
     * devuelve el commit del stash: por eso la búsqueda por `HEAD`.
     */
    #[test]
    fn searching_head_returns_the_head_commit_even_with_a_stash() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "first commit");
        let head = repo.commit_file("file.txt", "b\n", "last real commit");
        repo.write("file.txt", "dirty\n");
        repo.git(&["stash", "push", "-m", "saved work"]);

        let newest = log(&repo, 1);
        assert_ne!(newest[0].full_hash, head, "--all puts the stash first");

        let options = CommitSearchOptions {
            max_count: Some(1),
            hash: Some("HEAD".to_string()),
            ..Default::default()
        };
        let results = search_commits(&repo.path, &options).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].full_hash, head);
        assert_eq!(results[0].message, "last real commit");
    }

    #[test]
    fn returns_an_empty_list_when_nothing_matches_or_the_query_is_empty() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "only commit");

        let options = CommitSearchOptions {
            text: Some("nothing like this".into()),
            ..Default::default()
        };
        assert!(search_commits(&repo.path, &options).unwrap().is_empty());
        assert!(
            search_commits(&repo.path, &CommitSearchOptions::default())
                .unwrap()
                .is_empty()
        );
    }

    // --- filtros ---------------------------------------------------------------

    /// Un commit con fecha fija (autor y committer) que escribe `name`.
    fn commit_at(repo: &TestRepo, name: &str, content: &str, message: &str, date: &str) -> String {
        repo.write(name, content);
        repo.git(&["add", "--", name]);
        Git::new(&repo.path)
            .with_env("GIT_AUTHOR_DATE", date)
            .with_env("GIT_COMMITTER_DATE", date)
            .raw(["commit", "-m", message])
            .unwrap();
        repo.head()
    }

    fn messages(repo: &TestRepo, options: CommitSearchOptions) -> Vec<String> {
        search_commits(&repo.path, &options)
            .unwrap()
            .into_iter()
            .map(|commit| commit.message)
            .collect()
    }

    /// main: docs (enero), src con `parse_header` (marzo), docs (junio);
    /// feature (desde marzo): `render_body` en src (abril).
    fn filtered_repo() -> TestRepo {
        let repo = TestRepo::new();
        commit_at(
            &repo,
            "docs/read me.md",
            "intro\n",
            "Write the docs",
            "2024-01-10T12:00:00Z",
        );
        commit_at(
            &repo,
            "src/parser.rs",
            "fn parse_header() {}\n",
            "Add the parser",
            "2024-03-10T12:00:00Z",
        );
        repo.git(&["checkout", "-q", "-b", "feature"]);
        commit_at(
            &repo,
            "src/render.rs",
            "fn render_body() {}\n",
            "Add the renderer",
            "2024-04-10T12:00:00Z",
        );
        repo.git(&["checkout", "-q", "main"]);
        commit_at(
            &repo,
            "docs/read me.md",
            "intro\nmore\n",
            "Expand the docs",
            "2024-06-10T12:00:00Z",
        );
        repo
    }

    #[test]
    fn filters_by_path_alone_or_with_text() {
        let repo = filtered_repo();

        let by_path = |path: &str| CommitSearchOptions {
            path: Some(path.into()),
            ..Default::default()
        };
        assert_eq!(
            messages(&repo, by_path("docs/read me.md")),
            ["Expand the docs", "Write the docs"]
        );
        // Carpeta, separador de Windows y `./` delante.
        assert_eq!(
            messages(&repo, by_path(".\\src")),
            ["Add the renderer", "Add the parser"]
        );
        assert_eq!(
            messages(
                &repo,
                CommitSearchOptions {
                    text: Some("expand".into()),
                    ..by_path("docs")
                }
            ),
            ["Expand the docs"]
        );
        assert!(messages(&repo, by_path("nothing/here")).is_empty());
    }

    #[test]
    fn filters_by_branch_instead_of_every_ref() {
        let repo = filtered_repo();
        let on = |branch: &str| CommitSearchOptions {
            text: Some("add".into()),
            branch: Some(branch.into()),
            ..Default::default()
        };

        assert_eq!(messages(&repo, on("main")), ["Add the parser"]);
        assert_eq!(
            messages(&repo, on("feature")),
            ["Add the renderer", "Add the parser"]
        );
        // Sin rama, todo el historial.
        assert_eq!(
            messages(
                &repo,
                CommitSearchOptions {
                    text: Some("add".into()),
                    ..Default::default()
                }
            ),
            ["Add the renderer", "Add the parser"]
        );

        for branch in ["nope", "--all"] {
            let error = search_commits(&repo.path, &on(branch)).unwrap_err();
            assert_eq!(
                error.message(),
                format!("Failed to search commits: Unknown branch or revision '{branch}'.")
            );
        }
    }

    #[test]
    fn filters_by_date_range() {
        let repo = filtered_repo();
        let between = |since: Option<&str>, until: Option<&str>| CommitSearchOptions {
            since: since.map(Into::into),
            until: until.map(Into::into),
            ..Default::default()
        };

        assert_eq!(
            messages(&repo, between(Some("2024-03-01"), Some("2024-05-01"))),
            ["Add the renderer", "Add the parser"]
        );
        assert_eq!(
            messages(&repo, between(Some("2024-05-01"), None)),
            ["Expand the docs"]
        );
        assert_eq!(
            messages(&repo, between(None, Some("2024-02-01"))),
            ["Write the docs"]
        );
        // Un día solo cuenta entero en los dos extremos (la hora actual no).
        assert_eq!(
            messages(&repo, between(Some("2024-03-10"), Some("2024-03-10"))),
            ["Add the parser"]
        );
        assert_eq!(day_bound("2024-03-10", "00:00:00"), "2024-03-10 00:00:00");
        assert_eq!(day_bound("7 days ago", "00:00:00"), "7 days ago");
        assert_eq!(
            day_bound("2024-03-10T08:00", "00:00:00"),
            "2024-03-10T08:00"
        );
    }

    #[test]
    fn filters_by_changed_code_as_text_or_regex() {
        let repo = filtered_repo();
        let code = |text: &str, regex: bool| CommitSearchOptions {
            code: Some(text.into()),
            code_regex: regex,
            ..Default::default()
        };

        assert_eq!(
            messages(&repo, code("parse_header", false)),
            ["Add the parser"]
        );
        // `-S` es literal: el punto no es comodín.
        assert!(messages(&repo, code("parse.header", false)).is_empty());
        assert_eq!(
            messages(&repo, code("(parse|render)_[a-z]+", true)),
            ["Add the renderer", "Add the parser"]
        );
        // Como el resto de la búsqueda, sin distinguir mayúsculas.
        assert_eq!(
            messages(&repo, code("RENDER_BODY", false)),
            ["Add the renderer"]
        );
    }

    #[test]
    fn combines_every_filter() {
        let repo = filtered_repo();
        repo.git(&["config", "user.name", "Other Person"]);
        commit_at(
            &repo,
            "src/parser.rs",
            "fn parse_header() {}\nfn parse_footer() {}\n",
            "Add the footer parser",
            "2024-07-10T12:00:00Z",
        );

        let options = CommitSearchOptions {
            text: Some("parser".into()),
            author: Some("other".into()),
            path: Some("src".into()),
            branch: Some("main".into()),
            since: Some("2024-07-01".into()),
            until: Some("2024-08-01".into()),
            code: Some("parse_footer".into()),
            ..Default::default()
        };
        assert!(options.has_filters());
        assert_eq!(messages(&repo, options.clone()), ["Add the footer parser"]);

        // Con un filtro que no se cumple, nada.
        let feature_only = CommitSearchOptions {
            branch: Some("feature".into()),
            ..options
        };
        assert!(messages(&repo, feature_only).is_empty());
    }

    // Con filtros, un texto con forma de hash es texto: el atajo del commit
    // exacto ignoraría la ruta.
    #[test]
    fn a_hash_with_filters_does_not_jump_to_that_commit() {
        let repo = filtered_repo();
        let parser = repo.git(&["log", "-1", "--format=%H", "--grep=parser", "--all"]);
        let options = CommitSearchOptions {
            hash: Some(parser.trim()[..8].to_string()),
            path: Some("docs".into()),
            ..Default::default()
        };
        assert!(messages(&repo, options).is_empty());
    }

    /*
     * En un historial grande la búsqueda no lee todo: `max_count` corta la
     * página y `skip` trae la siguiente.
     */
    #[test]
    fn pages_through_a_large_history_without_reading_it_all() {
        let repo = TestRepo::new();
        let total = 450;
        let mut stream = String::new();
        for index in 0..total {
            let message = format!("change {index:03}");
            let content = format!("{index}\n");
            stream.push_str(&format!(
                "commit refs/heads/main\ncommitter T <t@e> {} +0000\ndata {}\n{message}\n",
                1_700_000_000 + index * 60,
                message.len()
            ));
            stream.push_str(&format!(
                "M 644 inline src/file.txt\ndata {}\n{content}\n",
                content.len()
            ));
        }
        repo.git_stdin(&["fast-import", "--quiet"], &stream);
        repo.git(&["reset", "--hard", "-q", "main"]);

        let page = |skip: u32| {
            messages(
                &repo,
                CommitSearchOptions {
                    path: Some("src/file.txt".into()),
                    skip: Some(skip),
                    ..Default::default()
                },
            )
        };
        let first = page(0);
        assert_eq!(first.len(), DEFAULT_MAX_COUNT as usize);
        assert_eq!(first[0], "change 449");
        let second = page(100);
        assert_eq!(second[0], "change 349");
        assert_eq!(page(400).len(), 50);

        let small = messages(
            &repo,
            CommitSearchOptions {
                max_count: Some(5),
                text: Some("change".into()),
                ..Default::default()
            },
        );
        assert_eq!(small.len(), 5);
    }

    #[test]
    fn the_new_options_use_the_wire_names() {
        let options: CommitSearchOptions = serde_json::from_str(
            r#"{"path":"src","branch":"main","since":"2024-01-01","until":"2024-02-01","code":"x","codeRegex":true,"skip":100}"#,
        )
        .unwrap();
        assert!(options.code_regex);
        assert_eq!(options.skip, Some(100));
        assert!(options.has_filters());
        assert!(!CommitSearchOptions::default().has_filters());
        assert!(
            !CommitSearchOptions {
                path: Some("  ".into()),
                ..Default::default()
            }
            .has_filters()
        );
    }

    #[test]
    fn options_and_commits_use_the_wire_names() {
        let options: CommitLogOptions = serde_json::from_str(r#"{"maxCount":25}"#).unwrap();
        assert_eq!(options.max_count, Some(25));
        assert_eq!(options.skip, None);

        let commits = parse_commit_log_output(
            "full\u{1f}short\u{1f}Ann\u{1f}ann@x\u{1f}2024-01-01T00:00:00Z\u{1f}msg\u{1f}\u{1f}\u{1f}\u{1e}\n",
        )
        .unwrap();
        let json = serde_json::to_value(&commits[0]).unwrap();
        assert_eq!(json["hash"], "short");
        assert_eq!(json["fullHash"], "full");
        assert_eq!(json["parents"], "");
    }
}
