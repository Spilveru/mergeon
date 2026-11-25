//! Forma de los resultados y constructores de error.

use mergeon_gitcmd::{ControlCode, OperationContext, run_with_operation_context};
use std::time::Duration;

use super::support::*;
use crate::exec::OperationError;
use crate::results::{
    MutationExtra, MutationPayload, build_generic_error_result, build_mutation_error_result,
    build_remote_mutation_error_result, compute_refs_changed, create_mutation_result,
    read_refs_fingerprint,
};
use crate::{
    Code, FetchOptions, MergeOptions, Operation, Outcome, PullOptions, PushOptions,
    RebaseTodoEntry, SetIdentityOptions, create_branch_operation, merge_operation,
    sequence_action_operation, set_git_identity_operation,
};

fn keys(value: &serde_json::Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("json object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn mutation_results_serialize_with_the_field_names_of_the_electron_app() {
    let (_root, repo_path) = create_repository();

    let result = create_branch_operation(&repo_path, "feature", None, true);
    let json = serde_json::to_value(&result).unwrap();

    assert_eq!(
        keys(&json),
        [
            "action",
            "allRemotes",
            "autostash",
            "branch",
            "code",
            "commit",
            "conflictFiles",
            "conflictState",
            "filePath",
            "filesAffected",
            "message",
            "mode",
            "operation",
            "operationState",
            "outcome",
            "rebase",
            "refsChanged",
            "remote",
            "remoteResults",
            "stashRef",
            "status",
            "success",
            "summary",
            "tag",
            "upstreamSet",
        ]
    );
    assert_eq!(json["operation"], "create-branch");
    assert_eq!(json["outcome"], "completed");
    assert_eq!(json["code"], "OK");
    assert_eq!(json["branch"], "feature");
    assert_eq!(json["refsChanged"], serde_json::Value::Null);
    assert_eq!(json["status"]["current"], "feature");
    assert_eq!(json["operationState"]["inProgress"], false);
    assert_eq!(json["conflictState"]["pendingCount"], 0);

    let restored: crate::MutationResult = serde_json::from_value(json).unwrap();
    assert_eq!(restored, result);
}

#[test]
fn other_results_serialize_with_the_field_names_of_the_electron_app() {
    let fixture = create_diverged_repository();

    let merge = merge_operation(&fixture.repo_path, "feature", &MergeOptions::default());
    let json = serde_json::to_value(&merge).unwrap();
    assert_eq!(
        keys(&json),
        [
            "branch",
            "code",
            "conflictFiles",
            "conflictState",
            "message",
            "operation",
            "operationState",
            "outcome",
            "remote",
            "status",
            "success",
            "upstreamSet",
        ]
    );
    assert_eq!(json["code"], "CONFLICT");
    assert_eq!(json["outcome"], "conflicted");
    assert_eq!(json["operationState"]["type"], "merge");

    let sequence = sequence_action_operation(&fixture.repo_path, "abort");
    let json = serde_json::to_value(&sequence).unwrap();
    assert_eq!(
        keys(&json),
        [
            "action",
            "code",
            "conflictState",
            "message",
            "operation",
            "operationState",
            "outcome",
            "status",
            "success",
        ]
    );
    assert_eq!(json["operation"], "merge");
    assert_eq!(json["action"], "abort");

    let identity = set_git_identity_operation(
        &fixture.repo_path,
        &SetIdentityOptions {
            scope: "system".to_string(),
            ..Default::default()
        },
    );
    assert_eq!(
        serde_json::to_value(&identity).unwrap(),
        serde_json::json!({
            "success": false,
            "operation": "set-identity",
            "outcome": "blocked",
            "code": "GIT_ERROR",
            "message": "Unsupported identity scope 'system'."
        })
    );
}

#[test]
fn options_deserialize_from_the_shapes_the_interface_sends() {
    let push: PushOptions =
        serde_json::from_value(serde_json::json!({ "targetBranch": "release", "pushTags": true }))
            .unwrap();
    assert_eq!(push.remote, "origin");
    assert_eq!(push.target_branch.as_deref(), Some("release"));
    assert!(push.push_tags && !push.force);

    let fetch: FetchOptions =
        serde_json::from_value(serde_json::json!({ "allRemotes": true })).unwrap();
    assert!(fetch.all_remotes);
    assert_eq!(fetch.remote, "origin");

    let pull: PullOptions = serde_json::from_value(serde_json::json!({ "rebase": true })).unwrap();
    assert_eq!(pull.remote.as_deref(), Some("origin"));
    assert!(pull.rebase && !pull.autostash);

    let entry: RebaseTodoEntry =
        serde_json::from_value(serde_json::json!({ "action": "pick", "hash": "abc1234" })).unwrap();
    assert_eq!(entry.message, None);

    assert_eq!(SetIdentityOptions::default().scope, "local");
}

#[test]
fn codes_and_outcomes_expose_their_wire_text() {
    assert_eq!(Code::UpstreamCreated.as_str(), "UPSTREAM_CREATED");
    assert_eq!(Code::from(ControlCode::Timeout), Code::Timeout);
    assert_eq!(Outcome::from(ControlCode::Cancelled).as_str(), "cancelled");
    assert_eq!(
        Operation::FastForwardBranch.to_string(),
        "fast-forward-branch"
    );
    assert_eq!(
        serde_json::to_value(Operation::CherryPick).unwrap(),
        "cherry-pick"
    );
}

#[test]
fn mutation_results_take_conflict_data_from_the_status() {
    let payload = MutationPayload::completed(Operation::Stage, "done");

    let without_status = create_mutation_result(payload.clone(), None);
    assert_eq!(without_status.status, None);
    assert_eq!(without_status.operation_state, None);
    assert!(!without_status.conflict_state.in_conflict);
    assert!(without_status.conflict_files.is_empty());
    assert_eq!(without_status.refs_changed, None);

    let mut status = mergeon_gitcmd::StructuredStatus::empty();
    status.conflicted = vec!["a.txt".to_string()];
    let from_status = create_mutation_result(payload.clone(), Some(status.clone()));
    assert_eq!(from_status.conflict_files, ["a.txt"]);

    // Los archivos que da la propia operación tienen prioridad.
    let explicit = create_mutation_result(
        payload.with(MutationExtra {
            conflict_files: vec!["b.txt".to_string()],
            ..Default::default()
        }),
        Some(status),
    );
    assert_eq!(explicit.conflict_files, ["b.txt"]);
}

#[test]
fn refs_fingerprint_only_changes_when_a_ref_moves() {
    let (root, repo_path) = create_repository();

    let before = read_refs_fingerprint(&repo_path);
    assert!(
        before
            .as_deref()
            .is_some_and(|refs| refs.contains("refs/heads/main"))
    );
    assert_eq!(
        compute_refs_changed(
            before.as_deref(),
            read_refs_fingerprint(&repo_path).as_deref()
        ),
        Some(false)
    );

    run_git(&repo_path, &["branch", "feature"]);
    assert_eq!(
        compute_refs_changed(
            before.as_deref(),
            read_refs_fingerprint(&repo_path).as_deref()
        ),
        Some(true)
    );

    // Sin huella no se puede afirmar nada.
    assert_eq!(read_refs_fingerprint(&root.path().join("missing")), None);
    assert_eq!(compute_refs_changed(None, before.as_deref()), None);
    assert_eq!(compute_refs_changed(before.as_deref(), None), None);
}

#[test]
fn remote_errors_are_classified_by_their_text() {
    let (_root, repo_path) = create_repository();
    let classify = |message: &str| {
        build_remote_mutation_error_result(
            &repo_path,
            Operation::Push,
            &OperationError::new(message),
            MutationExtra {
                remote: Some("origin".to_string()),
                ..Default::default()
            },
        )
    };

    let auth = classify("fatal: Authentication failed for 'https://example.com/repo.git'\n");
    assert_eq!(
        (auth.outcome, auth.code),
        (Outcome::Blocked, Code::AuthRequired)
    );
    assert_eq!(
        auth.message,
        "Authentication failed for 'https://example.com/repo.git'"
    );
    assert_eq!(auth.remote.as_deref(), Some("origin"));
    assert!(auth.status.is_some());

    let conflict = classify("CONFLICT (content): Merge conflict in a.txt\n");
    assert_eq!(
        (conflict.outcome, conflict.code),
        (Outcome::Conflicted, Code::Conflict)
    );

    let generic = classify("fatal: unable to access 'https://example.com/': boom\n");
    assert_eq!(
        (generic.outcome, generic.code),
        (Outcome::Failed, Code::GitError)
    );

    let empty = classify("");
    assert_eq!(empty.message, "Git push failed.");
}

#[test]
fn local_errors_are_classified_by_operation_and_text() {
    let (_root, repo_path) = create_repository();
    let classify = |operation: Operation, message: &str| {
        build_mutation_error_result(
            &repo_path,
            operation,
            &OperationError::new(message),
            MutationExtra {
                tag: Some("v1".to_string()),
                ..Default::default()
            },
        )
    };

    let overwrite = "error: Your local changes to the following files would be overwritten by checkout:\n\tfile.txt\n";
    let checkout = classify(Operation::Checkout, overwrite);
    assert_eq!(
        (checkout.outcome, checkout.code),
        (Outcome::Blocked, Code::CheckoutBlocked)
    );
    // El mismo texto en otra operación no es un checkout bloqueado.
    assert_eq!(
        classify(Operation::CheckoutTag, overwrite).code,
        Code::GitError
    );

    let conflict = "CONFLICT (content): Merge conflict in file.txt\n";
    let pop = classify(Operation::PopStash, conflict);
    assert_eq!(
        (pop.outcome, pop.code),
        (Outcome::Conflicted, Code::StashConflict)
    );
    let rebase = classify(Operation::InteractiveRebase, conflict);
    assert_eq!(
        (rebase.outcome, rebase.code),
        (Outcome::Conflicted, Code::GitError)
    );

    let tag = classify(Operation::CreateTag, "fatal: tag 'v1' already exists\n");
    assert_eq!(
        (tag.outcome, tag.code),
        (Outcome::Blocked, Code::TagAlreadyExists)
    );
    assert_eq!(tag.tag.as_deref(), Some("v1"));

    let empty = classify(Operation::DeleteBranch, "fatal:\n");
    assert_eq!(
        (empty.outcome, empty.code),
        (Outcome::Failed, Code::GitError)
    );
    assert_eq!(empty.message, "Git delete-branch failed.");
}

#[test]
fn generic_errors_cover_auth_conflicts_and_plain_failures() {
    let (_root, repo_path) = create_repository();
    let classify = |message: &str| {
        build_generic_error_result(
            &repo_path,
            Operation::Merge,
            &OperationError::new(message),
            Some("main".to_string()),
            None,
        )
    };

    let auth = classify("remote: Repository not found.\n");
    assert_eq!(
        (auth.outcome, auth.code),
        (Outcome::Blocked, Code::AuthRequired)
    );
    assert_eq!(auth.branch.as_deref(), Some("main"));

    let conflict = classify("error: could not apply 1234567... change\n");
    assert_eq!(
        (conflict.outcome, conflict.code),
        (Outcome::Conflicted, Code::Conflict)
    );
    assert_eq!(
        conflict.message,
        "merge stopped because Git detected conflicts."
    );
    assert!(conflict.status.is_some());

    let generic = classify("fatal: refusing to merge unrelated histories\n");
    assert_eq!(
        (generic.outcome, generic.code),
        (Outcome::Failed, Code::GitError)
    );
    assert_eq!(generic.message, "refusing to merge unrelated histories");
    assert_eq!(generic.status, None);
}

#[test]
fn a_timed_out_context_wins_over_the_error_text() {
    let (_root, repo_path) = create_repository();
    let context = OperationContext::new(None, Duration::from_secs(30));
    context.set_control_code(ControlCode::Timeout);
    let error = OperationError::new("fatal: Authentication failed");

    let (mutation, remote, generic) = run_with_operation_context(context, || {
        (
            build_mutation_error_result(
                &repo_path,
                Operation::Commit,
                &error,
                MutationExtra::default(),
            ),
            build_remote_mutation_error_result(
                &repo_path,
                Operation::Pull,
                &error,
                MutationExtra::default(),
            ),
            build_generic_error_result(&repo_path, Operation::Rebase, &error, None, None),
        )
    });

    for (outcome, code, message) in [
        (mutation.outcome, mutation.code, mutation.message),
        (remote.outcome, remote.code, remote.message),
        (generic.outcome, generic.code, generic.message),
    ] {
        assert_eq!(outcome, Outcome::Timeout);
        assert_eq!(code, Code::Timeout);
        assert_eq!(message, "Git operation timed out and was stopped.");
    }

    // Sin contexto decide el propio error.
    let own_control = OperationError {
        message: "Git operation was cancelled.".to_string(),
        control: Some(ControlCode::Cancelled),
    };
    let cancelled = build_mutation_error_result(
        &repo_path,
        Operation::Commit,
        &own_control,
        MutationExtra::default(),
    );
    assert_eq!(cancelled.outcome, Outcome::Cancelled);
    assert_eq!(cancelled.code, Code::Cancelled);
}
