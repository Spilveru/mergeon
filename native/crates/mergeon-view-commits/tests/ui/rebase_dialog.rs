//! `InteractiveRebaseDialog.test.js`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Entity, SharedString, TestAppContext};
use mergeon_backend::RebaseTodoEntry;
use mergeon_view_commits::{
    InteractiveRebaseDialog, InteractiveRebaseDialogEvent, RebaseAction, RebaseCommit,
    open_interactive_rebase_dialog,
};

use crate::support::{Scene, TestApp};

fn commits() -> Vec<RebaseCommit> {
    vec![
        RebaseCommit::new("a".repeat(40), "oldest commit"),
        RebaseCommit::new("b".repeat(40), "middle commit"),
        RebaseCommit::new("c".repeat(40), "newest commit"),
    ]
}

type Submitted = Rc<RefCell<Vec<Vec<RebaseTodoEntry>>>>;

fn open_dialog(
    app: &TestApp,
    cx: &mut TestAppContext,
) -> (Entity<InteractiveRebaseDialog>, Submitted) {
    app.mount(cx, |_, _| {
        Scene::new(|_, _| gpui_kit::div().into_any_element())
    });
    let dialog = app.in_window(cx, |window, cx| {
        open_interactive_rebase_dialog(commits(), window, cx)
    });
    let submitted: Submitted = Rc::default();
    cx.update(|cx| {
        let submitted = submitted.clone();
        cx.subscribe(
            &dialog,
            move |_, event: &InteractiveRebaseDialogEvent, _| {
                if let InteractiveRebaseDialogEvent::Submit(todo) = event {
                    submitted.borrow_mut().push(todo.clone());
                }
            },
        )
        .detach();
    });
    app.wait_for_dialog(cx, "Interactive rebase");
    (dialog, submitted)
}

fn short(todo: &[RebaseTodoEntry]) -> Vec<String> {
    todo.iter()
        .map(|entry| entry.hash[..7].to_string())
        .collect()
}

fn action_trigger(hash: char) -> SharedString {
    SharedString::from(format!("rebase-action:{}", hash.to_string().repeat(40)))
}

use gpui_kit::IntoElement as _;

#[gpui_kit::test]
async fn renders_rows_oldest_first_and_omits_squash_and_fixup_on_the_first_row(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let (dialog, _) = open_dialog(&app, cx);

    let rows: Vec<String> = dialog.read_with(cx, |dialog, _| {
        dialog
            .rows()
            .iter()
            .map(|row| row.original_message.clone())
            .collect()
    });
    assert_eq!(rows, ["oldest commit", "middle commit", "newest commit"]);
    assert_eq!(app.label(cx, "rebase-summary").as_deref(), Some("3 pick"));

    // El selector de la primera fila: pick, reword y drop.
    app.click(cx, action_trigger('a'));
    assert!(app.menu_is_open(cx));
    let entries = app.in_window(cx, |window, _| {
        let menu = gpui_kit::test::TestWindowExt::within(window, "popup-menu");
        (0usize..6)
            .filter(|index| menu.try_find(*index).is_some())
            .count()
    });
    assert_eq!(entries, 3);
    // Elegir "drop" (la tercera) cambia la fila.
    app.click_menu_item(cx, 2);
    assert_eq!(
        dialog.read_with(cx, |dialog, _| dialog.rows()[0].action),
        RebaseAction::Drop
    );

    // Al descartar la primera fila, la segunda se queda sin predecesor vivo:
    // solo ofrece pick, reword y drop.
    app.click(cx, action_trigger('b'));
    let entries = app.in_window(cx, |window, _| {
        let menu = gpui_kit::test::TestWindowExt::within(window, "popup-menu");
        (0usize..6)
            .filter(|index| menu.try_find(*index).is_some())
            .count()
    });
    assert_eq!(entries, 3);
    app.in_window(cx, |window, cx| {
        gpui_kit::test::TestWindowExt::press(window, "escape", cx);
    });

    // La tercera fila sí tiene a la segunda (`pick`) delante: ofrece las cinco.
    app.click(cx, action_trigger('c'));
    let entries = app.in_window(cx, |window, _| {
        let menu = gpui_kit::test::TestWindowExt::within(window, "popup-menu");
        (0usize..6)
            .filter(|index| menu.try_find(*index).is_some())
            .count()
    });
    assert_eq!(entries, 5);
}

#[gpui_kit::test]
async fn dropping_the_first_row_promotes_following_squash_to_pick(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, _) = open_dialog(&app, cx);

    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.set_action(1, RebaseAction::Squash, window, cx);
            assert_eq!(dialog.rows()[1].action, RebaseAction::Squash);
            dialog.set_action(0, RebaseAction::Drop, window, cx);
        });
    });

    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.rows()[0].action, RebaseAction::Drop);
        assert_eq!(dialog.rows()[1].action, RebaseAction::Pick);
        assert!(dialog.can_submit());
    });
}

#[gpui_kit::test]
async fn reorders_rows_with_the_move_buttons_and_emits_the_todo_in_order(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, submitted) = open_dialog(&app, cx);

    // Bajar la primera fila un puesto.
    app.click(
        cx,
        SharedString::from(format!("rebase-down:{}", "a".repeat(40))),
    );
    let order: Vec<String> = dialog.read_with(cx, |dialog, _| {
        dialog
            .rows()
            .iter()
            .map(|row| row.hash[..7].to_string())
            .collect()
    });
    assert_eq!(order, ["bbbbbbb", "aaaaaaa", "ccccccc"]);

    app.click(cx, "rebase-submit");
    let todo = submitted.borrow()[0].clone();
    assert_eq!(short(&todo), ["bbbbbbb", "aaaaaaa", "ccccccc"]);
    assert!(todo.iter().all(|entry| entry.action == "pick"));
}

#[gpui_kit::test]
async fn shows_the_reword_input_and_includes_the_message_in_the_todo(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, submitted) = open_dialog(&app, cx);

    app.click(cx, action_trigger('b'));
    app.click_menu_item(cx, 1);
    assert!(dialog.read_with(cx, |dialog, _| dialog.message_input(1).is_some()));
    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.set_message(1, "better message", window, cx)
        });
    });

    app.click(cx, "rebase-submit");
    let todo = submitted.borrow()[0].clone();
    assert_eq!(todo[1].action, "reword");
    assert_eq!(todo[1].message.as_deref(), Some("better message"));
    assert_eq!(todo[0].message, None);
}

#[gpui_kit::test]
async fn disables_submit_when_every_row_is_dropped(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, submitted) = open_dialog(&app, cx);

    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            for index in 0..3 {
                dialog.set_action(index, RebaseAction::Drop, window, cx);
            }
        });
    });

    assert!(!dialog.read_with(cx, |dialog, _| dialog.can_submit()));
    app.click(cx, "rebase-submit");
    assert!(submitted.borrow().is_empty());
}

#[gpui_kit::test]
async fn builds_only_the_visible_rows_of_a_long_range(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.mount(cx, |_, _| {
        Scene::new(|_, _| gpui_kit::div().into_any_element())
    });
    let commits: Vec<RebaseCommit> = (0..400)
        .map(|ix| RebaseCommit::new(format!("{ix:040}"), format!("commit {ix}")))
        .collect();
    let dialog = app.in_window(cx, |window, cx| {
        open_interactive_rebase_dialog(commits, window, cx)
    });
    app.wait_for_dialog(cx, "Interactive rebase");

    let row = |ix: usize| SharedString::from(format!("rebase-row:{ix:040}"));
    assert!(app.exists(cx, row(0)));
    assert!(!app.exists(cx, row(399)));
    let height = app.in_window(cx, |window, _| {
        gpui_kit::test::TestWindowExt::find(window, "rebase-list")
            .bounds()
            .size
            .height
    });
    assert!(height <= gpui_kit::px(420.), "{height:?}");
    assert_eq!(app.label(cx, "rebase-summary").as_deref(), Some("400 pick"));

    // Las filas virtuales siguen respondiendo: bajar la primera la cambia de
    // sitio con la segunda.
    app.click(cx, SharedString::from(format!("rebase-down:{:040}", 0)));
    let first = dialog.read_with(cx, |dialog, _| dialog.rows()[0].hash.clone());
    assert_eq!(first, format!("{:040}", 1));
}
