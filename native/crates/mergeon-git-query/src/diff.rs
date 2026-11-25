//! Parches para los visores: working tree, commit y la lista de archivos
//! tocados por un commit.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use mergeon_gitcmd::Git;

use crate::error::{Inner, QueryResult, read};
use crate::exec::{self, Capped};
use crate::fs_util::{read_utf8, stat};
use crate::paths::normalize_repository_file_path;
use crate::text::{js_trim, parse_count};

/// Un parche más grande que esto no se manda al visor: congelaría la interfaz.
pub const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

/*
 * Sin `--first-parent`, `git show` de un merge no devuelve nada y el detalle del
 * commit queda en blanco. Con él se ve lo que el merge trajo a la rama, que es
 * lo que se espera al abrirlo. `--cc` no vale: en un merge limpio sale vacío.
 */
const MERGE_DIFF_ARG: &str = "--first-parent";

/// Opciones comunes de los visores de parches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffOptions {
    /// Con `-w` un archivo reindentado deja de salir entero como cambiado.
    pub ignore_whitespace: bool,
    /// El archivo entero como contexto ("Whole file"), no solo las líneas
    /// alrededor de los cambios: git lo da como un único bloque.
    pub whole_file: bool,
}

/// Más contexto que líneas tiene cualquier archivo: con esto git saca el
/// archivo entero en un bloque (lo mismo que hace gitcraft).
pub const WHOLE_FILE_CONTEXT_ARG: &str = "--unified=999999999";

impl DiffOptions {
    /// Las opciones de `git diff` / `git show` que piden estas preferencias.
    pub fn push_args(&self, args: &mut Vec<&str>) {
        if self.ignore_whitespace {
            args.push("-w");
        }
        if self.whole_file {
            args.push(WHOLE_FILE_CONTEXT_ARG);
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Contrato común de los visores de parches: nunca un string gigante crudo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffPayload {
    /// Vacío cuando `too_large`.
    pub diff: String,
    /// Bytes del parche (o del archivo, si es untracked). Con `too_large` en
    /// un parche de git es un mínimo: la lectura se corta al pasar
    /// [`MAX_DIFF_BYTES`] sin esperar al resto.
    pub size: u64,
    pub too_large: bool,
    /// Se pidió "Whole file", pero el parche entero superaba [`MAX_DIFF_BYTES`]
    /// y se volvió al diff por bloques normal para no dejar al usuario en blanco.
    #[serde(default, skip_serializing_if = "is_false")]
    pub whole_file_fell_back: bool,
}

pub(crate) fn build_diff_payload(diff: String) -> DiffPayload {
    let size = diff.len() as u64;
    if size > MAX_DIFF_BYTES {
        return DiffPayload {
            diff: String::new(),
            size,
            too_large: true,
            whole_file_fell_back: false,
        };
    }
    DiffPayload {
        diff,
        size,
        too_large: false,
        whole_file_fell_back: false,
    }
}

/*
 * Parche de git para un visor. La salida se lee con tope: un parche de más de
 * MAX_DIFF_BYTES no llega a cargarse entero (antes se materializaba todo y se
 * medía después) y git se mata en cuanto lo pasa.
 *
 * Si se había pedido "Whole file" (`WHOLE_FILE_CONTEXT_ARG`) y el archivo
 * entero pasa del tope, se reintenta una vez con el contexto normal para que
 * un cambio pequeño en un archivo grande siga siendo visible.
 */
pub(crate) fn git_diff_payload<S: AsRef<str>>(git: &Git, args: &[S]) -> Inner<DiffPayload> {
    let read_capped = |cmd_args: &[&str]| -> Inner<DiffPayload> {
        Ok(match exec::raw_capped(git, cmd_args, MAX_DIFF_BYTES)? {
            Capped::Complete(diff) => build_diff_payload(diff),
            Capped::TooLarge(size) => DiffPayload {
                diff: String::new(),
                size,
                too_large: true,
                whole_file_fell_back: false,
            },
        })
    };

    let raw_args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let payload = read_capped(&raw_args)?;
    if payload.too_large && raw_args.contains(&WHOLE_FILE_CONTEXT_ARG) {
        let reduced: Vec<&str> = raw_args
            .into_iter()
            .filter(|arg| *arg != WHOLE_FILE_CONTEXT_ARG)
            .collect();
        let mut fallback = read_capped(&reduced)?;
        if !fallback.too_large {
            fallback.whole_file_fell_back = true;
        }
        return Ok(fallback);
    }
    Ok(payload)
}

fn build_untracked_diff(repo_path: &Path, file_path: &str) -> Inner<DiffPayload> {
    let absolute_path = repo_path.join(file_path);

    // Medir antes de leer: un untracked gigante no debe cargarse entero.
    let size = stat(&absolute_path)?.len();
    if size > MAX_DIFF_BYTES {
        return Ok(DiffPayload {
            diff: String::new(),
            size,
            too_large: true,
            whole_file_fell_back: false,
        });
    }

    let content = read_utf8(&absolute_path)?;
    let lines: Vec<&str> = if content.is_empty() {
        Vec::new()
    } else {
        content.split('\n').collect()
    };
    let additions = lines
        .iter()
        .map(|line| format!("+{line}"))
        .collect::<Vec<_>>()
        .join("\n");

    let diff = [
        format!("diff --git a/{file_path} b/{file_path}"),
        "new file mode 100644".to_string(),
        "--- /dev/null".to_string(),
        format!("+++ b/{file_path}"),
        format!("@@ -0,0 +1,{} @@", lines.len()),
        additions,
    ]
    .join("\n");

    Ok(DiffPayload {
        diff,
        size,
        too_large: false,
        whole_file_fell_back: false,
    })
}

/// Canal `git:getDiff`: parche de un archivo del working tree (`staged`: del
/// índice; `is_untracked`: se fabrica un parche de archivo nuevo).
pub fn get_diff(
    repo_path: &Path,
    file_path: &str,
    staged: bool,
    is_untracked: bool,
    options: &DiffOptions,
) -> QueryResult<DiffPayload> {
    read("Failed to get diff", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;

        if is_untracked {
            return build_untracked_diff(repo_path, &normalized_path);
        }

        // Si es staged, usar --cached, sino diff normal
        let mut args = vec!["diff"];
        if staged {
            args.push("--cached");
        }
        options.push_args(&mut args);
        args.extend(["--", &normalized_path]);

        git_diff_payload(&git, &args)
    })
}

/// Canal `git:getCommitDiff`: parche de un archivo en un commit.
pub fn get_commit_diff(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
    options: &DiffOptions,
) -> QueryResult<DiffPayload> {
    read("Failed to get commit diff", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;
        // `--format=` deja fuera la cabecera del commit: autor, fecha y mensaje ya
        // se ven en la pestaña Commit, y mezclados aquí el visor los numeraba como
        // si fueran líneas del archivo.
        let mut args = vec!["show", "--format=", MERGE_DIFF_ARG, "-M"];
        options.push_args(&mut args);
        args.extend([commit_hash, "--", &normalized_path]);
        let payload = git_diff_payload(&git, &args)?;

        // Con solo la ruta nueva en el pathspec git no ve el renombre y lo
        // pinta como un archivo nuevo entero (QA 0.12.0): si el commit lo
        // renombró, se pide otra vez con las dos rutas.
        if !payload.diff.contains("\nnew file mode ") {
            return Ok(payload);
        }
        let Some(source) = rename_source(&git, commit_hash, &normalized_path)? else {
            return Ok(payload);
        };
        args.push(&source);
        git_diff_payload(&git, &args)
    })
}

/// La ruta de origen si `commit` renombró (o copió) algo a `path`.
pub(crate) fn rename_source(git: &Git, commit_hash: &str, path: &str) -> Inner<Option<String>> {
    let output = exec::raw(
        git,
        &[
            "show",
            "--format=",
            MERGE_DIFF_ARG,
            "-M",
            "--name-status",
            "-z",
            commit_hash,
        ],
    )?;
    let mut fields = output.split('\0');
    while let Some(code) = fields.next() {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        if code.starts_with('R') || code.starts_with('C') {
            let (Some(source), Some(destination)) = (fields.next(), fields.next()) else {
                break;
            };
            if destination == path {
                return Ok(Some(source.to_string()));
            }
        } else {
            fields.next();
        }
    }
    Ok(None)
}

/// Un archivo tocado por un commit o un stash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// En renombres y copias, la ruta de destino.
    pub path: String,
    /// Primera letra del estado de `--name-status` (`A`, `M`, `D`, `R`, `C`...).
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
}

/*
 * Combina la salida de `--name-status -z` y `--numstat -z` en la lista de
 * archivos que consume el visor de cambios. Compartido entre el detalle de un
 * commit y el de un stash.
 *
 * La app Electron leía la salida por líneas, sin `-z`, y tenía dos defectos que
 * aquí se corrigen: numstat describe un renombre o una copia como
 * `viejo => nuevo` (o `dir/{viejo => nuevo}`), que no coincidía con la ruta de
 * destino y dejaba el archivo en +0/−0; y `core.quotePath` entrecomillaba las
 * rutas no ASCII (`"a\303\261o.txt"`). Con `-z` cada ruta va en su propio
 * campo, literal, y un renombre trae sus dos rutas por separado.
 */
pub(crate) fn parse_name_status_with_stats(
    status_output: &str,
    stats_output: &str,
) -> Vec<FileChange> {
    // numstat: `<+>\t<->\t<ruta>\0`; en renombres y copias la ruta va vacía y
    // le siguen dos campos, `<origen>\0<destino>\0`.
    let mut stats: HashMap<&str, (&str, &str)> = HashMap::new();
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
        stats.insert(path, (js_trim(added), js_trim(deleted)));
    }

    // name-status: `<estado>\0<ruta>\0`, o `<estado>\0<origen>\0<destino>\0`
    // para R y C.
    let mut changes = Vec::new();
    let mut fields = status_output.split('\0');
    while let Some(field) = fields.next() {
        let code = js_trim(field);
        if code.is_empty() {
            continue;
        }
        let status: String = code.chars().take(1).collect();
        let path = if status == "R" || status == "C" {
            let _source = fields.next();
            fields.next()
        } else {
            fields.next()
        };
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            continue;
        };

        // Los binarios traen `-` en numstat: cuentan como 0.
        let (added, deleted) = stats.get(path).copied().unwrap_or(("0", "0"));

        changes.push(FileChange {
            path: path.to_string(),
            status,
            additions: parse_count(added),
            deletions: parse_count(deleted),
        });
    }
    changes
}

/// Tope de bytes para el listado de archivos de un commit (16 MiB), para evitar
/// agotar memoria en commits masivos (imports o directorios vendor).
const MAX_COMMIT_FILES_BYTES: usize = 16 * 1024 * 1024;

fn truncate_at_last_nul(bytes: &[u8]) -> String {
    let slice = match bytes.iter().rposition(|&byte| byte == 0) {
        Some(pos) => &bytes[..=pos],
        None => bytes,
    };
    String::from_utf8_lossy(slice).into_owned()
}

/// Canal `git:getCommitFiles`: archivos tocados por un commit.
pub fn get_commit_files(repo_path: &Path, commit_hash: &str) -> QueryResult<Vec<FileChange>> {
    read("Failed to get commit files", || {
        let git = exec::open(repo_path)?;

        let status_args = [
            "show",
            "--name-status",
            "-z",
            "--pretty=format:",
            MERGE_DIFF_ARG,
            commit_hash,
        ];
        let capped_status = git.exec_capped(status_args, MAX_COMMIT_FILES_BYTES)?;
        if !capped_status.exceeded && capped_status.output.is_task_error() {
            return Err(mergeon_gitcmd::GitError::from_output(&capped_status.output).into());
        }
        let status_output = if capped_status.exceeded {
            truncate_at_last_nul(&capped_status.output.stdout)
        } else {
            capped_status.output.stdout_text()
        };

        let stats_args = [
            "show",
            "--numstat",
            "-z",
            "--pretty=format:",
            MERGE_DIFF_ARG,
            commit_hash,
        ];
        let capped_stats = git.exec_capped(stats_args, MAX_COMMIT_FILES_BYTES)?;
        if !capped_stats.exceeded && capped_stats.output.is_task_error() {
            return Err(mergeon_gitcmd::GitError::from_output(&capped_stats.output).into());
        }
        let stats_output = if capped_stats.exceeded {
            truncate_at_last_nul(&capped_stats.output.stdout)
        } else {
            capped_stats.output.stdout_text()
        };

        Ok(parse_name_status_with_stats(&status_output, &stats_output))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn change(path: &str, status: &str, additions: u64, deletions: u64) -> FileChange {
        FileChange {
            path: path.into(),
            status: status.into(),
            additions,
            deletions,
        }
    }

    #[test]
    fn a_renamed_file_shows_as_a_rename_in_its_commit_diff() {
        let repo = TestRepo::new();
        repo.commit_file("src/app.js", "one\ntwo\nthree\n", "Add app");
        repo.git(&["mv", "src/app.js", "src/entry.js"]);
        repo.git(&["commit", "-qm", "Rename app"]);

        let payload = get_commit_diff(
            &repo.path,
            &repo.head(),
            "src/entry.js",
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(
            payload.diff.contains("rename from src/app.js"),
            "{}",
            payload.diff
        );
        assert!(
            payload.diff.contains("rename to src/entry.js"),
            "{}",
            payload.diff
        );
        assert!(!payload.diff.contains("new file mode"), "{}", payload.diff);

        // Un archivo nuevo de verdad sigue siendo nuevo.
        let added = repo.commit_file("docs/new.md", "hi\n", "Add docs");
        let payload =
            get_commit_diff(&repo.path, &added, "docs/new.md", &DiffOptions::default()).unwrap();
        assert!(payload.diff.contains("new file mode"), "{}", payload.diff);
    }

    #[test]
    fn combines_name_status_and_numstat() {
        let status = "M\0src/app.js\0A\0docs/new file.md\0D\0old.txt\0M\0image.png\0";
        let stats =
            "3\t1\tsrc/app.js\x0010\t0\tdocs/new file.md\x000\t7\told.txt\x00-\t-\timage.png\0";

        assert_eq!(
            parse_name_status_with_stats(status, stats),
            [
                change("src/app.js", "M", 3, 1),
                change("docs/new file.md", "A", 10, 0),
                change("old.txt", "D", 0, 7),
                change("image.png", "M", 0, 0),
            ]
        );
    }

    // El original leía numstat por líneas: un renombre salía como
    // `viejo => nuevo`, no casaba con la ruta de destino y quedaba en 0/0.
    #[test]
    fn a_rename_reports_its_destination_path_with_its_real_stats() {
        let status = "R087\0old name.txt\0new name.txt\0C100\0a.txt\0b.txt\0";
        let stats = "2\t1\t\0old name.txt\0new name.txt\x000\t0\t\0a.txt\0b.txt\0";

        assert_eq!(
            parse_name_status_with_stats(status, stats),
            [
                change("new name.txt", "R", 2, 1),
                change("b.txt", "C", 0, 0)
            ]
        );
    }

    #[test]
    fn files_without_stats_count_as_zero() {
        assert_eq!(
            parse_name_status_with_stats("M\0a.txt\0", ""),
            [change("a.txt", "M", 0, 0)]
        );
        assert!(parse_name_status_with_stats("", "").is_empty());
        // Una tabulación es parte de la ruta, no un separador.
        assert_eq!(
            parse_name_status_with_stats("A\0tab\there.txt\0", "1\t0\ttab\there.txt\0"),
            [change("tab\there.txt", "A", 1, 0)]
        );
    }

    #[test]
    fn renames_and_unicode_paths_keep_their_stats_with_real_git() {
        let repo = TestRepo::new();
        repo.write("old name.txt", "l1\nl2\nl3\nl4\nl5\nl6\n");
        repo.write("dir/inner.txt", "one\ntwo\nthree\nfour\n");
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "base"]);

        repo.git(&["mv", "old name.txt", "nuevo ñandú.txt"]);
        repo.write("nuevo ñandú.txt", "l1\nl2\nl3\nl4\nl5\nchanged\n");
        repo.git(&["mv", "dir", "carpeta"]);
        repo.write("año.txt", "uno\ndos\n");
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "rename"]);

        let mut files = get_commit_files(&repo.path, &repo.head()).unwrap();
        files.sort_by(|left, right| left.path.cmp(&right.path));

        assert_eq!(
            files,
            [
                change("año.txt", "A", 2, 0),
                change("carpeta/inner.txt", "R", 0, 0),
                change("nuevo ñandú.txt", "R", 1, 1),
            ]
        );
    }

    #[test]
    fn lists_the_files_of_a_commit() {
        let repo = TestRepo::new();
        repo.write("keep.txt", "one\ntwo\n");
        repo.write("gone.txt", "bye\n");
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "base"]);

        repo.write("keep.txt", "one\nthree\nfour\n");
        repo.write("added.txt", "new\n");
        repo.git(&["rm", "-q", "gone.txt"]);
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "change"]);

        let mut files = get_commit_files(&repo.path, &repo.head()).unwrap();
        files.sort_by(|left, right| left.path.cmp(&right.path));

        assert_eq!(
            files,
            [
                change("added.txt", "A", 1, 0),
                change("gone.txt", "D", 0, 1),
                change("keep.txt", "M", 2, 1),
            ]
        );
    }

    #[test]
    fn a_merge_commit_shows_what_it_brought_to_the_branch() {
        let repo = TestRepo::new();
        repo.commit_file("base.txt", "base\n", "base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.commit_file("feature.txt", "feature\n", "feature");
        repo.git(&["checkout", "main"]);
        repo.commit_file("main.txt", "main\n", "main");
        repo.git(&["merge", "feature", "--no-ff", "-m", "merge feature"]);
        let merge = repo.head();

        assert_eq!(
            get_commit_files(&repo.path, &merge).unwrap(),
            [change("feature.txt", "A", 1, 0)]
        );

        let payload =
            get_commit_diff(&repo.path, &merge, "feature.txt", &DiffOptions::default()).unwrap();
        assert!(payload.diff.contains("+feature"), "{}", payload.diff);
        // Sin la cabecera del commit: el visor solo quiere el parche.
        assert!(payload.diff.starts_with("diff --git"), "{}", payload.diff);
        assert!(!payload.too_large);
        assert_eq!(payload.size, payload.diff.len() as u64);
    }

    #[test]
    fn ignoring_whitespace_hides_reindented_lines() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "one\ntwo\n", "base");
        let hash = repo.commit_file("file.txt", "    one\ntwo\nthree\n", "reindent");

        let plain =
            get_commit_diff(&repo.path, &hash, "file.txt", &DiffOptions::default()).unwrap();
        assert!(plain.diff.contains("+    one"));

        let options = DiffOptions {
            ignore_whitespace: true,
            ..DiffOptions::default()
        };
        let ignoring = get_commit_diff(&repo.path, &hash, "file.txt", &options).unwrap();
        assert!(!ignoring.diff.contains("+    one"), "{}", ignoring.diff);
        assert!(ignoring.diff.contains("+three"));

        repo.write("file.txt", "one\ntwo\nthree\n");
        let working = get_diff(&repo.path, "file.txt", false, false, &options).unwrap();
        assert_eq!(working.diff, "");
        assert_eq!(working.size, 0);
    }

    // "Whole file": el archivo entero en un solo bloque, con las líneas que
    // no cambiaron como contexto (commit, working tree e índice).
    #[test]
    fn whole_file_brings_every_line_as_context() {
        let repo = TestRepo::new();
        let base: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        repo.commit_file("file.txt", &base, "base");
        let hash = repo.commit_file(
            "file.txt",
            &base.replace("line 20\n", "line twenty\n"),
            "edit",
        );

        let plain =
            get_commit_diff(&repo.path, &hash, "file.txt", &DiffOptions::default()).unwrap();
        assert!(!plain.diff.contains(" line 1\n"), "{}", plain.diff);

        let whole = DiffOptions {
            whole_file: true,
            ..DiffOptions::default()
        };
        let full = get_commit_diff(&repo.path, &hash, "file.txt", &whole).unwrap();
        assert!(full.diff.contains("@@ -1,40 +1,40 @@"), "{}", full.diff);
        assert!(full.diff.contains(" line 1\n"));
        assert!(full.diff.contains(" line 40\n"));
        assert!(full.diff.contains("-line 20\n+line twenty\n"));
        assert_eq!(full.diff.matches("\n@@ ").count(), 1);

        repo.write("file.txt", base.replace("line 3\n", "line three\n"));
        let working = get_diff(&repo.path, "file.txt", false, false, &whole).unwrap();
        assert!(working.diff.contains(" line 40\n"), "{}", working.diff);
        assert!(working.diff.contains("+line three\n"));
        repo.git(&["add", "file.txt"]);
        let staged = get_diff(&repo.path, "file.txt", true, false, &whole).unwrap();
        assert!(staged.diff.contains(" line 1\n"), "{}", staged.diff);
        assert!(staged.diff.contains("-line 3\n+line three\n"));
        assert!(!full.whole_file_fell_back);
        assert!(!working.whole_file_fell_back);
        assert!(!staged.whole_file_fell_back);
    }

    #[test]
    fn whole_file_falls_back_to_hunks_when_the_full_file_exceeds_the_diff_limit() {
        let repo = TestRepo::new();
        let line = format!("{}\n", "a".repeat(99)); // 100 bytes/line
        let base = line.repeat(24_000); // ~2.4 MB (> MAX_DIFF_BYTES)
        repo.commit_file("huge.txt", &base, "base");
        let mut edited = base.clone();
        edited.replace_range(1_200_000..1_200_005, "BBBBB");
        let hash = repo.commit_file("huge.txt", &edited, "one-line edit");

        let whole = DiffOptions {
            whole_file: true,
            ..DiffOptions::default()
        };
        let payload = get_commit_diff(&repo.path, &hash, "huge.txt", &whole).unwrap();
        assert!(!payload.too_large);
        assert!(payload.whole_file_fell_back);
        assert!(payload.diff.contains("+BBBBB"));
    }

    #[test]
    fn staged_and_unstaged_changes_are_separate_diffs() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.write("file.txt", "staged\n");
        repo.git(&["add", "file.txt"]);
        repo.write("file.txt", "unstaged\n");

        let options = DiffOptions::default();
        let staged = get_diff(&repo.path, "file.txt", true, false, &options).unwrap();
        assert!(staged.diff.contains("+staged") && staged.diff.contains("-base"));

        let unstaged = get_diff(&repo.path, "file.txt", false, false, &options).unwrap();
        assert!(unstaged.diff.contains("+unstaged") && unstaged.diff.contains("-staged"));
    }

    #[test]
    fn an_untracked_file_becomes_a_new_file_patch() {
        let repo = TestRepo::new();
        repo.write("notes/new.txt", "one\ntwo\n");

        let payload = get_diff(
            &repo.path,
            ".\\notes\\new.txt",
            false,
            true,
            &DiffOptions::default(),
        )
        .unwrap();

        assert_eq!(
            payload.diff,
            [
                "diff --git a/notes/new.txt b/notes/new.txt",
                "new file mode 100644",
                "--- /dev/null",
                "+++ b/notes/new.txt",
                "@@ -0,0 +1,3 @@",
                "+one",
                "+two",
                "+",
            ]
            .join("\n")
        );
        assert_eq!(payload.size, 8);
        assert!(!payload.too_large);

        repo.write("empty.txt", "");
        let empty = get_diff(
            &repo.path,
            "empty.txt",
            false,
            true,
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(
            empty.diff.ends_with("@@ -0,0 +1,0 @@\n"),
            "{:?}",
            empty.diff
        );
    }

    // Límites de diff con git de verdad: un archivo por encima del cap no debe
    // generar salida gigante, sino un payload con `tooLarge`.
    #[test]
    fn caps_working_tree_diffs_and_untracked_files_above_the_diff_limit() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        let options = DiffOptions::default();

        repo.write("file.txt", "changed\n");
        let normal = get_diff(&repo.path, "file.txt", false, false, &options).unwrap();
        assert!(!normal.too_large);
        assert!(normal.diff.contains("+changed"));

        // Cambio tracked gigante (>2 MB de parche): la lectura se corta al
        // pasar el tope, sin esperar a los ~3 MB.
        repo.write("file.txt", format!("{}\n", "y".repeat(80)).repeat(40000));
        let oversized = get_diff(&repo.path, "file.txt", false, false, &options).unwrap();
        assert!(oversized.too_large);
        assert_eq!(oversized.diff, "");
        assert!(oversized.size > MAX_DIFF_BYTES);
        assert!(oversized.size <= MAX_DIFF_BYTES + 64 * 1024);

        // Untracked gigante: ni siquiera se lee el archivo entero
        repo.write("huge-untracked.txt", "z".repeat(2 * 1024 * 1024 + 10));
        let untracked = get_diff(&repo.path, "huge-untracked.txt", false, true, &options).unwrap();
        assert!(untracked.too_large);
        assert_eq!(untracked.diff, "");
        assert_eq!(untracked.size, 2 * 1024 * 1024 + 10);
    }

    // Un archivo de 10 MB cambiado entero: ni el parche del working tree, ni
    // el del índice, ni el del commit se cargan; justo por debajo del tope sí.
    #[test]
    fn a_ten_megabyte_change_is_too_large_everywhere_without_reading_it_all() {
        let repo = TestRepo::new();
        let line = |fill: &str| format!("{}\n", fill.repeat(79));
        let ten_mb = 10 * 1024 * 1024 / 80;
        repo.commit_file("big.txt", &line("a").repeat(ten_mb), "base");
        let hash = repo.commit_file("big.txt", &line("b").repeat(ten_mb), "rewrite");
        repo.write("big.txt", line("c").repeat(ten_mb));
        let options = DiffOptions::default();

        let payloads = [
            get_diff(&repo.path, "big.txt", false, false, &options).unwrap(),
            get_commit_diff(&repo.path, &hash, "big.txt", &options).unwrap(),
        ];
        repo.git(&["add", "big.txt"]);
        let staged = get_diff(&repo.path, "big.txt", true, false, &options).unwrap();
        for payload in payloads.iter().chain([&staged]) {
            assert!(payload.too_large);
            assert!(payload.diff.is_empty());
            assert!(payload.size > MAX_DIFF_BYTES);
            assert!(
                payload.size <= MAX_DIFF_BYTES + 64 * 1024,
                "{}",
                payload.size
            );
        }

        // Un parche que cabe se lee entero, aunque el archivo sea grande.
        repo.git(&["commit", "-q", "-m", "c"]);
        let mut content = line("c").repeat(ten_mb);
        content.replace_range(0..1, "d");
        repo.write("big.txt", content);
        let small = get_diff(&repo.path, "big.txt", false, false, &options).unwrap();
        assert!(!small.too_large);
        assert!(small.diff.contains("+dcc"), "{}", small.diff);
        assert_eq!(small.size, small.diff.len() as u64);
    }

    #[test]
    fn rejects_paths_outside_the_repository() {
        let repo = TestRepo::new();
        let options = DiffOptions::default();

        let error = get_diff(&repo.path, "../secret.txt", false, false, &options).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to get diff: The file path must stay inside the repository."
        );

        let error = get_diff(&repo.path, "/etc/passwd", false, true, &options).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to get diff: The file path must be relative to the repository."
        );

        let error = get_commit_diff(&repo.path, "HEAD", "..", &options).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to get commit diff: The file path must stay inside the repository."
        );
    }

    // El original crea el git antes de validar nada: con el repo borrado gana
    // ese error, no el de la ruta.
    #[test]
    fn a_missing_repository_is_reported_before_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");

        let error = get_diff(&missing, "../x", false, true, &DiffOptions::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to get diff: Cannot use simple-git on a directory that does not exist"
        );
    }

    #[test]
    fn a_missing_untracked_file_fails_like_node() {
        let repo = TestRepo::new();
        let error =
            get_diff(&repo.path, "gone.txt", false, true, &DiffOptions::default()).unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "Failed to get diff: ENOENT: no such file or directory, stat '{}'",
                repo.path.join("gone.txt").display()
            )
        );
    }

    #[test]
    fn an_unknown_commit_fails_with_the_read_prefix() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");

        let error = get_commit_files(&repo.path, "0000000").unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to get commit files: fatal:"),
            "{error}"
        );
    }

    #[test]
    fn payloads_use_the_wire_names() {
        let json = serde_json::to_value(build_diff_payload("x".into())).unwrap();
        assert_eq!(json["tooLarge"], false);
        assert_eq!(json["size"], 1);
        let options: DiffOptions = serde_json::from_str(r#"{"ignoreWhitespace":true}"#).unwrap();
        assert!(options.ignore_whitespace);
        assert!(!options.whole_file);
        let options: DiffOptions = serde_json::from_str(r#"{"wholeFile":true}"#).unwrap();
        assert!(options.whole_file);
    }
}
