//! Deshacer y rehacer: captura, copia de descartes y cada paso, con git real.

use std::path::Path;

use super::support::*;
use crate::undo::{
    DiscardBackupOutcome, DiscardProbe, HeadTarget, PatchData, RefProbe, ResetMode, SavedKind,
    UndoStep, back_up_discard, estimate_discard_backup_size, published_upstream, read_ref_snapshot,
    undo_operation,
};
use crate::{
    Code, Operation, Outcome, discard_all_operation, discard_changes_operation,
    pop_stash_operation, stash_operation,
};

fn status_lines(repo_path: &Path) -> Vec<String> {
    run_git(repo_path, &["status", "--porcelain"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn stash_shas(repo_path: &Path) -> Vec<String> {
    run_git(repo_path, &["stash", "list", "--format=%H"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn rev(repo_path: &Path, revision: &str) -> String {
    run_git(repo_path, &["rev-parse", revision])
        .trim()
        .to_string()
}

fn undo(repo_path: &Path, steps: &[UndoStep]) -> crate::MutationResult {
    undo_operation(repo_path, "the test operation", steps, false)
}

#[test]
fn the_snapshot_reads_head_the_branch_refs_and_config() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["branch", "topic"]);
    run_git(&repo_path, &["config", "branch.topic.remote", "origin"]);
    run_git(
        &repo_path,
        &["config", "branch.topic.merge", "refs/heads/topic"],
    );

    let probe = RefProbe::head()
        .with_branch("topic")
        .with_ref("refs/tags/missing");
    let snapshot = read_ref_snapshot(&repo_path, &probe);

    assert_eq!(snapshot.head.as_deref(), Some(head(&repo_path).as_str()));
    assert_eq!(snapshot.branch(), Some("main"));
    assert_eq!(snapshot.rev("refs/heads/topic"), snapshot.head.as_deref());
    assert_eq!(snapshot.rev("refs/tags/missing"), None);
    assert_eq!(snapshot.config("branch.topic.remote"), Some("origin"));
    assert_eq!(
        snapshot.head_target(),
        Some(HeadTarget::Branch("main".into()))
    );

    run_git(&repo_path, &["checkout", "--detach"]);
    let detached = read_ref_snapshot(&repo_path, &RefProbe::head());
    assert_eq!(detached.branch(), None);
    assert_eq!(
        detached.head_target(),
        Some(HeadTarget::Detached(head(&repo_path)))
    );
}

#[test]
fn undoing_a_commit_with_soft_leaves_its_changes_staged_and_redo_restores_it() {
    let (_root, repo_path) = create_repository();
    let before = head(&repo_path);
    let after = commit_file(&repo_path, "second\n", "second");
    let back = UndoStep::MoveHead {
        branch: Some("main".into()),
        from: after.clone(),
        to: before.clone(),
        mode: ResetMode::Soft,
    };

    let result = undo_operation(&repo_path, "commit 'second'", &[back], false);
    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::Undo);
    assert_eq!(result.message, "Undid commit 'second'.");
    assert_eq!(head(&repo_path), before);
    assert_eq!(status_lines(&repo_path), ["M  file.txt"]);

    let forward = UndoStep::MoveHead {
        branch: Some("main".into()),
        from: before,
        to: after.clone(),
        mode: ResetMode::Soft,
    };
    let result = undo_operation(&repo_path, "commit 'second'", &[forward], true);
    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::Redo);
    assert_eq!(result.message, "Redid commit 'second'.");
    assert_eq!(head(&repo_path), after);
    assert!(status_lines(&repo_path).is_empty());
}

#[test]
fn a_head_that_moved_outside_makes_the_entry_stale_and_touches_nothing() {
    let (_root, repo_path) = create_repository();
    let before = head(&repo_path);
    let after = commit_file(&repo_path, "second\n", "second");
    // Alguien hace otro commit en la terminal.
    let external = commit_file(&repo_path, "third\n", "third");

    let result = undo(
        &repo_path,
        &[UndoStep::MoveHead {
            branch: Some("main".into()),
            from: after,
            to: before,
            mode: ResetMode::Keep,
        }],
    );
    assert!(!result.success);
    assert_eq!(result.code, Code::UndoStale);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert!(
        result.message.contains("HEAD moved since"),
        "{}",
        result.message
    );
    assert_eq!(head(&repo_path), external);

    // Otra rama: tampoco aplica.
    run_git(&repo_path, &["checkout", "-b", "elsewhere"]);
    let result = undo(
        &repo_path,
        &[UndoStep::MoveHead {
            branch: Some("main".into()),
            from: external.clone(),
            to: external,
            mode: ResetMode::Keep,
        }],
    );
    assert_eq!(result.code, Code::UndoStale);
    assert!(result.message.contains("'elsewhere'"), "{}", result.message);
}

#[test]
fn keep_refuses_to_overwrite_local_changes_and_says_which_file() {
    let (_root, repo_path) = create_repository();
    let before = head(&repo_path);
    let after = commit_file(&repo_path, "merged\n", "merge-like change");
    // Cambio local en el mismo archivo que el commit deshecho.
    std::fs::write(repo_path.join("file.txt"), "local edit\n").unwrap();

    let result = undo(
        &repo_path,
        &[UndoStep::MoveHead {
            branch: Some("main".into()),
            from: after.clone(),
            to: before,
            mode: ResetMode::Keep,
        }],
    );
    assert!(!result.success);
    assert_eq!(result.code, Code::UndoBlocked);
    assert!(result.message.contains("file.txt"), "{}", result.message);
    assert!(
        result.message.contains("Commit, stash or discard"),
        "{}",
        result.message
    );
    // Nada se perdió.
    assert_eq!(head(&repo_path), after);
    assert_eq!(read_file(&repo_path.join("file.txt")), "local edit\n");
}

#[test]
fn keep_carries_unrelated_local_changes() {
    let (_root, repo_path) = create_repository();
    let before = head(&repo_path);
    let after = commit_file(&repo_path, "second\n", "second");
    std::fs::write(repo_path.join("other.txt"), "untouched\n").unwrap();

    let result = undo(
        &repo_path,
        &[UndoStep::MoveHead {
            branch: Some("main".into()),
            from: after,
            to: before.clone(),
            mode: ResetMode::Keep,
        }],
    );
    assert!(result.success, "{}", result.message);
    assert_eq!(head(&repo_path), before);
    assert_eq!(read_file(&repo_path.join("file.txt")), "base\n");
    assert_eq!(read_file(&repo_path.join("other.txt")), "untouched\n");
}

#[test]
fn checkout_goes_back_and_forth_without_forcing() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    let back = UndoStep::Checkout {
        from: HeadTarget::Branch("feature".into()),
        to: HeadTarget::Branch("main".into()),
    };

    assert!(undo(&repo_path, std::slice::from_ref(&back)).success);
    assert_eq!(
        run_git(&repo_path, &["branch", "--show-current"]).trim(),
        "main"
    );
    // Ya no está en `feature`: la misma entrada no aplica dos veces.
    assert_eq!(undo(&repo_path, &[back]).code, Code::UndoStale);

    let sha = head(&repo_path);
    let detach = UndoStep::Checkout {
        from: HeadTarget::Branch("main".into()),
        to: HeadTarget::Detached(sha.clone()),
    };
    assert!(undo(&repo_path, &[detach]).success);
    assert_eq!(
        run_git(&repo_path, &["branch", "--show-current"]).trim(),
        ""
    );
    assert_eq!(head(&repo_path), sha);
}

#[test]
fn a_deleted_branch_comes_back_with_its_hash_and_upstream() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "foo"]);
    let sha = commit_file(&repo_path, "foo\n", "foo work");
    run_git(&repo_path, &["config", "branch.foo.remote", "origin"]);
    run_git(
        &repo_path,
        &["config", "branch.foo.merge", "refs/heads/foo"],
    );
    run_git(&repo_path, &["checkout", "main"]);
    run_git(&repo_path, &["branch", "-D", "foo"]);

    let create = UndoStep::CreateBranch {
        name: "foo".into(),
        sha: sha.clone(),
        upstream: Some(("origin".into(), "refs/heads/foo".into())),
    };
    let result = undo(&repo_path, std::slice::from_ref(&create));
    assert!(result.success, "{}", result.message);
    assert_eq!(rev(&repo_path, "refs/heads/foo"), sha);
    assert_eq!(
        run_git(&repo_path, &["config", "branch.foo.merge"]).trim(),
        "refs/heads/foo"
    );
    // Ya existe: recrearla otra vez no aplica.
    assert_eq!(undo(&repo_path, &[create]).code, Code::UndoStale);

    // Rehacer el borrado: solo si sigue en el mismo commit.
    let delete = UndoStep::DeleteBranch {
        name: "foo".into(),
        sha: head(&repo_path),
    };
    assert_eq!(undo(&repo_path, &[delete]).code, Code::UndoStale);
    let delete = UndoStep::DeleteBranch {
        name: "foo".into(),
        sha,
    };
    assert!(undo(&repo_path, &[delete]).success);
    assert!(
        run_git(&repo_path, &["branch", "--list", "foo"])
            .trim()
            .is_empty()
    );
    assert!(
        git_config_missing(&repo_path, "branch.foo.remote"),
        "el upstream se va con la rama"
    );
}

fn git_config_missing(repo_path: &Path, key: &str) -> bool {
    !crate::exec::git(repo_path)
        .exec(["config", "--get", key])
        .unwrap()
        .success()
}

#[test]
fn renaming_back_needs_the_new_name_free() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["branch", "old"]);
    run_git(&repo_path, &["branch", "-m", "old", "new"]);

    let back = UndoStep::RenameBranch {
        from: "new".into(),
        to: "old".into(),
    };
    assert!(undo(&repo_path, std::slice::from_ref(&back)).success);
    assert!(
        !run_git(&repo_path, &["branch", "--list", "old"])
            .trim()
            .is_empty()
    );
    assert_eq!(undo(&repo_path, &[back]).code, Code::UndoStale);
}

#[test]
fn a_deleted_annotated_tag_comes_back_identical() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["tag", "-a", "v1", "-m", "Release one"]);
    let tag_object = rev(&repo_path, "refs/tags/v1");
    run_git(&repo_path, &["tag", "-d", "v1"]);

    let result = undo(
        &repo_path,
        &[UndoStep::CreateTag {
            name: "v1".into(),
            sha: tag_object.clone(),
        }],
    );
    assert!(result.success, "{}", result.message);
    assert_eq!(rev(&repo_path, "refs/tags/v1"), tag_object);
    assert_eq!(run_git(&repo_path, &["cat-file", "-t", "v1"]).trim(), "tag");
    assert_eq!(
        run_git(
            &repo_path,
            &["tag", "-l", "--format=%(contents:subject)", "v1"]
        )
        .trim(),
        "Release one"
    );

    // Y se borra otra vez si nadie lo movió.
    assert!(
        undo(
            &repo_path,
            &[UndoStep::DeleteTag {
                name: "v1".into(),
                sha: tag_object,
            }],
        )
        .success
    );
    assert!(run_git(&repo_path, &["tag", "-l"]).trim().is_empty());
}

#[test]
fn a_dropped_stash_goes_back_to_the_list_with_its_message() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "wip\n").unwrap();
    assert!(stash_operation(&repo_path, Some("keep me")).success);
    let sha = stash_shas(&repo_path)[0].clone();
    run_git(&repo_path, &["stash", "drop"]);
    assert!(stash_shas(&repo_path).is_empty());

    let store = UndoStep::StashStore { sha: sha.clone() };
    assert!(undo(&repo_path, std::slice::from_ref(&store)).success);
    assert_eq!(stash_shas(&repo_path), std::slice::from_ref(&sha));
    assert_eq!(
        run_git(&repo_path, &["stash", "list", "--format=%gs"]).trim(),
        "On main: keep me"
    );
    // Ya está: no se duplica.
    assert_eq!(undo(&repo_path, &[store]).code, Code::UndoStale);
    assert!(undo(&repo_path, &[UndoStep::StashDrop { sha }]).success);
    assert!(stash_shas(&repo_path).is_empty());
}

#[test]
fn undoing_a_pop_takes_the_changes_back_out_including_untracked_files() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    std::fs::write(repo_path.join("new.txt"), "untracked\n").unwrap();
    run_git(
        &repo_path,
        &["stash", "push", "--include-untracked", "-m", "wip"],
    );
    let sha = stash_shas(&repo_path)[0].clone();
    assert!(pop_stash_operation(&repo_path, "stash@{0}").success);
    assert_eq!(read_file(&repo_path.join("new.txt")), "untracked\n");

    let result = undo(
        &repo_path,
        &[
            UndoStep::StashUnapply { sha: sha.clone() },
            UndoStep::StashStore { sha: sha.clone() },
        ],
    );
    assert!(result.success, "{}", result.message);
    assert!(
        status_lines(&repo_path).is_empty(),
        "{:?}",
        status_lines(&repo_path)
    );
    assert_eq!(stash_shas(&repo_path), std::slice::from_ref(&sha));

    // Rehacer el pop.
    assert!(
        undo(
            &repo_path,
            &[UndoStep::StashApply {
                sha: sha.clone(),
                pop: true
            }]
        )
        .success
    );
    assert_eq!(read_file(&repo_path.join("file.txt")), "changed\n");
    assert_eq!(read_file(&repo_path.join("new.txt")), "untracked\n");
    assert!(stash_shas(&repo_path).is_empty());
}

#[test]
fn unapplying_refuses_when_an_untracked_file_changed_after_the_pop() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("new.txt"), "untracked\n").unwrap();
    run_git(&repo_path, &["stash", "push", "--include-untracked"]);
    let sha = stash_shas(&repo_path)[0].clone();
    assert!(pop_stash_operation(&repo_path, "stash@{0}").success);
    std::fs::write(repo_path.join("new.txt"), "edited after\n").unwrap();

    let result = undo(&repo_path, &[UndoStep::StashUnapply { sha }]);
    assert_eq!(result.code, Code::UndoBlocked);
    assert_eq!(read_file(&repo_path.join("new.txt")), "edited after\n");
}

#[test]
fn undoing_a_stash_push_pops_it_with_the_index_and_redo_stashes_it_again() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "staged\n").unwrap();
    run_git(&repo_path, &["add", "file.txt"]);
    assert!(stash_operation(&repo_path, None).success);
    let sha = stash_shas(&repo_path)[0].clone();

    assert!(
        undo(
            &repo_path,
            &[UndoStep::StashApply {
                sha: sha.clone(),
                pop: true
            }]
        )
        .success
    );
    // Con `--index`: lo preparado vuelve preparado.
    assert_eq!(status_lines(&repo_path), ["M  file.txt"]);
    assert!(stash_shas(&repo_path).is_empty());

    let redo = undo_operation(
        &repo_path,
        "stash",
        &[
            UndoStep::StashUnapply { sha: sha.clone() },
            UndoStep::StashStore { sha: sha.clone() },
        ],
        true,
    );
    assert!(redo.success, "{}", redo.message);
    assert!(status_lines(&repo_path).is_empty());
    assert_eq!(stash_shas(&repo_path), [sha]);
}

fn probe(directory: &Path, paths: Option<Vec<String>>) -> DiscardProbe {
    DiscardProbe {
        paths,
        directory: directory.to_path_buf(),
        limit: 10 * 1024 * 1024,
    }
}

#[test]
fn a_discard_all_is_recovered_with_tracked_untracked_and_binary_files() {
    let (root, repo_path) = create_repository();
    std::fs::write(repo_path.join("image.bin"), [0u8, 1, 2, 3, 255]).unwrap();
    run_git(&repo_path, &["add", "image.bin"]);
    run_git(&repo_path, &["commit", "-m", "binary"]);

    std::fs::write(repo_path.join("file.txt"), "edited\n").unwrap();
    std::fs::write(repo_path.join("image.bin"), [9u8, 8, 0, 7]).unwrap();
    std::fs::create_dir_all(repo_path.join("notes/deep")).unwrap();
    std::fs::write(repo_path.join("notes/deep/todo.md"), "untracked note\n").unwrap();
    std::fs::write(repo_path.join("loose.txt"), "loose\n").unwrap();
    let before = status_lines(&repo_path);

    let backup_dir = root.path().join("backup");
    let DiscardBackupOutcome::Saved(backup) =
        back_up_discard(&repo_path, &probe(&backup_dir, None))
    else {
        panic!("se esperaba una copia");
    };
    assert!(backup.patch.is_some());
    assert!(backup.untracked.iter().any(
        |file| file.path == "notes/deep/todo.md" && matches!(file.kind, SavedKind::File { .. })
    ));
    assert!(discard_all_operation(&repo_path, None).success);
    assert!(status_lines(&repo_path).is_empty());

    let restore = [
        UndoStep::ApplyPatch {
            patch: PatchData::File(backup.patch.clone().unwrap()),
            reverse: false,
        },
        UndoStep::RestoreFiles {
            files: backup.untracked.clone(),
        },
    ];
    let result = undo(&repo_path, &restore);
    assert!(result.success, "{}", result.message);
    assert_eq!(status_lines(&repo_path), before);
    assert_eq!(read_file(&repo_path.join("file.txt")), "edited\n");
    assert_eq!(
        std::fs::read(repo_path.join("image.bin")).unwrap(),
        [9u8, 8, 0, 7]
    );
    assert_eq!(
        read_file(&repo_path.join("notes/deep/todo.md")),
        "untracked note\n"
    );

    // Rehacer: se descarta otra vez lo mismo.
    let again = [
        UndoStep::RemoveFiles {
            files: backup.untracked.clone(),
        },
        UndoStep::ApplyPatch {
            patch: PatchData::File(backup.patch.clone().unwrap()),
            reverse: true,
        },
    ];
    let result = undo_operation(&repo_path, "discard", &again, true);
    assert!(result.success, "{}", result.message);
    assert!(
        status_lines(&repo_path).is_empty(),
        "{:?}",
        status_lines(&repo_path)
    );
    assert!(!repo_path.join("notes/deep/todo.md").exists());
}

#[test]
fn restoring_never_overwrites_a_file_that_came_back() {
    let (root, repo_path) = create_repository();
    std::fs::write(repo_path.join("scratch.txt"), "first\n").unwrap();
    let DiscardBackupOutcome::Saved(backup) = back_up_discard(
        &repo_path,
        &probe(&root.path().join("b"), Some(vec!["scratch.txt".into()])),
    ) else {
        panic!("se esperaba una copia");
    };
    assert!(backup.patch.is_none());
    assert!(discard_changes_operation(&repo_path, "scratch.txt").success);
    std::fs::write(repo_path.join("scratch.txt"), "second\n").unwrap();

    let result = undo(
        &repo_path,
        &[UndoStep::RestoreFiles {
            files: backup.untracked,
        }],
    );
    assert_eq!(result.code, Code::UndoBlocked);
    assert_eq!(read_file(&repo_path.join("scratch.txt")), "second\n");
}

#[test]
fn a_patch_that_no_longer_matches_is_refused_without_touching_the_file() {
    let (root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "edited\n").unwrap();
    let DiscardBackupOutcome::Saved(backup) = back_up_discard(
        &repo_path,
        &probe(&root.path().join("b"), Some(vec!["file.txt".into()])),
    ) else {
        panic!("se esperaba una copia");
    };
    assert!(discard_changes_operation(&repo_path, "file.txt").success);
    std::fs::write(repo_path.join("file.txt"), "something else\n").unwrap();

    let result = undo(
        &repo_path,
        &[UndoStep::ApplyPatch {
            patch: PatchData::File(backup.patch.unwrap()),
            reverse: false,
        }],
    );
    assert_eq!(result.code, Code::UndoBlocked);
    assert_eq!(read_file(&repo_path.join("file.txt")), "something else\n");
}

#[test]
fn a_discard_backup_over_the_limit_is_not_kept() {
    let (root, repo_path) = create_repository();
    std::fs::write(repo_path.join("big.txt"), "x".repeat(4096)).unwrap();
    let directory = root.path().join("big");
    let outcome = back_up_discard(
        &repo_path,
        &DiscardProbe {
            paths: None,
            directory: directory.clone(),
            limit: 1024,
        },
    );
    assert!(matches!(outcome, DiscardBackupOutcome::TooLarge(bytes) if bytes > 1024));
    assert!(!directory.exists());
    assert!(estimate_discard_backup_size(&repo_path, None) >= 4096);

    // Sin nada que descartar no hay copia.
    run_git(&repo_path, &["clean", "-fdq"]);
    assert_eq!(
        back_up_discard(&repo_path, &probe(&directory, None)),
        DiscardBackupOutcome::Empty
    );
}

#[test]
fn published_upstream_detects_commits_already_on_the_remote() {
    let root = create_temp_root();
    let remote = root.path().join("remote.git");
    init_bare(&remote, true);
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    let base = commit_file(&repo_path, "base\n", "base");
    run_git(&repo_path, &["remote", "add", "origin", path_str(&remote)]);
    let pushed = commit_file(&repo_path, "pushed\n", "pushed");
    run_git(&repo_path, &["push", "-u", "origin", "main"]);
    let local = commit_file(&repo_path, "local\n", "local only");

    // Deshacer el commit local: no toca nada publicado.
    assert_eq!(
        published_upstream(&repo_path, "main", &local, &pushed),
        None
    );
    // Deshacer hasta antes del publicado: sí.
    assert_eq!(
        published_upstream(&repo_path, "main", &local, &base).as_deref(),
        Some("origin/main")
    );
    // Sin upstream no hay nada que avisar.
    run_git(&repo_path, &["checkout", "-b", "solo"]);
    assert_eq!(published_upstream(&repo_path, "solo", &local, &base), None);
}

#[test]
fn a_failure_after_the_first_step_reports_a_partial_undo() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    let sha = head(&repo_path);
    let result = undo(
        &repo_path,
        &[
            UndoStep::Checkout {
                from: HeadTarget::Branch("feature".into()),
                to: HeadTarget::Branch("main".into()),
            },
            // Ya existe: este paso falla tras el checkout.
            UndoStep::CreateBranch {
                name: "feature".into(),
                sha,
                upstream: None,
            },
        ],
    );
    assert!(!result.success);
    assert_eq!(result.code, Code::PartialFailure);
    assert_eq!(result.outcome, Outcome::Partial);
    assert!(
        result.message.contains("stopped halfway"),
        "{}",
        result.message
    );
}
