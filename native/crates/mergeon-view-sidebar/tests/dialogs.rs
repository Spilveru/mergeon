//! Los diálogos del sidebar por separado: port de
//! `src/components/repository/CheckoutBranchDialog.test.js` y cobertura de
//! «Create Branch», que el sidebar abre desde «New Branch...».

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use mergeon_app::LocalChangesMode;
use mergeon_backend::{BranchComparison, ComparedCommit};
use mergeon_view_sidebar::SidebarCommand;
use mergeon_view_sidebar::dialogs::{
    CheckoutBranchDialog, CheckoutBranchDialogEvent, CheckoutBranchOptions, CheckoutTargetOption,
    open_branch_comparison_dialog, open_checkout_branch_dialog,
};

use support::{Harness, TestRepo};

/// Abre el diálogo de checkout y recoge lo que emite.
fn open_checkout(
    h: &Harness,
    options: CheckoutBranchOptions,
    cx: &mut TestAppContext,
) -> (
    Entity<CheckoutBranchDialog>,
    Rc<RefCell<Vec<CheckoutBranchDialogEvent>>>,
) {
    let dialog = h.in_window(cx, |window, cx| {
        open_checkout_branch_dialog(options, window, cx)
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&dialog, move |_, event: &CheckoutBranchDialogEvent, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    (dialog, events)
}

#[gpui_kit::test]
async fn renders_the_plain_switch_mode_for_local_branches(cx: &mut TestAppContext) {
    let h = Harness::mount(None, cx);
    let (dialog, _) = open_checkout(&h, CheckoutBranchOptions::new("feature"), cx);

    assert_eq!(h.dialog_titles(cx), ["Switch Branch"]);
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.options().target_label, "feature");
        assert_eq!(dialog.confirm_label(), "Checkout");
        assert!(!dialog.has_target_choice());
    });
    assert!(h.exists(cx, "checkout-submit"));
    assert!(h.exists(cx, "radio-keep"));
}

#[gpui_kit::test]
async fn renders_the_tracking_mode_when_a_remote_branch_is_provided(cx: &mut TestAppContext) {
    let h = Harness::mount(None, cx);
    let (dialog, _) = open_checkout(
        &h,
        CheckoutBranchOptions::new("origin/feature").remote_branch("origin/feature"),
        cx,
    );

    assert_eq!(h.dialog_titles(cx), ["Track Remote Branch"]);
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.confirm_label(), "Create and Checkout");
    });
}

#[gpui_kit::test]
async fn submits_the_selected_local_changes_mode(cx: &mut TestAppContext) {
    let h = Harness::mount(None, cx);
    let (_dialog, events) = open_checkout(
        &h,
        CheckoutBranchOptions::new("origin/feature")
            .remote_branch("origin/feature")
            .has_local_changes(false),
        cx,
    );

    // Sin cambios locales las opciones están deshabilitadas: el clic no cambia nada.
    h.click(cx, "radio-discard");
    h.click(cx, "checkout-submit");

    // `target` viaja siempre; es `None` cuando no hay destinos que elegir.
    assert_eq!(
        *events.borrow(),
        [CheckoutBranchDialogEvent::Submit {
            local_changes_mode: LocalChangesMode::Keep,
            target: None,
        }]
    );
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn submits_the_chosen_mode_and_target(cx: &mut TestAppContext) {
    let h = Harness::mount(None, cx);
    let (dialog, events) = open_checkout(
        &h,
        CheckoutBranchOptions::new("d6cf210")
            .has_local_changes(true)
            .title("Checkout")
            .target_options(vec![
                CheckoutTargetOption::new("main", "main"),
                CheckoutTargetOption::new("d6cf210", "Commit d6cf210")
                    .warning("You will be in detached HEAD state.")
                    .confirm_label("Checkout commit"),
            ]),
        cx,
    );
    assert_eq!(h.dialog_titles(cx), ["Checkout"]);

    h.click(cx, "radio-d6cf210");
    h.click(cx, "radio-stash");
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.confirm_label(), "Checkout commit");
        assert_eq!(dialog.local_changes_mode(), LocalChangesMode::Stash);
    });
    h.click(cx, "checkout-submit");

    assert_eq!(
        *events.borrow(),
        [CheckoutBranchDialogEvent::Submit {
            local_changes_mode: LocalChangesMode::Stash,
            target: Some("d6cf210".into()),
        }]
    );
}

#[gpui_kit::test]
async fn dismissing_the_dialog_reports_a_close(cx: &mut TestAppContext) {
    let h = Harness::mount(None, cx);
    let (_dialog, events) = open_checkout(&h, CheckoutBranchOptions::new("feature"), cx);

    h.in_window(cx, |window, cx| window.press("escape", cx));

    assert_eq!(*events.borrow(), [CheckoutBranchDialogEvent::Close]);
    assert!(h.dialog_titles(cx).is_empty());
}

// ---------------------------------------------------------------------------
// Create Branch
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn creates_a_branch_from_the_context_menu_and_checks_it_out(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    let branch_reads = h.reads(cx, "branches");

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(cx, "branch:feature", &entries, "New Branch...");
    h.wait_ui(cx, "the create branch dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    assert_eq!(h.dialog_titles(cx), ["Create Branch"]);

    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.create_branch_dialog().cloned())
        .expect("create branch dialog");
    // Sin nombre no se puede crear.
    dialog.read_with(cx, |dialog, cx| {
        assert!(!dialog.is_submittable(cx));
        assert_eq!(dialog.confirm_label(), "Create and Checkout");
    });
    h.click(cx, "create-branch-submit");
    assert_eq!(h.dialog_titles(cx), ["Create Branch"]);

    // El campo tiene el foco al abrir: se escribe directamente.
    h.in_window(cx, |window, cx| window.input("topic", cx));
    h.click(cx, "create-branch-submit");
    h.wait_repo(cx, "the new branch", |repository| {
        repository.current_branch.as_deref() == Some("topic")
    });
    h.settle(cx);

    assert!(h.dialog_titles(cx).is_empty());
    assert_eq!(repo.current_branch(), "topic");
    // Nace de la rama del menú, no de la actual.
    assert_eq!(repo.rev_parse("topic"), repo.rev_parse("feature"));
    assert_eq!(h.reads(cx, "branches"), branch_reads + 1);
}

#[gpui_kit::test]
async fn creates_a_branch_without_checking_it_out(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::NewBranchFromBranch("main".to_string()),
                window,
                cx,
            );
        });
    });
    h.wait_ui(cx, "the create branch dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    h.in_window(cx, |window, cx| window.input("later", cx));
    h.click(cx, "create-branch-checkout");
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.create_branch_dialog().cloned())
        .expect("create branch dialog");
    dialog.read_with(cx, |dialog, _| {
        assert!(!dialog.checkout_after_create());
        assert_eq!(dialog.confirm_label(), "Create Branch");
    });

    // Enter en el campo confirma.
    h.in_window(cx, |window, cx| window.press("enter", cx));
    h.wait_repo(cx, "the new branch", |repository| {
        repository
            .branches
            .iter()
            .any(|branch| branch.name == "later")
    });
    h.settle(cx);

    assert_eq!(repo.current_branch(), "main");
    assert_eq!(repo.branches(), ["later", "main"]);
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.create_branch_dialog().is_none())
    );
}

// La comparación lista como mucho 200 commits por lado: el resto se cuenta.
#[gpui_kit::test]
async fn the_comparison_says_how_many_commits_it_did_not_list(cx: &mut TestAppContext) {
    let h = Harness::mount(None, cx);
    let commit = |message: &str| ComparedCommit {
        hash: "abc1234".into(),
        full_hash: "abc1234".repeat(5) + "abcde",
        message: message.into(),
        author: "Test User".into(),
        date: "2024-01-01T00:00:00Z".into(),
    };
    let comparison = BranchComparison {
        ahead: 250,
        behind: 1,
        ahead_commits: vec![commit("newest ahead")],
        behind_commits: vec![commit("only behind")],
        files_changed: 3,
        insertions: 10,
        deletions: 2,
    };

    h.in_window(cx, |window, cx| {
        open_branch_comparison_dialog("feature", "main", comparison, window, cx);
    });

    assert_eq!(h.dialog_titles(cx), ["Branch Comparison"]);
    assert!(h.exists(cx, "branch-comparison-ahead-more"));
    assert!(!h.exists(cx, "branch-comparison-behind-more"));
}
