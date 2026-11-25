//! merge, rebase, revert, cherry-pick, resolución de conflictos y acciones de secuencia.

use mergeon_gitcmd::{
    OperationContext, OperationType, get_structured_status, run_with_operation_context,
};
use std::time::Duration;

use super::support::*;
use crate::{
    Code, ConflictResolution, ConflictSide, MergeOptions, Operation, Outcome, RebaseOptions,
    cherry_pick_operation, merge_operation, rebase_operation, resolve_conflict_operation,
    resolve_conflict_with_operation, revert_operation, sequence_action_operation,
};

#[test]
fn returns_conflict_for_merge_conflicts() {
    let fixture = create_diverged_repository();

    let result = merge_operation(&fixture.repo_path, "feature", &MergeOptions::default());

    assert!(!result.success);
    assert_eq!(result.code, Code::Conflict);
    assert_eq!(result.operation, Operation::Merge);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert_eq!(
        result.message,
        "Merge stopped because Git detected conflicts."
    );
    assert_eq!(result.branch.as_deref(), Some("main"));
    assert!(result.conflict_files.contains(&"file.txt".to_string()));
    assert_eq!(
        result.operation_state.as_ref().and_then(|state| state.kind),
        Some(OperationType::Merge)
    );
    assert!(result.conflict_state.pending_count > 0);
    assert_eq!(
        result.conflict_state.operation_type,
        Some(OperationType::Merge)
    );
    assert!(result.status.is_some());
}

#[test]
fn summarizes_fast_forward_output_instead_of_exposing_git_file_stats() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file(&repo_path, "feature\n", "feature change");
    run_git(&repo_path, &["checkout", "main"]);

    let result = merge_operation(&repo_path, "feature", &MergeOptions::default());

    assert!(result.success);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.message, "Successfully merged feature.");
    // Sin conflicto no se adjunta el estado completo.
    assert_eq!(result.status, None);
    assert!(result.conflict_files.is_empty());
    assert_eq!(
        result.operation_state.map(|state| state.in_progress),
        Some(false)
    );

    let again = merge_operation(&repo_path, "feature", &MergeOptions::default());
    assert!(again.success);
    assert_eq!(again.outcome, Outcome::Noop);
    assert_eq!(again.code, Code::AlreadyUpToDate);
    assert_eq!(again.message, "main is already up to date with feature.");
}

#[test]
fn merge_honours_target_branch_no_fast_forward_and_message() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file_at(&repo_path, "feature\n", "feature change", "feature.txt");

    // Se lanza desde `feature`: la operación cambia antes a la rama destino.
    let result = merge_operation(
        &repo_path,
        "feature",
        &MergeOptions {
            target_branch: Some("main".to_string()),
            no_fast_forward: true,
            message: Some("Merge the feature".to_string()),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.branch.as_deref(), Some("main"));
    assert_eq!(
        get_structured_status(&repo_path)
            .unwrap()
            .current
            .as_deref(),
        Some("main")
    );
    assert_eq!(log_subjects(&repo_path)[0], "Merge the feature");
    let parents = run_git(&repo_path, &["rev-list", "--parents", "-1", "HEAD"]);
    assert_eq!(parents.trim().split(' ').count(), 3);
}

#[test]
fn merging_an_unknown_branch_is_a_git_error() {
    let (_root, repo_path) = create_repository();

    let result = merge_operation(&repo_path, "no-such-branch", &MergeOptions::default());

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
    assert!(
        result.message.contains("no-such-branch"),
        "{}",
        result.message
    );
    assert_eq!(result.branch.as_deref(), Some("main"));
}

#[test]
fn returns_conflict_for_rebase_conflicts() {
    let fixture = create_diverged_repository();
    run_git(&fixture.repo_path, &["checkout", "feature"]);

    let result = rebase_operation(&fixture.repo_path, "main", &RebaseOptions::default());

    assert!(!result.success);
    assert_eq!(result.code, Code::Conflict);
    assert_eq!(result.operation, Operation::Rebase);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert_eq!(
        result.message,
        "rebase stopped because Git detected conflicts."
    );
    assert_eq!(result.branch.as_deref(), Some("feature"));
    assert!(result.conflict_files.contains(&"file.txt".to_string()));
    assert_eq!(
        result.operation_state.as_ref().and_then(|state| state.kind),
        Some(OperationType::Rebase)
    );
    assert!(result.conflict_state.pending_count > 0);
    assert_eq!(
        result.conflict_state.operation_type,
        Some(OperationType::Rebase)
    );
}

#[test]
fn rebases_cleanly_and_reports_when_there_is_nothing_to_do() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file_at(&repo_path, "feature\n", "feature change", "feature.txt");
    run_git(&repo_path, &["checkout", "main"]);
    commit_file_at(&repo_path, "main\n", "main change", "main.txt");
    run_git(&repo_path, &["checkout", "feature"]);

    let result = rebase_operation(&repo_path, "main", &RebaseOptions::default());
    assert!(result.success, "{}", result.message);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.message, "Successfully rebased onto main.");
    assert_eq!(
        log_subjects(&repo_path),
        ["feature change", "main change", "base"]
    );

    let again = rebase_operation(&repo_path, "main", &RebaseOptions::default());
    assert!(again.success);
    assert_eq!(again.outcome, Outcome::Noop);
    assert_eq!(again.code, Code::AlreadyUpToDate);
    assert_eq!(again.message, "feature is already up to date with main.");
}

#[test]
fn rebases_the_target_branch_instead_of_head() {
    // Arrastrar `feature` sobre `topic` estando en `main`: se rebasa
    // `topic` sobre `feature`; `main` no se toca.
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "topic"]);
    commit_file_at(&repo_path, "topic\n", "topic change", "topic.txt");
    run_git(&repo_path, &["checkout", "main"]);
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file_at(&repo_path, "feature\n", "feature change", "feature.txt");
    run_git(&repo_path, &["checkout", "main"]);
    commit_file_at(&repo_path, "main\n", "main change", "main.txt");
    let main_before = head(&repo_path);

    let result = rebase_operation(
        &repo_path,
        "feature",
        &RebaseOptions {
            target_branch: Some("topic".into()),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.branch.as_deref(), Some("topic"));
    assert_eq!(
        run_git(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "topic"
    );
    assert_eq!(
        log_subjects(&repo_path),
        ["topic change", "feature change", "base"]
    );
    assert_eq!(
        run_git(&repo_path, &["rev-parse", "main"]).trim(),
        main_before
    );
}

#[test]
fn rebase_onto_target_with_target_already_checked_out_behaves_like_plain_rebase() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file_at(&repo_path, "feature\n", "feature change", "feature.txt");
    run_git(&repo_path, &["checkout", "main"]);
    commit_file_at(&repo_path, "main\n", "main change", "main.txt");
    run_git(&repo_path, &["checkout", "feature"]);

    let result = rebase_operation(
        &repo_path,
        "main",
        &RebaseOptions {
            target_branch: Some("feature".into()),
            ..Default::default()
        },
    );
    assert!(result.success, "{}", result.message);
    assert_eq!(result.branch.as_deref(), Some("feature"));
    assert_eq!(
        log_subjects(&repo_path),
        ["feature change", "main change", "base"]
    );
}

#[test]
fn rebase_onto_target_is_blocked_cleanly_when_checkout_fails() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "topic"]);
    commit_file_at(&repo_path, "topic\n", "topic edits file", "file.txt");
    run_git(&repo_path, &["checkout", "main"]);
    let main_before = head(&repo_path);
    // Un cambio sin confirmar en el mismo archivo impide el checkout.
    std::fs::write(repo_path.join("file.txt"), "dirty\n").unwrap();

    let result = rebase_operation(
        &repo_path,
        "main",
        &RebaseOptions {
            target_branch: Some("topic".into()),
            ..Default::default()
        },
    );

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(
        run_git(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );
    assert_eq!(head(&repo_path), main_before);
    assert_eq!(read_file(&repo_path.join("file.txt")), "dirty\n");
}

#[test]
fn returns_conflict_for_revert_conflicts() {
    let (_root, repo_path) = create_repository();
    let revert_target = commit_file(&repo_path, "main\n", "target change");
    commit_file(&repo_path, "other\n", "other change");

    let result = revert_operation(&repo_path, &revert_target);

    assert!(!result.success);
    assert_eq!(result.code, Code::Conflict);
    assert!(result.conflict_files.contains(&"file.txt".to_string()));
    assert_eq!(
        result.operation_state.and_then(|state| state.kind),
        Some(OperationType::Revert)
    );
}

#[test]
fn reverts_and_cherry_picks_without_conflicts() {
    let (_root, repo_path) = create_repository();
    let added = commit_file_at(&repo_path, "extra\n", "add extra", "extra.txt");

    let reverted = revert_operation(&repo_path, &added);
    assert!(reverted.success, "{}", reverted.message);
    assert_eq!(reverted.operation, Operation::Revert);
    assert_eq!(reverted.code, Code::Ok);
    assert_eq!(reverted.branch.as_deref(), Some("main"));
    assert!(!repo_path.join("extra.txt").exists());

    let picked = cherry_pick_operation(&repo_path, &added);
    assert!(picked.success, "{}", picked.message);
    assert_eq!(picked.operation, Operation::CherryPick);
    assert_eq!(picked.code, Code::Ok);
    assert_eq!(read_file(&repo_path.join("extra.txt")), "extra\n");
    assert_eq!(log_subjects(&repo_path)[0], "add extra");
}

#[test]
fn returns_conflict_for_cherry_pick_conflicts() {
    let fixture = create_diverged_repository();

    let result = cherry_pick_operation(&fixture.repo_path, &fixture.feature_commit);

    assert!(!result.success);
    assert_eq!(result.code, Code::Conflict);
    assert_eq!(result.operation, Operation::CherryPick);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert!(result.conflict_files.contains(&"file.txt".to_string()));
    assert_eq!(
        result.operation_state.as_ref().and_then(|state| state.kind),
        Some(OperationType::CherryPick)
    );
    assert!(result.conflict_state.pending_count > 0);
    assert_eq!(
        result.conflict_state.operation_type,
        Some(OperationType::CherryPick)
    );
}

#[test]
fn resolve_conflict_stages_the_resolved_file_and_marks_ready_to_continue_on_the_last_conflict() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    merge_operation(repo_path, "feature", &MergeOptions::default());

    let result = resolve_conflict_operation(repo_path, "file.txt", "merged\n");
    let file_content = std::fs::read_to_string(repo_path.join("file.txt")).unwrap();

    assert!(result.success);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.message, "Resolved file.txt.");
    assert_eq!(result.file_path, "file.txt");
    assert_eq!(file_content, "merged\n");
    assert!(result.status.as_ref().unwrap().conflicted.is_empty());
    assert_eq!(result.conflict_state.pending_count, 0);
    assert!(result.conflict_state.ready_to_continue);
    assert_eq!(
        result.operation_state.and_then(|state| state.kind),
        Some(OperationType::Merge)
    );
}

/// `main` borra `file.txt` y `feature` lo modifica: el merge para con un
/// conflicto de borrado/modificación (sin etapa `:2:`).
fn delete_modify_conflict() -> (tempfile::TempDir, std::path::PathBuf) {
    let (root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file(&repo_path, "changed on feature\n", "feature change");
    run_git(&repo_path, &["checkout", "main"]);
    run_git(&repo_path, &["rm", "-q", "file.txt"]);
    run_git(&repo_path, &["commit", "-m", "main removes"]);
    let result = merge_operation(&repo_path, "feature", &MergeOptions::default());
    assert_eq!(result.code, Code::Conflict);
    (root, repo_path)
}

#[test]
fn resolve_conflict_writes_content_byte_for_byte_with_crlf() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    merge_operation(repo_path, "feature", &MergeOptions::default());

    let result = resolve_conflict_with_operation(
        repo_path,
        "file.txt",
        &ConflictResolution::Content("one\r\ntwo\r\n".to_string()),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(
        std::fs::read(repo_path.join("file.txt")).unwrap(),
        b"one\r\ntwo\r\n"
    );
    assert!(result.conflict_state.ready_to_continue);
}

#[test]
fn resolve_conflict_can_keep_one_stage_whole() {
    for (side, expected) in [
        (ConflictSide::Ours, "main\n"),
        (ConflictSide::Theirs, "feature\n"),
        (ConflictSide::Base, "base\n"),
    ] {
        let fixture = create_diverged_repository();
        let repo_path = &fixture.repo_path;
        merge_operation(repo_path, "feature", &MergeOptions::default());

        let result =
            resolve_conflict_with_operation(repo_path, "file.txt", &ConflictResolution::Keep(side));

        assert!(result.success, "{side:?}: {}", result.message);
        assert_eq!(read_file(&repo_path.join("file.txt")), expected, "{side:?}");
        assert!(result.status.as_ref().unwrap().conflicted.is_empty());
        assert_eq!(
            run_git(repo_path, &["show", ":file.txt"]),
            expected,
            "{side:?}: the index holds the chosen version"
        );
    }
}

#[test]
fn resolve_conflict_keeps_a_binary_version_exactly() {
    let (_root, repo_path) = create_repository();
    let write = |bytes: &[u8]| std::fs::write(repo_path.join("logo.bin"), bytes).unwrap();
    write(b"\x89PNG\0base");
    run_git(&repo_path, &["add", "logo.bin"]);
    run_git(&repo_path, &["commit", "-m", "logo"]);
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    write(b"\x89PNG\0\xfftheirs");
    run_git(&repo_path, &["commit", "-am", "feature logo"]);
    run_git(&repo_path, &["checkout", "main"]);
    write(b"\x89PNG\0ours");
    run_git(&repo_path, &["commit", "-am", "main logo"]);
    merge_operation(&repo_path, "feature", &MergeOptions::default());

    let result = resolve_conflict_with_operation(
        &repo_path,
        "logo.bin",
        &ConflictResolution::Keep(ConflictSide::Theirs),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(
        std::fs::read(repo_path.join("logo.bin")).unwrap(),
        b"\x89PNG\0\xfftheirs"
    );
    assert_eq!(result.conflict_state.pending_count, 0);
}

#[test]
fn resolve_conflict_deletes_or_keeps_a_file_deleted_on_one_side() {
    let (_root, repo_path) = delete_modify_conflict();
    let result =
        resolve_conflict_with_operation(&repo_path, "file.txt", &ConflictResolution::Delete);
    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Resolved file.txt by deleting it.");
    assert!(!repo_path.join("file.txt").exists());
    assert!(run_git(&repo_path, &["ls-files", "--", "file.txt"]).is_empty());
    assert!(result.conflict_state.ready_to_continue);

    let (_root, repo_path) = delete_modify_conflict();
    let result = resolve_conflict_with_operation(
        &repo_path,
        "file.txt",
        &ConflictResolution::Keep(ConflictSide::Theirs),
    );
    assert!(result.success, "{}", result.message);
    assert_eq!(
        read_file(&repo_path.join("file.txt")),
        "changed on feature\n"
    );
    assert_eq!(
        run_git(&repo_path, &["ls-files", "--", "file.txt"]),
        "file.txt\n"
    );
    assert!(result.conflict_state.ready_to_continue);
}

#[test]
fn keeping_a_missing_stage_fails_without_touching_the_conflict() {
    let (_root, repo_path) = delete_modify_conflict();
    let result = resolve_conflict_with_operation(
        &repo_path,
        "file.txt",
        &ConflictResolution::Keep(ConflictSide::Ours),
    );
    assert!(!result.success);
    assert_eq!(result.code, Code::GitError);
    assert_eq!(result.conflict_state.pending_count, 1);
}

#[test]
fn resolve_conflict_reports_a_file_it_cannot_write() {
    let (_root, repo_path) = create_repository();

    let result = resolve_conflict_operation(&repo_path, "missing-dir/file.txt", "content\n");

    assert!(!result.success);
    assert_eq!(result.code, Code::WriteFailed);
    assert_eq!(result.file_path, "missing-dir/file.txt");
    assert_eq!(result.status, None);
    assert_eq!(result.operation_state, None);
    assert!(!result.conflict_state.in_conflict);
}

#[test]
fn returns_no_operation_in_progress_when_continuing_without_an_operation() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    commit_file(&repo_path, "hello\n", "initial commit");

    let result = sequence_action_operation(&repo_path, "continue");

    assert!(!result.success);
    assert_eq!(result.code, Code::NoOperationInProgress);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.operation, None);
    assert_eq!(result.action, "continue");
}

#[test]
fn aborts_a_merge_conflict_and_clears_operation_state() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    merge_operation(repo_path, "feature", &MergeOptions::default());

    let result = sequence_action_operation(repo_path, "abort");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.operation, Some(OperationType::Merge));
    assert_eq!(result.message, "Abort completed for merge.");
    let status = result.status.unwrap();
    assert!(!status.operation_state.in_progress);
    assert_eq!(status.conflict_state.pending_count, 0);
}

#[test]
fn skips_a_conflicted_rebase_step() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    run_git(repo_path, &["checkout", "feature"]);
    rebase_operation(repo_path, "main", &RebaseOptions::default());

    let result = sequence_action_operation(repo_path, "skip");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.operation, Some(OperationType::Rebase));
    assert!(!result.status.unwrap().operation_state.in_progress);
}

#[test]
fn skip_is_only_supported_for_rebase() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    merge_operation(repo_path, "feature", &MergeOptions::default());

    let result = sequence_action_operation(repo_path, "skip");

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::ActionNotSupported);
    assert_eq!(result.message, "Skip is not supported for merge.");
    assert_eq!(result.operation, Some(OperationType::Merge));
}

#[test]
fn returns_conflicts_remain_when_continuing_with_unresolved_conflicts() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    merge_operation(repo_path, "feature", &MergeOptions::default());

    let result = sequence_action_operation(repo_path, "continue");

    assert!(!result.success);
    assert_eq!(result.code, Code::ConflictsRemain);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(
        result.message,
        "Resolve and stage all conflicted files before continuing merge."
    );
    assert!(result.status.unwrap().conflict_state.pending_count > 0);
}

// `rebase --continue` con conflictos sin resolver sale con 1 y lo explica por
// stdout, sin stderr: con el criterio de `git.raw` la app Electron lo daba por
// completado.
#[test]
fn continuing_a_rebase_with_unresolved_conflicts_is_blocked() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    run_git(repo_path, &["checkout", "feature"]);
    rebase_operation(repo_path, "main", &RebaseOptions::default());

    let result = sequence_action_operation(repo_path, "continue");

    assert!(!result.success, "{}", result.message);
    assert_eq!(result.operation, Some(OperationType::Rebase));
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::ConflictsRemain);
    assert_eq!(
        result.message,
        "Resolve and stage all conflicted files before continuing rebase."
    );
    let status = result.status.unwrap();
    assert!(status.operation_state.in_progress);
    assert_eq!(status.conflict_state.pending_files, ["file.txt"]);

    // Ya resuelto, continuar sí termina el rebase.
    resolve_conflict_operation(repo_path, "file.txt", "both\n");
    let result = sequence_action_operation(repo_path, "continue");
    assert!(result.success, "{}", result.message);
    assert!(!result.status.unwrap().operation_state.in_progress);
    assert_eq!(
        log_subjects(repo_path),
        ["feature change", "main change", "base"]
    );
}

#[test]
fn continues_a_resolved_merge_without_opening_an_editor() {
    let fixture = create_diverged_repository();
    let repo_path = &fixture.repo_path;
    merge_operation(repo_path, "feature", &MergeOptions::default());
    resolve_conflict_operation(repo_path, "file.txt", "merged\n");

    let result = sequence_action_operation(repo_path, "continue");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    let status = result.status.unwrap();
    assert!(!status.operation_state.in_progress);
    assert!(status.is_clean);
    // El commit de merge conserva el mensaje que git dejó prellenado.
    assert!(log_subjects(repo_path)[0].starts_with("Merge branch 'feature'"));
}

#[test]
fn a_cancelled_merge_reports_the_control_code() {
    let fixture = create_diverged_repository();

    let context = OperationContext::new(None, Duration::from_secs(30));
    context.cancel();
    let result = run_with_operation_context(context, || {
        merge_operation(&fixture.repo_path, "feature", &MergeOptions::default())
    });

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Cancelled);
    assert_eq!(result.code, Code::Cancelled);
    assert_eq!(result.message, "Git operation was cancelled.");
    // No llegó a lanzarse ningún git: el repositorio sigue intacto.
    assert!(
        !get_structured_status(&fixture.repo_path)
            .unwrap()
            .operation_state
            .in_progress
    );
}
