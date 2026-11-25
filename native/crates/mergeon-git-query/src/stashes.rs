//! Stashes: listado, archivos y parche de un archivo.

use std::path::Path;

use crate::diff::{
    DiffOptions, DiffPayload, FileChange, git_diff_payload, parse_name_status_with_stats,
};
use mergeon_gitcmd::Git;

use crate::error::{Inner, QueryResult, read};
use crate::exec;
use crate::paths::normalize_repository_file_path;
use crate::simple_log::{LogEntry, parse_log_entries, pretty_argument};

/// Una entrada de `git stash list`, con la forma de `git.stashList().all`.
pub type StashEntry = LogEntry;

/// Canal `git:getStashes`: del más reciente (`stash@{0}`) al más antiguo.
pub fn get_stashes(repo_path: &Path) -> QueryResult<Vec<StashEntry>> {
    read("Failed to get stashes", || {
        let git = exec::open(repo_path)?;
        let output = exec::raw(&git, &["stash", "list", &pretty_argument()])?;
        Ok(parse_log_entries(&output))
    })
}

/// El tercer padre del stash (`<stash>^3`), donde `git stash -u` guarda los
/// archivos sin seguimiento, si lo tiene.
fn untracked_commit(git: &Git, stash_ref: &str) -> Inner<Option<String>> {
    let parent = format!("{stash_ref}^3");
    // Sin `^{commit}`: con él, git 2.53 da por bueno un `^3` que no existe.
    let output = exec::exec(git, &["rev-parse", "--verify", "--quiet", parent.as_str()])?;
    Ok((output.exit_code == Some(0)).then_some(parent))
}

/*
 * Archivos tocados por un stash, comparando contra su commit base (`ref^1`).
 * Un stash es un commit de merge (working tree + índice, y a veces un tercer
 * padre con los untracked); diffear contra el primer padre da exactamente lo
 * que el usuario guardó. Los sin seguimiento (`git stash -u`) están en el
 * tercer padre, un commit sin padres: salen como archivos nuevos (`A`).
 */
/// Canal `git:getStashFiles`.
pub fn get_stash_files(repo_path: &Path, stash_ref: &str) -> QueryResult<Vec<FileChange>> {
    read("Failed to read the stash contents", || {
        let git = exec::open(repo_path)?;
        let base = format!("{stash_ref}^1");
        let status_output = exec::raw(&git, &["diff", "--name-status", "-z", &base, stash_ref])?;
        let stats_output = exec::raw(&git, &["diff", "--numstat", "-z", &base, stash_ref])?;
        let mut files = parse_name_status_with_stats(&status_output, &stats_output);
        if let Some(untracked) = untracked_commit(&git, stash_ref)? {
            // `--root`: el commit de los untracked no tiene padres, y así se
            // compara con el árbol vacío.
            let tree_diff = |format: &str| {
                exec::raw(
                    &git,
                    &[
                        "diff-tree",
                        "-r",
                        "--root",
                        "--no-commit-id",
                        "-z",
                        format,
                        untracked.as_str(),
                    ],
                )
            };
            let status_output = tree_diff("--name-status")?;
            let stats_output = tree_diff("--numstat")?;
            let untracked: Vec<FileChange> =
                parse_name_status_with_stats(&status_output, &stats_output)
                    .into_iter()
                    .filter(|file| !files.iter().any(|tracked| tracked.path == file.path))
                    .collect();
            files.extend(untracked);
        }
        Ok(files)
    })
}

/// Canal `git:getStashDiff`: el parche de un archivo dentro de un stash. Un
/// archivo sin seguimiento guardado con `git stash -u` sale como nuevo,
/// desde el tercer padre del stash.
pub fn get_stash_diff(
    repo_path: &Path,
    stash_ref: &str,
    file_path: &str,
    options: &DiffOptions,
) -> QueryResult<DiffPayload> {
    read("Failed to read the stash diff", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;
        let base = format!("{stash_ref}^1");
        let mut args = vec!["diff"];
        options.push_args(&mut args);
        args.extend([base.as_str(), stash_ref, "--", &normalized_path]);

        let payload = git_diff_payload(&git, &args)?;
        if !payload.diff.is_empty() || payload.too_large {
            return Ok(payload);
        }
        let Some(untracked) = untracked_commit(&git, stash_ref)? else {
            return Ok(payload);
        };
        let pathspec = format!(":(literal){normalized_path}");
        let mut args = vec!["diff-tree", "-p", "-r", "--root", "--no-commit-id"];
        if options.ignore_whitespace {
            args.push("-w");
        }
        args.extend([untracked.as_str(), "--", pathspec.as_str()]);
        git_diff_payload(&git, &args)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn repo_with_stashes() -> TestRepo {
        let repo = TestRepo::new();
        repo.write("a.txt", "one\n");
        repo.write("b.txt", "bee\n");
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "base"]);

        repo.write("a.txt", "one\ntwo\n");
        repo.git(&["stash", "push", "-m", "first stash"]);

        repo.write("a.txt", "changed\n");
        repo.write("c.txt", "new\n");
        repo.git(&["add", "c.txt"]);
        repo.git(&["rm", "-q", "b.txt"]);
        repo.git(&["stash", "push", "-m", "second stash"]);
        repo
    }

    #[test]
    fn lists_stashes_newest_first() {
        let repo = repo_with_stashes();

        let stashes = get_stashes(&repo.path).unwrap();

        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].message, "On main: second stash");
        assert_eq!(stashes[1].message, "On main: first stash");
        assert_eq!(stashes[0].refs, "refs/stash");
        assert_eq!(stashes[0].hash.len(), 40);
        assert_eq!(stashes[0].author_name, "Test User");
        assert_eq!(stashes[0].author_email, "test@example.com");
        assert!(stashes[0].date.contains('T'));

        let json = serde_json::to_value(&stashes[0]).unwrap();
        assert_eq!(json["author_name"], "Test User");
        assert_eq!(json["body"], "");
    }

    #[test]
    fn a_repository_without_stashes_lists_nothing() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "base");
        assert!(get_stashes(&repo.path).unwrap().is_empty());
    }

    #[test]
    fn lists_the_files_saved_in_a_stash() {
        let repo = repo_with_stashes();

        let mut files = get_stash_files(&repo.path, "stash@{0}").unwrap();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let summary: Vec<_> = files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.status.as_str(),
                    file.additions,
                    file.deletions,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("a.txt", "M", 1, 1),
                ("b.txt", "D", 0, 1),
                ("c.txt", "A", 1, 0)
            ]
        );

        let older = get_stash_files(&repo.path, "stash@{1}").unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].path, "a.txt");
        assert_eq!((older[0].additions, older[0].deletions), (1, 0));
    }

    // Mismo defecto que en el detalle de un commit (ver
    // `parse_name_status_with_stats`): renombres en 0/0 y rutas entrecomilladas.
    #[test]
    fn a_stashed_rename_and_unicode_paths_keep_their_stats() {
        let repo = TestRepo::new();
        repo.write("viejo.txt", "a\nb\nc\nd\ne\n");
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "base"]);

        repo.git(&["mv", "viejo.txt", "señal.txt"]);
        repo.write("señal.txt", "a\nb\nc\nd\nE\n");
        repo.write("ñu.txt", "nuevo\n");
        repo.git(&["add", "--all"]);
        repo.git(&["stash", "push", "-m", "rename"]);

        let mut files = get_stash_files(&repo.path, "stash@{0}").unwrap();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let summary: Vec<_> = files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.status.as_str(),
                    file.additions,
                    file.deletions,
                )
            })
            .collect();
        assert_eq!(summary, [("señal.txt", "R", 1, 1), ("ñu.txt", "A", 1, 0)]);
    }

    #[test]
    fn shows_the_patch_of_one_file_in_a_stash() {
        let repo = repo_with_stashes();

        let payload =
            get_stash_diff(&repo.path, "stash@{1}", "a.txt", &DiffOptions::default()).unwrap();

        assert!(payload.diff.contains("+two"), "{}", payload.diff);
        assert!(!payload.too_large);
        assert_eq!(payload.size, payload.diff.len() as u64);

        let other =
            get_stash_diff(&repo.path, "stash@{1}", "b.txt", &DiffOptions::default()).unwrap();
        assert_eq!(other.diff, "");
    }

    /// `git stash -u` con solo archivos sin seguimiento: no hay nada contra
    /// el primer padre, todo está en el tercero.
    #[test]
    fn an_untracked_only_stash_lists_its_files_as_new() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "base");
        repo.write("notes/todo list.md", "first\nsecond\n");
        repo.write("ñu.txt", "nuevo\n");
        repo.git(&["stash", "push", "-u", "-m", "untracked only"]);
        assert!(!repo.path.join("ñu.txt").exists());

        let mut files = get_stash_files(&repo.path, "stash@{0}").unwrap();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let summary: Vec<_> = files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.status.as_str(),
                    file.additions,
                    file.deletions,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [("notes/todo list.md", "A", 2, 0), ("ñu.txt", "A", 1, 0)]
        );

        let payload = get_stash_diff(
            &repo.path,
            "stash@{0}",
            "notes/todo list.md",
            &DiffOptions::default(),
        )
        .unwrap();
        assert!(payload.diff.contains("new file mode"), "{}", payload.diff);
        assert!(payload.diff.contains("+first\n+second"), "{}", payload.diff);
        assert!(!payload.too_large);
    }

    /// Cambios seguidos y sin seguimiento a la vez: salen los dos, y el
    /// parche de cada uno viene de su sitio.
    #[test]
    fn a_stash_with_untracked_files_lists_both_kinds() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "base");
        repo.write("a.txt", "one\ntwo\n");
        repo.write("new.txt", "fresh\n");
        repo.git(&["stash", "push", "-u"]);

        let mut files = get_stash_files(&repo.path, "stash@{0}").unwrap();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let paths: Vec<_> = files
            .iter()
            .map(|file| (file.path.as_str(), file.status.as_str()))
            .collect();
        assert_eq!(paths, [("a.txt", "M"), ("new.txt", "A")]);

        let tracked =
            get_stash_diff(&repo.path, "stash@{0}", "a.txt", &DiffOptions::default()).unwrap();
        assert!(tracked.diff.contains("+two"), "{}", tracked.diff);
        let untracked =
            get_stash_diff(&repo.path, "stash@{0}", "new.txt", &DiffOptions::default()).unwrap();
        assert!(untracked.diff.contains("+fresh"), "{}", untracked.diff);
        // Un archivo que no está en el stash no tiene parche.
        let missing =
            get_stash_diff(&repo.path, "stash@{0}", "nope.txt", &DiffOptions::default()).unwrap();
        assert_eq!(missing.diff, "");
    }

    #[test]
    fn failures_carry_the_read_prefix() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "base");

        let error = get_stash_files(&repo.path, "stash@{0}").unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to read the stash contents: "),
            "{error}"
        );

        let error =
            get_stash_diff(&repo.path, "stash@{0}", "a.txt", &DiffOptions::default()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to read the stash diff: "),
            "{error}"
        );

        let error = get_stash_diff(&repo.path, "stash@{0}", "../a.txt", &DiffOptions::default())
            .unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to read the stash diff: The file path must stay inside the repository."
        );

        let dir = tempfile::tempdir().unwrap();
        assert!(
            get_stashes(dir.path())
                .unwrap_err()
                .message()
                .starts_with("Failed to get stashes: ")
        );
    }
}
