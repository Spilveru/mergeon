//! Tests de humo del resto de `useGit`: cada función llega al backend con los
//! argumentos en su sitio y devuelve algo con sentido. El comportamiento de
//! cada operación se prueba en `mergeon-git-ops` y `mergeon-git-query`.

use gpui_kit::TestAppContext;
use mergeon_backend::{
    ApplyPatchOptions, CommitLogOptions, CommitOptions, CommitSearchOptions, CompareLimits,
    CompareMode, DiffOptions, InteractiveRebaseOptions, MergeOptions, Outcome, RebaseOptions,
    RebaseTodoEntry, ReflogOptions, RemoteTagOptions, SetIdentityOptions, SubmoduleUpdateOptions,
};
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo, git_in};
use crate::StructuredOutcome;

/// Llama a un método del modelo y espera su resultado.
macro_rules! git {
    ($app:expr, $cx:expr, |$model:ident, $inner:ident| $call:expr) => {
        $app.model.update($cx, |$model, $inner| $call).await
    };
}

#[gpui_kit::test]
async fn repository_and_history_reads(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let first = repo.head();
    let second = repo.commit_file("src/lib.rs", "pub fn demo() {}\n", "Add the library");
    repo.git(&["tag", "-a", "v1.0.0", "-m", "First release"]);
    let path = repo.path_string();

    let inspection = git!(app, cx, |model, cx| model.inspect_repository(&path, cx));
    assert!(inspection.valid);
    assert!(git!(app, cx, |model, cx| model.is_valid_repository(&path, cx)));
    assert!(!git!(app, cx, |model, cx| model
        .is_valid_repository("/does/not/exist", cx)));

    let commits = git!(app, cx, |model, cx| model.get_commits(
        &path,
        CommitLogOptions::default(),
        cx
    ))
    .unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].full_hash, second);

    let found = git!(app, cx, |model, cx| model.search_commits(
        &path,
        CommitSearchOptions {
            text: Some("library".to_string()),
            ..Default::default()
        },
        cx
    ))
    .unwrap();
    assert_eq!(found.len(), 1);

    assert_eq!(
        git!(app, cx, |model, cx| model.resolve_ref(&path, "HEAD", cx)).unwrap(),
        second
    );
    let reflog = git!(app, cx, |model, cx| model.get_reflog(
        &path,
        ReflogOptions::default(),
        cx
    ))
    .unwrap();
    assert!(!reflog.is_empty());

    let files = git!(app, cx, |model, cx| model
        .get_commit_files(&path, &second, cx))
    .unwrap();
    assert_eq!(files[0].path, "src/lib.rs");
    let diff = git!(app, cx, |model, cx| model.get_commit_diff(
        &path,
        &second,
        "src/lib.rs",
        DiffOptions::default(),
        cx
    ))
    .unwrap();
    assert!(diff.diff.contains("pub fn demo"));

    let tree = git!(app, cx, |model, cx| model
        .get_tree_entries(&path, &second, "", cx))
    .unwrap();
    assert!(tree.entries.iter().any(|entry| entry.name == "src"));
    let file = git!(app, cx, |model, cx| model.get_file_at_commit(
        &path,
        &first,
        "README.md",
        cx
    ))
    .unwrap();
    assert_eq!(file.content, "# Demo\n");

    let blame = git!(app, cx, |model, cx| model.blame(&path, "README.md", cx)).unwrap();
    assert_eq!(blame.lines.len(), 1);
    assert_eq!(
        git!(app, cx, |model, cx| model.read_file(&path, "README.md", cx)).unwrap(),
        "# Demo\n"
    );

    assert_eq!(
        git!(app, cx, |model, cx| model.get_tags(&path, cx)).unwrap(),
        ["v1.0.0"]
    );
    let details = git!(app, cx, |model, cx| model
        .get_tag_details(&path, "v1.0.0", cx))
    .unwrap();
    assert!(details.message.contains("First release"));

    let identity = git!(app, cx, |model, cx| model.get_identity(&path, cx)).unwrap();
    assert_eq!(identity.local.name.as_deref(), Some("Test User"));
    let state = git!(app, cx, |model, cx| model.get_operation_state(&path, cx)).unwrap();
    assert!(!state.in_progress);
    assert!(
        git!(app, cx, |model, cx| model.get_submodules(&path, cx))
            .unwrap()
            .is_empty()
    );
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn working_tree_mutations(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.write("build.log", "noise\n");
    let path = repo.path_string();

    let ignored = git!(app, cx, |model, cx| model.add_to_ignore(
        &path,
        "build.log",
        cx
    ));
    assert!(ignored.success(), "{}", ignored.message());
    let again = git!(app, cx, |model, cx| model.add_to_ignore(
        &path,
        "build.log",
        cx
    ));
    assert_eq!(again.code(), "ALREADY_IGNORED");
    assert_eq!(app.host.messages(NotifyType::Info).len(), 1);

    let staged = git!(app, cx, |model, cx| model.stage_all(&path, cx));
    assert!(staged.success());
    let status = git!(app, cx, |model, cx| model.get_status(&path, cx)).unwrap();
    assert_eq!(status.staged.len(), 3);

    let unstaged = git!(app, cx, |model, cx| model.unstage(&path, "b.txt", cx));
    assert!(unstaged.success());
    let diff = git!(app, cx, |model, cx| model.get_diff(
        &path,
        "a.txt",
        true,
        false,
        DiffOptions::default(),
        cx
    ))
    .unwrap();
    assert!(diff.diff.contains("+a"));
    assert!(git!(app, cx, |model, cx| model.unstage_all(&path, cx)).success());

    let changes = git!(app, cx, |model, cx| model
        .has_uncommitted_changes(&path, cx))
    .unwrap();
    assert!(changes.has_changes);

    let discarded = git!(app, cx, |model, cx| model
        .discard_changes(&path, "b.txt", cx));
    assert!(discarded.success(), "{}", discarded.message());
    assert!(!repo.path.join("b.txt").exists());
    let discarded = git!(app, cx, |model, cx| model.discard_all(&path, None, cx));
    assert!(discarded.succeeded_or_noop());
    assert!(!repo.path.join("a.txt").exists());

    // Un bloque del diff, al índice y de vuelta.
    repo.commit_file(".gitignore", "build.log\n", "Ignore logs");
    repo.write("README.md", "# Demo\n\nmore\n");
    let patch = repo.git(&["diff", "--", "README.md"]);
    let applied = git!(app, cx, |model, cx| model.apply_patch(
        &path,
        &patch,
        ApplyPatchOptions::default(),
        cx
    ));
    assert!(applied.success(), "{}", applied.message());
    assert_eq!(applied.operation(), Some("stage-hunk"));
    assert!(
        repo.git(&["diff", "--cached", "--name-only"])
            .contains("README.md")
    );
    let reverted = git!(app, cx, |model, cx| model.apply_patch(
        &path,
        &patch,
        ApplyPatchOptions {
            reverse: true,
            ..Default::default()
        },
        cx
    ));
    assert_eq!(reverted.operation(), Some("unstage-hunk"));
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]).trim(), "");
    // Al revés y sobre el working tree es descartar ese bloque.
    let discarded = git!(app, cx, |model, cx| model.apply_patch(
        &path,
        &patch,
        ApplyPatchOptions {
            reverse: true,
            worktree: true,
            ..Default::default()
        },
        cx
    ));
    assert!(discarded.success(), "{}", discarded.message());
    assert_eq!(discarded.operation(), Some("discard-hunk"));
    assert_eq!(repo.git(&["diff", "--name-only"]).trim(), "");
    repo.write("README.md", "# Demo\n\nmore\n");

    // Un commit silencioso no avisa; enmendarlo es otra operación.
    app.host.clear_toasts();
    git!(app, cx, |model, cx| model.stage(&path, "README.md", cx));
    app.host.clear_toasts();
    let committed = git!(app, cx, |model, cx| model.commit(
        &path,
        "Extend the README",
        CommitOptions::default(),
        true,
        cx
    ));
    assert!(committed.success());
    assert!(app.host.toasts().is_empty());
    let amended = git!(app, cx, |model, cx| model.commit(
        &path,
        "Extend the README a bit",
        CommitOptions {
            amend: true,
            ..Default::default()
        },
        false,
        cx
    ));
    assert!(amended.success(), "{}", amended.message());
    assert_eq!(repo.log_subjects()[0], "Extend the README a bit");

    let identity = git!(app, cx, |model, cx| model.set_identity(
        &path,
        SetIdentityOptions {
            scope: "local".to_string(),
            name: Some("Another User".to_string()),
            email: Some("another@example.com".to_string()),
        },
        cx
    ));
    assert!(identity.success, "{}", identity.message);
    assert_eq!(repo.git(&["config", "user.name"]).trim(), "Another User");
}

#[gpui_kit::test]
async fn branches_tags_and_stashes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let path = repo.path_string();

    let created = git!(app, cx, |model, cx| model.create_branch(
        &path,
        "feature",
        Some("main"),
        false,
        cx
    ));
    assert!(created.success(), "{}", created.message());
    assert_eq!(repo.current_branch(), "main");
    let renamed = git!(app, cx, |model, cx| model
        .rename_branch(&path, "feature", "topic", cx));
    assert!(renamed.success(), "{}", renamed.message());
    let checked_out = git!(app, cx, |model, cx| model.checkout(&path, "topic", cx));
    assert!(checked_out.success());
    assert_eq!(repo.current_branch(), "topic");

    let branches = git!(app, cx, |model, cx| model.get_branches(&path, cx)).unwrap();
    assert_eq!(branches.current, "topic");
    assert!(branches.all.contains(&"remotes/origin/main".to_string()));

    repo.commit_file("topic.txt", "topic\n", "Topic work");
    let comparison = git!(app, cx, |model, cx| model
        .compare_branches(&path, "topic", "main", cx))
    .unwrap();
    assert_eq!(comparison.ahead + comparison.behind, 1);
    let revisions = git!(app, cx, |model, cx| model.compare_revisions(
        &path,
        "main",
        "topic",
        CompareMode::SinceDivergence,
        CompareLimits::default(),
        cx
    ))
    .unwrap();
    assert_eq!(revisions.ahead, 1);
    assert_eq!(revisions.files.files[0].path, "topic.txt");
    let direct = git!(app, cx, |model, cx| model.get_comparison_files(
        &path,
        &revisions.revisions,
        CompareMode::Direct,
        10,
        cx
    ))
    .unwrap();
    assert_eq!(direct.total, 1);
    let diff = git!(app, cx, |model, cx| model.get_comparison_diff(
        &path,
        &revisions.revisions,
        CompareMode::SinceDivergence,
        &revisions.files.files[0],
        DiffOptions::default(),
        cx
    ))
    .unwrap();
    assert!(diff.diff.contains("+topic"), "{}", diff.diff);
    let conflicts = git!(app, cx, |model, cx| model
        .check_merge_conflicts(&path, "topic", "main", cx))
    .unwrap();
    assert!(!conflicts.has_conflicts);

    git!(app, cx, |model, cx| model.checkout(&path, "main", cx));
    let merged = git!(app, cx, |model, cx| model.merge(
        &path,
        "topic",
        MergeOptions::default(),
        false,
        cx
    ));
    assert!(merged.success(), "{}", merged.message());
    assert!(repo.path.join("topic.txt").exists());
    let deleted = git!(app, cx, |model, cx| model.delete_branch(&path, "topic", cx));
    assert!(deleted.success(), "{}", deleted.message());

    // Tags: crear, publicar, borrar en el remoto y en local.
    let tagged = git!(app, cx, |model, cx| model.create_tag(
        &path,
        "v1.0.0",
        None,
        Some("First release"),
        cx
    ));
    assert!(tagged.success(), "{}", tagged.message());
    let duplicate = git!(app, cx, |model, cx| model
        .create_tag(&path, "v1.0.0", None, None, cx));
    assert_eq!(duplicate.code(), "TAG_ALREADY_EXISTS");
    let pushed = git!(app, cx, |model, cx| model.push_tag(
        &path,
        "v1.0.0",
        RemoteTagOptions::default(),
        cx
    ));
    assert!(pushed.success(), "{}", pushed.message());
    assert!(git_in(&origin, &["tag"]).contains("v1.0.0"));
    let removed = git!(app, cx, |model, cx| model.delete_remote_tag(
        &path,
        "v1.0.0",
        RemoteTagOptions::default(),
        cx
    ));
    assert!(removed.success(), "{}", removed.message());
    assert!(git!(app, cx, |model, cx| model.delete_tag(&path, "v1.0.0", cx)).success());
    assert_eq!(repo.git(&["tag"]).trim(), "");

    // Stashes: guardar, mirar, aplicar, sacar y tirar.
    repo.write("README.md", "# Demo\n\nstashed\n");
    let stashed = git!(app, cx, |model, cx| model.stash(&path, Some("wip"), cx));
    assert!(stashed.success(), "{}", stashed.message());
    let stashes = git!(app, cx, |model, cx| model.get_stashes(&path, cx)).unwrap();
    assert_eq!(stashes.len(), 1);
    let files = git!(app, cx, |model, cx| model.get_stash_files(
        &path,
        "stash@{0}",
        cx
    ))
    .unwrap();
    assert_eq!(files[0].path, "README.md");
    let diff = git!(app, cx, |model, cx| model.get_stash_diff(
        &path,
        "stash@{0}",
        "README.md",
        DiffOptions::default(),
        cx
    ))
    .unwrap();
    assert!(diff.diff.contains("+stashed"));
    assert!(
        git!(app, cx, |model, cx| model.apply_stash(
            &path,
            "stash@{0}",
            cx
        ))
        .success()
    );
    repo.git(&["checkout", "--", "README.md"]);
    assert!(git!(app, cx, |model, cx| model.pop_stash(&path, "stash@{0}", cx)).success());
    repo.git(&["stash", "push", "-m", "again"]);
    let dropped = git!(app, cx, |model, cx| model.drop_stash(
        &path,
        "stash@{0}",
        cx
    ));
    assert!(dropped.success(), "{}", dropped.message());

    // Ramas remotas.
    let colleague = repo.colleague(&origin);
    git_in(&colleague.path, &["checkout", "-b", "shared"]);
    git_in(&colleague.path, &["push", "origin", "shared"]);
    repo.git(&["fetch", "origin"]);
    let tracked = git!(app, cx, |model, cx| model.checkout_remote_branch(
        &path,
        "origin/shared",
        cx
    ));
    assert!(tracked.success(), "{}", tracked.message());
    assert_eq!(repo.current_branch(), "shared");
    git!(app, cx, |model, cx| model.checkout(&path, "main", cx));
    let removed = git!(app, cx, |model, cx| model
        .delete_remote_branch(&path, "origin", "shared", cx));
    assert!(removed.success(), "{}", removed.message());
    assert!(!git_in(&origin, &["branch"]).contains("shared"));
}

#[gpui_kit::test]
async fn history_rewriting_and_remotes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let base = repo.head();
    let second = repo.commit_file("a.txt", "a\n", "Add a");
    let path = repo.path_string();

    let reverted = git!(app, cx, |model, cx| model.revert(&path, &second, cx));
    assert!(reverted.success(), "{}", reverted.message());
    assert!(!repo.path.join("a.txt").exists());

    let reset = git!(app, cx, |model, cx| model.reset(&path, &second, "hard", cx));
    assert!(reset.success(), "{}", reset.message());
    assert_eq!(repo.head(), second);

    repo.git(&["checkout", "-b", "topic", &base]);
    let picked = git!(app, cx, |model, cx| model.cherry_pick(&path, &second, cx));
    assert!(picked.success(), "{}", picked.message());
    assert!(repo.path.join("a.txt").exists());

    repo.commit_file("b.txt", "b\n", "Add b");
    let rebased = git!(app, cx, |model, cx| model.rebase(
        &path,
        "main",
        RebaseOptions::default(),
        cx
    ));
    assert!(rebased.succeeded_or_noop(), "{}", rebased.message());

    let head = repo.head();
    let reworded = git!(app, cx, |model, cx| model.interactive_rebase(
        &path,
        InteractiveRebaseOptions {
            base_commit: Some(second.clone()),
            todo: vec![RebaseTodoEntry {
                action: "reword".to_string(),
                hash: head.clone(),
                message: Some("Add b, reworded".to_string()),
            }],
        },
        cx
    ));
    assert!(reworded.success(), "{}", reworded.message());
    assert_eq!(repo.log_subjects()[0], "Add b, reworded");

    // Sin operación en curso, continuar no hace nada y avisa.
    app.host.clear_toasts();
    let sequence = git!(app, cx, |model, cx| model
        .sequence_action(&path, "continue", cx));
    assert_eq!(sequence.code(), "NO_OPERATION_IN_PROGRESS");
    assert_eq!(app.host.messages(NotifyType::Warning).len(), 1);

    // Remotos.
    let added = git!(app, cx, |model, cx| model.add_remote(
        &path,
        "upstream",
        "https://example.invalid/a.git",
        true,
        cx
    ));
    assert!(added.success());
    let renamed = git!(app, cx, |model, cx| model
        .rename_remote(&path, "upstream", "fork", cx));
    assert!(renamed.success(), "{}", renamed.message());
    let updated = git!(app, cx, |model, cx| model.set_remote_url(
        &path,
        "fork",
        "https://example.invalid/b.git",
        cx
    ));
    assert!(updated.success(), "{}", updated.message());
    let remotes = git!(app, cx, |model, cx| model.get_remotes(&path, cx)).unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "fork");
    assert_eq!(remotes[0].refs.fetch, "https://example.invalid/b.git");
    let removed = git!(app, cx, |model, cx| model.remove_remote(&path, "fork", cx));
    assert!(removed.success(), "{}", removed.message());

    let bare = repo.root().join("reachable.git");
    std::fs::create_dir(&bare).unwrap();
    git_in(&bare, &["init", "--bare", "-b", "main"]);
    let connection = git!(app, cx, |model, cx| model.test_remote_connection(
        &path,
        &bare.to_string_lossy(),
        cx
    ));
    assert!(connection.success, "{}", connection.message);

    let submodules = git!(app, cx, |model, cx| model.update_submodules(
        &path,
        SubmoduleUpdateOptions::default(),
        cx
    ));
    assert!(submodules.succeeded_or_noop(), "{}", submodules.message());

    let conflict = git!(app, cx, |model, cx| model.resolve_conflict(
        &path,
        "a.txt",
        "resolved\n",
        cx
    ));
    assert_ne!(conflict.outcome(), Some(Outcome::Completed));

    // La rama actual se adelanta hasta su upstream.
    let origin = repo.publish_to_origin();
    let colleague = repo.colleague(&origin);
    let remote_head = colleague.commit_and_push("c.txt", "c\n", "Remote change");
    repo.git(&["fetch", "origin"]);
    let blocked = git!(app, cx, |model, cx| model.fast_forward_branch(
        &path,
        "main",
        "origin/main",
        cx
    ));
    assert!(!blocked.success());
    repo.git(&["checkout", "main"]);
    let forwarded = git!(app, cx, |model, cx| model.fast_forward_branch(
        &path,
        "main",
        "origin/main",
        cx
    ));
    assert!(forwarded.success(), "{}", forwarded.message());
    assert_eq!(repo.git(&["rev-parse", "main"]).trim(), remote_head);

    let dropped = git!(app, cx, |model, cx| model.rebase_action(
        &path,
        "drop",
        &repo.head(),
        Default::default(),
        cx
    ));
    assert!(dropped.operation().is_some());
    let tag = git!(app, cx, |model, cx| model.checkout_tag(
        &path,
        "missing-tag",
        cx
    ));
    assert!(!tag.success());
}
