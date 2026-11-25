//! Las tres versiones de un archivo en conflicto, leídas del índice.
//!
//! Durante un merge (o rebase, cherry-pick, revert) git deja en el índice, para
//! cada archivo en conflicto, hasta tres etapas: `:1:` el ancestro común
//! (base), `:2:` lo que hay en HEAD (ours) y `:3:` lo que llega (theirs). Cuál
//! falta dice qué clase de conflicto es: sin base, añadido por los dos; sin
//! `:2:` o sin `:3:`, borrado por uno y modificado por el otro.
//!
//! El resolvedor parte de estas etapas y no de los marcadores del working tree,
//! que faltan con `merge.conflictStyle=merge` (no llevan la base) y se
//! confunden si el archivo ya tenía líneas que parecen marcadores. Si no hay
//! etapas (el archivo ya se preparó, o el conflicto es de otra herramienta),
//! queda el archivo del working tree para leer sus marcadores.

use crate::error::{Inner, QueryResult, read};
use crate::exec::open;
use crate::files::{MAX_READ_FILE_BYTES, read_capped_utf8_optional};
use crate::paths::{assert_path_inside_repository, normalize_repo_relative_path};

/// Lo que git mira para decidir si un blob es binario: un NUL en los primeros
/// 8000 bytes (`buffer_is_binary`).
const BINARY_SNIFF_BYTES: usize = 8000;

/// Una etapa del índice (`:1:`, `:2:` o `:3:`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictStage {
    /// Hash del blob.
    pub oid: String,
    /// Modo del archivo (`100644`, `100755`, `120000`…).
    pub mode: String,
    /// Tamaño en bytes.
    pub size: u64,
    /// Lleva un NUL al principio, como entiende git un binario. Un binario no
    /// trae texto.
    pub binary: bool,
    /// Contenido (UTF-8; lo inválido se sustituye). Vacío si es binario.
    pub text: String,
}

/// Las etapas de un archivo en conflicto y, si hace falta, su contenido en el
/// working tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictVersions {
    /// Ruta relativa al repositorio, con `/`.
    pub path: String,
    /// `:1:`, el ancestro común. Falta si los dos lados lo añadieron.
    pub base: Option<ConflictStage>,
    /// `:2:`, lo actual (HEAD). Falta si HEAD lo borró.
    pub ours: Option<ConflictStage>,
    /// `:3:`, lo entrante. Falta si lo borró lo que llega.
    pub theirs: Option<ConflictStage>,
    /// El archivo en el working tree, con sus marcadores, si existe y es
    /// texto. Es de donde lee el resolvedor cuando no hay etapas.
    pub working: Option<String>,
    /// Quién es cada lado, para explicar qué hizo cada uno ("main",
    /// "feature", "commit 1a2b3c4"). Solo se leen cuando el archivo se
    /// resuelve entero (binario, o le falta algún lado); `None` si no se
    /// sabe (un `stash apply`, por ejemplo).
    pub sides: ConflictSides,
}

/// Los nombres de los dos lados de la operación en curso.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictSides {
    /// `:2:`: la rama actual en un merge, cherry-pick o revert; en un rebase,
    /// aquello sobre lo que se rebasa.
    pub ours: Option<String>,
    /// `:3:`: lo que llega (la rama que se fusiona, el commit que se aplica).
    pub theirs: Option<String>,
}

/// Qué clase de conflicto es, por las etapas que hay (las letras de
/// `git status --short`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// `UU`: los dos lados lo modificaron.
    BothModified,
    /// `AA`: los dos lados lo añadieron (no hay base).
    BothAdded,
    /// `DU`: HEAD lo borró y lo que llega lo modificó.
    DeletedByUs,
    /// `UD`: HEAD lo modificó y lo que llega lo borró.
    DeletedByThem,
    /// `AU`: solo está en HEAD (sin base ni lado entrante; renombrados).
    AddedByUs,
    /// `UA`: solo está en lo que llega.
    AddedByThem,
    /// `DD`: solo queda la base (los dos lo borraron o renombraron).
    BothDeleted,
    /// Sin etapas en el índice: solo queda el archivo con marcadores.
    Unstaged,
}

impl ConflictVersions {
    /// Hay al menos una etapa en el índice.
    pub fn has_stages(&self) -> bool {
        self.base.is_some() || self.ours.is_some() || self.theirs.is_some()
    }

    /// Alguna de las etapas es binaria.
    pub fn is_binary(&self) -> bool {
        [&self.base, &self.ours, &self.theirs]
            .into_iter()
            .flatten()
            .any(|stage| stage.binary)
    }

    pub fn kind(&self) -> ConflictKind {
        match (
            self.base.is_some(),
            self.ours.is_some(),
            self.theirs.is_some(),
        ) {
            (_, true, true) if self.base.is_none() => ConflictKind::BothAdded,
            (_, true, true) => ConflictKind::BothModified,
            (true, false, true) => ConflictKind::DeletedByUs,
            (true, true, false) => ConflictKind::DeletedByThem,
            (false, true, false) => ConflictKind::AddedByUs,
            (false, false, true) => ConflictKind::AddedByThem,
            (true, false, false) => ConflictKind::BothDeleted,
            (false, false, false) => ConflictKind::Unstaged,
        }
    }
}

/// Una línea de `git ls-files -u -z`: `<modo> <hash> <etapa>\t<ruta>`.
fn parse_unmerged_entry(entry: &str) -> Option<(String, String, u8, &str)> {
    let (meta, path) = entry.split_once('\t')?;
    let mut fields = meta.split(' ');
    let mode = fields.next()?.to_string();
    let oid = fields.next()?.to_string();
    let stage = fields.next()?.parse().ok()?;
    Some((mode, oid, stage, path))
}

fn too_large(size: u64) -> String {
    format!(
        "The file is too large to open here ({:.1} MB; the limit is {} MB).",
        size as f64 / (1024.0 * 1024.0),
        MAX_READ_FILE_BYTES / (1024 * 1024)
    )
}

/// Lee el blob `oid` sin cargar más del tope. Un binario puede pasar del tope
/// (no se enseña, solo se elige entero); un texto no.
fn read_stage(git: &mergeon_gitcmd::Git, mode: String, oid: String) -> Inner<ConflictStage> {
    let limit = usize::try_from(MAX_READ_FILE_BYTES).unwrap_or(usize::MAX);
    let capped = git.exec_capped(["cat-file", "blob", oid.as_str()], limit)?;
    let bytes = &capped.output.stdout;
    let binary = bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0);
    if capped.exceeded {
        let size = git
            .raw(["cat-file", "-s", oid.as_str()])?
            .trim()
            .parse()
            .unwrap_or(bytes.len() as u64);
        if !binary {
            return Err(too_large(size).into());
        }
        return Ok(ConflictStage {
            oid,
            mode,
            size,
            binary,
            text: String::new(),
        });
    }
    if capped.output.is_task_error() {
        return Err(mergeon_gitcmd::GitError::from_output(&capped.output).into());
    }
    Ok(ConflictStage {
        oid,
        mode,
        size: bytes.len() as u64,
        binary,
        text: if binary {
            String::new()
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        },
    })
}

/// Los primeros caracteres de un hash guardado en un archivo del gitdir.
fn short_hash(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?;
    (hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash[..7].to_string())
}

/// El nombre entre las primeras comillas simples de `MERGE_MSG` ("Merge
/// branch 'feature'", "Merge remote-tracking branch 'origin/x'", "Merge tag
/// 'v1'").
fn merge_message_name(message: &str) -> Option<String> {
    let first = message.lines().next()?;
    let (_, rest) = first.split_once('\'')?;
    let (name, _) = rest.split_once('\'')?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Quién es cada lado de la operación en curso, leído de los archivos que
/// deja git en el gitdir (lecturas pequeñas, como `detect_operation_state`)
/// y de un `rev-parse` para la rama actual. Lo que no se sepa queda en `None`.
fn conflict_sides(git: &mergeon_gitcmd::Git, repo_path: &std::path::Path) -> ConflictSides {
    let Ok(git_dir) = mergeon_gitcmd::git_dir_path(repo_path) else {
        return ConflictSides::default();
    };
    let read = |name: &str| std::fs::read_to_string(git_dir.join(name)).ok();
    let current_branch = || {
        git.raw(["rev-parse", "--abbrev-ref", "HEAD"])
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty() && name != "HEAD")
    };

    // Rebase: `:2:` es aquello sobre lo que se rebasa y `:3:` el commit de la
    // rama que se está reaplicando.
    for dir in ["rebase-merge", "rebase-apply"] {
        let Some(head_name) = read(&format!("{dir}/head-name")) else {
            continue;
        };
        let branch = head_name
            .trim()
            .trim_start_matches("refs/heads/")
            .to_string();
        let commit = read("REBASE_HEAD").as_deref().and_then(short_hash);
        let theirs = match commit {
            Some(commit) if !branch.is_empty() && branch != "detached HEAD" => {
                format!("{branch} ({commit})")
            }
            Some(commit) => format!("commit {commit}"),
            None => branch,
        };
        let ours = read(&format!("{dir}/onto"))
            .as_deref()
            .and_then(short_hash)
            .map(|onto| {
                git.raw([
                    "name-rev",
                    "--name-only",
                    "--no-undefined",
                    "--exclude=refs/tags/*",
                    onto.as_str(),
                ])
                .ok()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or(onto)
            });
        return ConflictSides {
            ours,
            theirs: (!theirs.is_empty()).then_some(theirs),
        };
    }

    let theirs = if let Some(head) = read("MERGE_HEAD") {
        read("MERGE_MSG")
            .as_deref()
            .and_then(merge_message_name)
            .or_else(|| short_hash(&head))
    } else if let Some(head) = read("CHERRY_PICK_HEAD") {
        short_hash(&head).map(|commit| format!("commit {commit}"))
    } else if let Some(head) = read("REVERT_HEAD") {
        short_hash(&head).map(|commit| format!("revert of {commit}"))
    } else {
        None
    };
    ConflictSides {
        ours: current_branch(),
        theirs,
    }
}

/// Las tres versiones de `file_path` según el índice (`git ls-files -u` y
/// `git cat-file blob` de cada etapa) y el archivo del working tree. Un texto
/// de más de [`MAX_READ_FILE_BYTES`] es un error, como en `read_file`.
pub fn read_conflict_versions(
    repo_path: &std::path::Path,
    file_path: &str,
) -> QueryResult<ConflictVersions> {
    read("Failed to read conflict", || {
        let path = normalize_repo_relative_path(file_path)?;
        let full_path = assert_path_inside_repository(repo_path, &path)?;
        let git = open(repo_path)?;
        // `:(literal)`: la ruta no es un patrón aunque lleve `*` o `?`.
        let pathspec = format!(":(literal){path}");
        let listing = git.raw(["ls-files", "-u", "-z", "--", pathspec.as_str()])?;

        let mut versions = ConflictVersions {
            path: path.clone(),
            ..ConflictVersions::default()
        };
        for entry in listing.split('\0').filter(|entry| !entry.is_empty()) {
            let Some((mode, oid, stage, entry_path)) = parse_unmerged_entry(entry) else {
                continue;
            };
            if entry_path != path {
                continue;
            }
            let slot = match stage {
                1 => &mut versions.base,
                2 => &mut versions.ours,
                3 => &mut versions.theirs,
                _ => continue,
            };
            *slot = Some(read_stage(&git, mode, oid)?);
        }

        if versions.is_binary() || versions.ours.is_none() || versions.theirs.is_none() {
            versions.sides = conflict_sides(&git, repo_path);
        }

        // El working tree se lee siempre que se pueda (es lo que enseñaba el
        // resolvedor antes), pero sin etapas es lo único que hay: entonces su
        // error manda.
        if !versions.is_binary() {
            versions.working = match read_capped_utf8_optional(&full_path) {
                Ok(working) => working,
                Err(error) if !versions.has_stages() => return Err(error),
                Err(_) => None,
            };
        }
        Ok(versions)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    /// `main` y `feature` cambian `name` desde `base`; deja el merge parado.
    fn conflicted(
        name: &str,
        base: Option<&str>,
        ours: Option<&str>,
        theirs: Option<&str>,
    ) -> TestRepo {
        let repo = TestRepo::new();
        repo.commit_file("README", "readme\n", "init");
        if let Some(base) = base {
            repo.commit_file(name, base, "base");
        }
        repo.git(&["checkout", "-q", "-b", "feature"]);
        match theirs {
            Some(theirs) => {
                repo.commit_file(name, theirs, "feature");
            }
            None => {
                repo.git(&["rm", "-q", "--", name]);
                repo.git(&["commit", "-q", "-m", "feature removes"]);
            }
        }
        repo.git(&["checkout", "-q", "main"]);
        match ours {
            Some(ours) => {
                repo.commit_file(name, ours, "main");
            }
            None => {
                repo.git(&["rm", "-q", "--", name]);
                repo.git(&["commit", "-q", "-m", "main removes"]);
            }
        }
        let merged = mergeon_gitcmd::Git::new(&repo.path).exec(["merge", "feature"]);
        assert_ne!(merged.unwrap().exit_code, Some(0), "the merge should stop");
        repo
    }

    #[test]
    fn reads_the_three_stages_of_a_modify_modify_conflict() {
        let repo = conflicted(
            "notes.txt",
            Some("base\n"),
            Some("ours\n"),
            Some("theirs\n"),
        );
        let versions = read_conflict_versions(&repo.path, "notes.txt").unwrap();

        assert_eq!(versions.kind(), ConflictKind::BothModified);
        assert_eq!(versions.base.as_ref().unwrap().text, "base\n");
        assert_eq!(versions.ours.as_ref().unwrap().text, "ours\n");
        assert_eq!(versions.theirs.as_ref().unwrap().text, "theirs\n");
        assert_eq!(versions.ours.as_ref().unwrap().mode, "100644");
        assert!(!versions.is_binary());
        assert!(versions.working.as_deref().unwrap().contains("<<<<<<<"));
    }

    #[test]
    fn an_add_add_conflict_has_no_base() {
        let repo = conflicted("new.txt", None, Some("one\n"), Some("two\n"));
        let versions = read_conflict_versions(&repo.path, "new.txt").unwrap();

        assert_eq!(versions.kind(), ConflictKind::BothAdded);
        assert!(versions.base.is_none());
        assert_eq!(versions.ours.unwrap().text, "one\n");
        assert_eq!(versions.theirs.unwrap().text, "two\n");
    }

    #[test]
    fn a_delete_modify_conflict_misses_the_deleting_side() {
        let repo = conflicted("gone.txt", Some("base\n"), None, Some("changed\n"));
        let versions = read_conflict_versions(&repo.path, "gone.txt").unwrap();
        assert_eq!(versions.kind(), ConflictKind::DeletedByUs);
        assert!(versions.ours.is_none());
        assert_eq!(versions.theirs.unwrap().text, "changed\n");

        let repo = conflicted("kept.txt", Some("base\n"), Some("changed\n"), None);
        let versions = read_conflict_versions(&repo.path, "kept.txt").unwrap();
        assert_eq!(versions.kind(), ConflictKind::DeletedByThem);
        assert!(versions.theirs.is_none());
        assert_eq!(versions.ours.unwrap().text, "changed\n");
    }

    #[test]
    fn a_whole_file_conflict_names_both_sides() {
        let repo = conflicted("kept.txt", Some("base\n"), Some("changed\n"), None);
        let versions = read_conflict_versions(&repo.path, "kept.txt").unwrap();
        assert_eq!(
            versions.sides,
            ConflictSides {
                ours: Some("main".to_string()),
                theirs: Some("feature".to_string()),
            }
        );

        // Un texto que se fusiona por líneas no las necesita.
        let repo = conflicted("t.txt", Some("b\n"), Some("o\n"), Some("t\n"));
        let versions = read_conflict_versions(&repo.path, "t.txt").unwrap();
        assert_eq!(versions.sides, ConflictSides::default());
    }

    #[test]
    fn a_rebase_names_the_upstream_and_the_replayed_commit() {
        let repo = TestRepo::new();
        repo.commit_file("README", "readme\n", "init");
        repo.commit_file("gone.txt", "base\n", "base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.commit_file("gone.txt", "changed\n", "feature changes");
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["rm", "-q", "--", "gone.txt"]);
        repo.git(&["commit", "-q", "-m", "main removes"]);
        repo.git(&["checkout", "-q", "feature"]);
        let rebased = mergeon_gitcmd::Git::new(&repo.path).exec(["rebase", "main"]);
        assert_ne!(
            rebased.unwrap().exit_code,
            Some(0),
            "the rebase should stop"
        );

        let versions = read_conflict_versions(&repo.path, "gone.txt").unwrap();
        // En un rebase, `:2:` es `main` (lo que borró) y `:3:` el commit de
        // `feature` que se reaplica.
        assert_eq!(versions.kind(), ConflictKind::DeletedByUs);
        assert_eq!(versions.sides.ours.as_deref(), Some("main"));
        let theirs = versions.sides.theirs.unwrap();
        assert!(theirs.starts_with("feature ("), "{theirs}");
    }

    #[test]
    fn names_come_from_the_merge_message() {
        assert_eq!(
            merge_message_name("Merge branch 'feature/x'\n\n# Conflicts:\n"),
            Some("feature/x".to_string())
        );
        assert_eq!(
            merge_message_name("Merge remote-tracking branch 'origin/main' into dev"),
            Some("origin/main".to_string())
        );
        assert_eq!(merge_message_name("Merge something"), None);
        assert_eq!(
            short_hash("0123456789abcdef\n"),
            Some("0123456".to_string())
        );
        assert_eq!(short_hash("nope"), None);
    }

    #[test]
    fn a_binary_stage_carries_no_text() {
        let repo = TestRepo::new();
        repo.write("logo.bin", b"\x89PNG\0\0base");
        repo.git(&["add", "logo.bin"]);
        repo.git(&["commit", "-q", "-m", "base"]);
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("logo.bin", b"\x89PNG\0\0theirs!");
        repo.git(&["commit", "-q", "-am", "feature"]);
        repo.git(&["checkout", "-q", "main"]);
        repo.write("logo.bin", b"\x89PNG\0\0ours");
        repo.git(&["commit", "-q", "-am", "main"]);
        let _ = mergeon_gitcmd::Git::new(&repo.path).exec(["merge", "feature"]);

        let versions = read_conflict_versions(&repo.path, "logo.bin").unwrap();
        assert!(versions.is_binary());
        let theirs = versions.theirs.unwrap();
        assert!(theirs.binary);
        assert!(theirs.text.is_empty());
        assert_eq!(theirs.size, 13);
    }

    #[test]
    fn a_path_with_glob_characters_and_spaces_is_literal() {
        // Windows no admite `*` en un nombre de archivo; `[x]` sí.
        let name = if cfg!(windows) {
            "a [x].md"
        } else {
            "a [x]*.md"
        };
        let repo = conflicted(
            &format!("docs/{name}"),
            Some("base\n"),
            Some("ours\n"),
            Some("theirs\n"),
        );
        let versions = read_conflict_versions(&repo.path, &format!("docs\\{name}")).unwrap();
        assert_eq!(versions.path, format!("docs/{name}"));
        assert_eq!(versions.ours.unwrap().text, "ours\n");
    }

    #[test]
    fn without_stages_only_the_working_file_is_left() {
        let repo = TestRepo::new();
        repo.commit_file(
            "plain.txt",
            "<<<<<<< a\nx\n=======\ny\n>>>>>>> b\n",
            "markers",
        );
        let versions = read_conflict_versions(&repo.path, "plain.txt").unwrap();
        assert_eq!(versions.kind(), ConflictKind::Unstaged);
        assert!(versions.working.unwrap().starts_with("<<<<<<< a"));

        assert!(read_conflict_versions(&repo.path, "../outside").is_err());
    }
}
