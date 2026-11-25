//! `stash_push_operation`: stash de unas rutas, con lo que no se sigue,
//! conservando el índice o solo lo preparado.

use std::path::Path;

use super::support::*;
use crate::{Code, Outcome, StashOptions, stash_capabilities, stash_push_operation};

/// `git status --porcelain` sin la cabecera, línea a línea y ordenado.
fn porcelain(repo_path: &Path) -> Vec<String> {
    let mut lines: Vec<String> = run_git(repo_path, &["status", "--porcelain"])
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

/// Archivos que guarda el último stash (con los que no se siguen).
fn stashed_files(repo_path: &Path) -> Vec<String> {
    let mut files: Vec<String> = run_git(
        repo_path,
        &[
            "stash",
            "show",
            "--include-untracked",
            "--name-only",
            "stash@{0}",
        ],
    )
    .lines()
    .map(str::to_string)
    .collect();
    files.sort();
    files
}

fn stash_count(repo_path: &Path) -> usize {
    run_git(repo_path, &["stash", "list"]).lines().count()
}

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| path.to_string()).collect()
}

/// Un nombre con comodines de pathspec. Windows no admite `*` en un nombre
/// de archivo, pero sí `[...]`, que git también tomaría por un patrón.
const WILD: &str = if cfg!(windows) { "b[ab].txt" } else { "b*.txt" };

/// `base` con tres archivos seguidos más `file.txt`.
fn repository_with_files() -> (tempfile::TempDir, std::path::PathBuf) {
    let (root, repo_path) = create_repository();
    commit_file_at(&repo_path, "a\n", "a", "a.txt");
    commit_file_at(&repo_path, "spaced\n", "spaced", "with space.txt");
    commit_file_at(&repo_path, "star\n", "star", WILD);
    (root, repo_path)
}

#[test]
fn stashes_only_the_given_paths_even_with_spaces_and_wildcards_in_their_names() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("a.txt"), "a changed\n").unwrap();
    std::fs::write(repo_path.join("with space.txt"), "spaced changed\n").unwrap();
    std::fs::write(repo_path.join(WILD), "star changed\n").unwrap();
    std::fs::write(repo_path.join("file.txt"), "base changed\n").unwrap();

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            message: Some("  two files  ".to_string()),
            paths: paths(&["with space.txt", WILD]),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Stashed changes in 2 files.");
    assert_eq!(stashed_files(&repo_path), [WILD, "with space.txt"]);
    // `WILD` es literal: no arrastra a otros archivos.
    assert_eq!(porcelain(&repo_path), [" M a.txt", " M file.txt"]);
    assert_eq!(
        run_git(&repo_path, &["stash", "list", "--format=%gs"]).trim(),
        "On main: two files"
    );
    assert_eq!(result.files_affected, ["with space.txt", WILD]);
}

#[test]
fn includes_untracked_files_only_when_asked() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("a.txt"), "a changed\n").unwrap();
    std::fs::write(repo_path.join("new file.txt"), "new\n").unwrap();

    let without = stash_push_operation(&repo_path, &StashOptions::default());
    assert!(without.success, "{}", without.message);
    assert_eq!(stashed_files(&repo_path), ["a.txt"]);
    assert_eq!(porcelain(&repo_path), ["?? \"new file.txt\""]);
    run_git(&repo_path, &["stash", "pop"]);

    let with = stash_push_operation(
        &repo_path,
        &StashOptions {
            include_untracked: true,
            ..Default::default()
        },
    );
    assert!(with.success, "{}", with.message);
    assert_eq!(stashed_files(&repo_path), ["a.txt", "new file.txt"]);
    assert!(porcelain(&repo_path).is_empty());
}

#[test]
fn an_untracked_path_needs_include_untracked() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("new file.txt"), "new\n").unwrap();

    let blocked = stash_push_operation(
        &repo_path,
        &StashOptions {
            paths: paths(&["new file.txt"]),
            ..Default::default()
        },
    );
    assert!(!blocked.success);
    assert_eq!(blocked.outcome, Outcome::Blocked);
    assert!(blocked.message.contains("Include untracked files"));
    assert_eq!(stash_count(&repo_path), 0);

    let stashed = stash_push_operation(
        &repo_path,
        &StashOptions {
            paths: paths(&["new file.txt"]),
            include_untracked: true,
            ..Default::default()
        },
    );
    assert!(stashed.success, "{}", stashed.message);
    assert_eq!(stashed.message, "Stashed changes in 1 file.");
    assert_eq!(stashed_files(&repo_path), ["new file.txt"]);
    assert!(porcelain(&repo_path).is_empty());
}

#[test]
fn keep_index_leaves_the_staged_half_of_a_partially_staged_file() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("a.txt"), "a\nstaged\n").unwrap();
    run_git(&repo_path, &["add", "a.txt"]);
    std::fs::write(repo_path.join("a.txt"), "a\nstaged\nunstaged\n").unwrap();

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            keep_index: true,
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    // Lo preparado se queda en el índice y en el working tree; el stash
    // guarda el archivo entero.
    assert_eq!(porcelain(&repo_path), ["M  a.txt"]);
    assert_eq!(read_file(&repo_path.join("a.txt")), "a\nstaged\n");
    assert_eq!(
        run_git(&repo_path, &["show", "stash@{0}:a.txt"]),
        "a\nstaged\nunstaged\n"
    );
}

#[test]
fn staged_only_stashes_the_staged_half_and_keeps_the_rest() {
    if !stash_capabilities().staged_only {
        eprintln!("git < 2.35: se omite el test de --staged");
        return;
    }
    let (_root, repo_path) = repository_with_files();
    let lines: String = (1..=10).map(|line| format!("{line}\n")).collect();
    commit_file_at(&repo_path, &lines, "numbers", "numbers.txt");
    // Un archivo a medio preparar con las dos partes separadas: la primera
    // línea preparada, la novena sin preparar.
    std::fs::write(repo_path.join("numbers.txt"), lines.replace("1\n", "one\n")).unwrap();
    run_git(&repo_path, &["add", "numbers.txt"]);
    std::fs::write(
        repo_path.join("numbers.txt"),
        lines.replace("1\n", "one\n").replace("9\n", "nine\n"),
    )
    .unwrap();
    std::fs::write(repo_path.join("a.txt"), "a changed\n").unwrap();

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            staged_only: true,
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Staged changes stashed.");
    assert_eq!(porcelain(&repo_path), [" M a.txt", " M numbers.txt"]);
    assert_eq!(
        read_file(&repo_path.join("numbers.txt")),
        lines.replace("9\n", "nine\n")
    );
    assert_eq!(stashed_files(&repo_path), ["numbers.txt"]);
}

#[test]
fn staged_only_that_git_cannot_untangle_leaves_everything_as_it_was() {
    if !stash_capabilities().staged_only {
        eprintln!("git < 2.35: se omite el test de --staged");
        return;
    }
    let (_root, repo_path) = repository_with_files();
    // Preparado y sin preparar en líneas contiguas: git crea el stash pero no
    // puede sacar lo preparado del working tree.
    std::fs::write(repo_path.join("a.txt"), "a\nstaged\n").unwrap();
    run_git(&repo_path, &["add", "a.txt"]);
    std::fs::write(repo_path.join("a.txt"), "a\nstaged\nunstaged\n").unwrap();
    let before = porcelain(&repo_path);

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            staged_only: true,
            ..Default::default()
        },
    );

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert!(
        result.message.contains("Nothing was stashed"),
        "{}",
        result.message
    );
    assert_eq!(stash_count(&repo_path), 0);
    assert_eq!(porcelain(&repo_path), before);
    assert_eq!(read_file(&repo_path.join("a.txt")), "a\nstaged\nunstaged\n");
}

#[test]
fn rejects_option_combinations_git_does_not_support() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("a.txt"), "a changed\n").unwrap();
    run_git(&repo_path, &["add", "a.txt"]);

    for (options, needle) in [
        (
            StashOptions {
                staged_only: true,
                include_untracked: true,
                ..Default::default()
            },
            "untracked",
        ),
        (
            StashOptions {
                staged_only: true,
                keep_index: true,
                ..Default::default()
            },
            "index",
        ),
    ] {
        let result = stash_push_operation(&repo_path, &options);
        assert!(!result.success);
        assert_eq!(result.outcome, Outcome::Blocked);
        if stash_capabilities().staged_only {
            assert!(result.message.contains(needle), "{}", result.message);
        }
    }
    assert_eq!(stash_count(&repo_path), 0);
}

#[test]
fn paths_without_changes_are_a_noop() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("a.txt"), "a changed\n").unwrap();

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            paths: paths(&["file.txt"]),
            ..Default::default()
        },
    );
    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Noop);
    assert_eq!(result.code, Code::NoChanges);
    assert_eq!(
        result.message,
        "The selected files have no changes to stash."
    );

    let staged = stash_push_operation(
        &repo_path,
        &StashOptions {
            staged_only: true,
            ..Default::default()
        },
    );
    assert_eq!(staged.outcome, Outcome::Noop);
    assert_eq!(staged.message, "There are no staged changes to stash.");
    assert_eq!(stash_count(&repo_path), 0);
}

#[test]
fn staged_renames_and_deletions_by_path_are_explained_instead_of_failing() {
    let (_root, repo_path) = repository_with_files();
    run_git(&repo_path, &["mv", "a.txt", "renamed a.txt"]);
    run_git(&repo_path, &["rm", "-q", WILD]);
    std::fs::write(repo_path.join("file.txt"), "other\n").unwrap();
    let before = porcelain(&repo_path);

    for (path, needle) in [
        (
            "renamed a.txt",
            "staged rename of renamed a.txt".to_string(),
        ),
        (WILD, format!("staged deletion of {WILD}")),
    ] {
        let result = stash_push_operation(
            &repo_path,
            &StashOptions {
                paths: paths(&[path]),
                ..Default::default()
            },
        );
        assert!(!result.success);
        assert_eq!(result.outcome, Outcome::Blocked);
        assert!(result.message.contains(&needle), "{}", result.message);
    }
    assert_eq!(stash_count(&repo_path), 0);
    assert_eq!(porcelain(&repo_path), before);

    // Sin rutas, el stash de todo sí los guarda y los recupera enteros.
    let all = stash_push_operation(&repo_path, &StashOptions::default());
    assert!(all.success, "{}", all.message);
    assert!(porcelain(&repo_path).is_empty());
    run_git(&repo_path, &["stash", "pop", "--index"]);
    assert_eq!(porcelain(&repo_path), before);
}

#[test]
fn keep_index_with_paths_and_a_partially_staged_file() {
    let (_root, repo_path) = repository_with_files();
    std::fs::write(repo_path.join("a.txt"), "a\nstaged\n").unwrap();
    run_git(&repo_path, &["add", "a.txt"]);
    std::fs::write(repo_path.join("a.txt"), "a\nstaged\nunstaged\n").unwrap();
    std::fs::write(repo_path.join("file.txt"), "other\n").unwrap();

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            paths: paths(&["a.txt"]),
            keep_index: true,
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(porcelain(&repo_path), [" M file.txt", "M  a.txt"]);
    assert_eq!(stashed_files(&repo_path), ["a.txt"]);
}

#[test]
fn many_paths_go_through_stdin() {
    let (_root, repo_path) = create_repository();
    let names: Vec<String> = (0..300)
        .map(|index| format!("dir/file with spaces {index:03}.txt"))
        .collect();
    std::fs::create_dir(repo_path.join("dir")).unwrap();
    for name in &names {
        std::fs::write(repo_path.join(name), "x\n").unwrap();
    }
    run_git(&repo_path, &["add", "dir"]);
    run_git(&repo_path, &["commit", "-m", "many"]);
    for name in &names {
        std::fs::write(repo_path.join(name), "y\n").unwrap();
    }
    std::fs::write(repo_path.join("file.txt"), "kept\n").unwrap();

    let result = stash_push_operation(
        &repo_path,
        &StashOptions {
            paths: names[..250].to_vec(),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(stashed_files(&repo_path).len(), 250);
    // Quedan los 50 sin pedir y `file.txt`.
    assert_eq!(porcelain(&repo_path).len(), 51);
}
