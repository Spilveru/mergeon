//! `OperationBanner`, `OperationStatusSlot` y `MutationQueueBanner`
//! (`OperationBanner.test.js`, `OperationStatusSlot.test.js`,
//! `MutationQueueBanner.test.js`).

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{IntoElement as _, TestAppContext};
use mergeon_app::AppModel;
use mergeon_gitcmd::{ConflictState, OperationState, OperationType};
use mergeon_model::operation_types::{OperationPhase, QueueOperation};
use mergeon_model::repositories::MutationQueue;
use mergeon_view_toolbar::{MutationQueueBanner, OperationBanner, SequenceAction};

use crate::support::{TestApp, TestRepo};

fn operation(kind: OperationType, label: &str, can_skip: bool) -> OperationState {
    OperationState {
        in_progress: true,
        kind: Some(kind),
        label: label.to_string(),
        can_continue: true,
        can_abort: true,
        can_skip,
    }
}

fn conflict(kind: OperationType, initial: usize, pending: usize) -> ConflictState {
    ConflictState {
        in_conflict: true,
        operation_type: Some(kind),
        initial_count: initial,
        pending_count: pending,
        resolved_count: initial - pending,
        pending_files: (0..pending)
            .map(|index| format!("file-{index}.txt"))
            .collect(),
        ready_to_continue: pending == 0,
    }
}

/// El repositorio activo sigue con una operación de secuencia a medias.
fn sequence_in_progress(model: &AppModel) -> bool {
    model.active_repository().is_some_and(|repository| {
        repository
            .operation_state
            .as_ref()
            .is_some_and(|state| state.in_progress)
    })
}

/// Dibuja el banner suelto y devuelve las acciones que emite.
fn show_banner(
    app: &TestApp,
    cx: &mut TestAppContext,
    operation: OperationState,
    conflict: ConflictState,
    configure: impl Fn(OperationBanner) -> OperationBanner + 'static,
) -> Rc<RefCell<Vec<SequenceAction>>> {
    let emitted = Rc::new(RefCell::new(Vec::new()));
    app.show(cx, {
        let emitted = emitted.clone();
        move |_, _| {
            let emitted = emitted.clone();
            configure(OperationBanner::new(
                Some(operation.clone()),
                Some(conflict.clone()),
            ))
            .on_sequence_action(move |action, _, _| emitted.borrow_mut().push(action))
            .into_any_element()
        }
    });
    emitted
}

// ---------------------------------------------------------------------------
// OperationBanner.test.js
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn renders_action_buttons_for_a_rebase_operation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    show_banner(
        &app,
        cx,
        operation(OperationType::Rebase, "Rebase in progress", true),
        conflict(OperationType::Rebase, 5, 3),
        |banner| banner,
    );

    assert_eq!(
        app.label(cx, "operation-banner").as_deref(),
        Some("Rebase in progress")
    );
    for id in ["sequence-continue", "sequence-skip", "sequence-abort"] {
        assert!(app.exists(cx, id), "`{id}` should be offered");
    }
}

#[gpui_kit::test]
async fn shows_ready_to_continue_text_when_there_are_no_pending_conflicts(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    show_banner(
        &app,
        cx,
        operation(OperationType::Merge, "Merge in progress", false),
        conflict(OperationType::Merge, 2, 0),
        |banner| banner,
    );

    assert_eq!(
        app.label(cx, "operation-banner").as_deref(),
        Some("All conflicts resolved. Ready to continue merge.")
    );
    // Un merge no se puede saltar.
    assert!(!app.exists(cx, "sequence-skip"));
}

#[gpui_kit::test]
async fn emits_sequence_action_when_clicking_continue(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let emitted = show_banner(
        &app,
        cx,
        operation(OperationType::Merge, "Merge in progress", false),
        conflict(OperationType::Merge, 1, 0),
        |banner| banner,
    );

    app.click(cx, "sequence-continue");
    assert_eq!(*emitted.borrow(), [SequenceAction::Continue]);

    app.click(cx, "sequence-abort");
    assert_eq!(
        *emitted.borrow(),
        [SequenceAction::Continue, SequenceAction::Abort]
    );
}

#[gpui_kit::test]
async fn the_busy_action_is_locked_and_the_others_stay_available(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let emitted = show_banner(
        &app,
        cx,
        operation(OperationType::Rebase, "Rebase in progress", true),
        conflict(OperationType::Rebase, 1, 1),
        |banner| banner.busy_action(Some(SequenceAction::Continue)),
    );

    app.click(cx, "sequence-continue");
    app.click(cx, "sequence-skip");

    assert_eq!(*emitted.borrow(), [SequenceAction::Skip]);
}

#[gpui_kit::test]
async fn the_actions_can_be_left_out(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    show_banner(
        &app,
        cx,
        operation(OperationType::Merge, "Merge in progress", false),
        conflict(OperationType::Merge, 1, 1),
        |banner| banner.show_actions(false),
    );

    assert!(app.exists(cx, "operation-banner"));
    assert!(!app.exists(cx, "sequence-continue"));
    assert!(!app.exists(cx, "sequence-abort"));
}

// ---------------------------------------------------------------------------
// OperationStatusSlot.test.js
// ---------------------------------------------------------------------------

/// El slot ya no reserva altura: sin operación de secuencia desaparece por
/// completo.
#[gpui_kit::test]
async fn renders_nothing_when_there_is_no_active_operation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    assert!(!app.exists(cx, "operation-status-slot"));

    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    assert!(!app.exists(cx, "operation-status-slot"));
}

#[gpui_kit::test]
async fn renders_one_sequence_operation_banner_and_forwards_actions(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict().resolve_conflict();
    app.open(&repo, cx).await;

    assert!(app.exists(cx, "operation-status-slot"));
    assert_eq!(
        app.label(cx, "operation-banner").as_deref(),
        Some("All conflicts resolved. Ready to continue merge.")
    );

    // Continue llega a `AppModel::handle_sequence_action`: el merge termina
    // y la franja desaparece.
    app.click(cx, "sequence-continue");
    app.wait_for(cx, "the merge to finish", |model| {
        !sequence_in_progress(model)
    });

    assert!(!app.exists(cx, "operation-status-slot"));
    assert_eq!(
        repo.git(&["log", "-1", "--format=%P"])
            .split_whitespace()
            .count(),
        2,
        "HEAD should be the merge commit"
    );
}

#[gpui_kit::test]
async fn shows_the_conflict_progress_of_the_active_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict();
    app.open(&repo, cx).await;

    assert_eq!(
        app.label(cx, "operation-banner").as_deref(),
        Some("Merge in progress")
    );
}

#[gpui_kit::test]
async fn abort_asks_for_confirmation_before_aborting(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict();
    let before = repo.head();
    app.open(&repo, cx).await;

    // Sin confirmar no pasa nada.
    app.host.answer_confirm(false);
    app.click(cx, "sequence-abort");
    app.wait_until(cx, "the confirmation", || app.host.confirms().len() == 1);
    assert!(app.exists(cx, "operation-status-slot"));

    let confirmation = &app.host.confirms()[0];
    assert_eq!(confirmation.title.as_deref(), Some("Abort operation"));
    assert_eq!(
        confirmation.description.as_deref(),
        Some("Abort the current merge operation?")
    );
    assert_eq!(confirmation.variant.as_deref(), Some("destructive"));

    app.host.answer_confirm(true);
    app.click(cx, "sequence-abort");
    app.wait_for(cx, "the merge to be aborted", |model| {
        !sequence_in_progress(model)
    });

    assert!(!app.exists(cx, "operation-status-slot"));
    assert_eq!(repo.head(), before);
    assert!(
        repo.try_git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// MutationQueueBanner.test.js
// ---------------------------------------------------------------------------

fn queue() -> MutationQueue {
    MutationQueue {
        active: Some(QueueOperation {
            operation_id: Some("fetch-1".to_string()),
            operation: "fetch".to_string(),
            phase: OperationPhase::Running,
            cancellable: true,
            ..Default::default()
        }),
        queued: vec![QueueOperation {
            operation_id: Some("pull-1".to_string()),
            operation: "pull".to_string(),
            phase: OperationPhase::Queued,
            position: 1,
            ..Default::default()
        }],
    }
}

fn show_queue(
    app: &TestApp,
    cx: &mut TestAppContext,
    queue: MutationQueue,
) -> Rc<RefCell<Vec<String>>> {
    let cancelled = Rc::new(RefCell::new(Vec::new()));
    app.show(cx, {
        let cancelled = cancelled.clone();
        move |_, _| {
            let cancelled = cancelled.clone();
            MutationQueueBanner::new(queue.clone())
                .on_cancel(move |operation_id, _, _| {
                    cancelled.borrow_mut().push(operation_id.to_string());
                })
                .into_any_element()
        }
    });
    cancelled
}

#[gpui_kit::test]
async fn shows_the_active_and_queued_operations_and_emits_cancellation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let cancelled = show_queue(&app, cx, queue());

    assert!(app.exists(cx, "mutation-queue"));
    assert_eq!(
        app.label(cx, "queue-row-fetch-1").as_deref(),
        Some("Running Fetch")
    );
    assert_eq!(
        app.label(cx, "queue-row-pull-1").as_deref(),
        Some("Queued #1 Pull")
    );

    app.click(cx, "queue-remove-pull-1");
    assert_eq!(*cancelled.borrow(), ["pull-1"]);

    app.click(cx, "queue-cancel-fetch-1");
    assert_eq!(*cancelled.borrow(), ["pull-1", "fetch-1"]);
}

#[gpui_kit::test]
async fn an_operation_that_cannot_be_cancelled_has_no_cancel_button(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let mut queue = queue();
    if let Some(active) = queue.active.as_mut() {
        active.cancellable = false;
        active.phase = OperationPhase::Cancelling;
    }
    show_queue(&app, cx, queue);

    assert_eq!(
        app.label(cx, "queue-row-fetch-1").as_deref(),
        Some("Stopping Fetch")
    );
    assert!(!app.exists(cx, "queue-cancel-fetch-1"));
}

#[gpui_kit::test]
async fn an_empty_queue_draws_nothing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    show_queue(&app, cx, MutationQueue::default());

    assert!(!app.exists(cx, "mutation-queue"));
}
