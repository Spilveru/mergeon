//! `RepositoryLauncher.vue`: elegir cierra el lanzador y arranca su flujo.

use gpui_kit::TestAppContext;
use mergeon_app::AppEvent;
use mergeon_app::DialogRequest;
use serde_json::json;

use super::support::{TestApp, Workspace, account, seeded_store};
use crate::open_repository_launcher;

#[gpui_kit::test]
async fn open_local_closes_the_launcher_and_asks_for_a_folder(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let repository = workspace.repository("work/demo", None);
    let app = TestApp::new(cx);
    app.host
        .answer_directory(Some(std::path::Path::new(&repository)));

    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    assert_eq!(app.dialog_titles(cx), ["Open repository"]);

    app.click(cx, "launcher-local");

    assert!(app.dialog_titles(cx).is_empty());
    assert_eq!(app.host.directory_requests(), 1);
    app.wait_for(cx, "the chosen folder to open", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .open_repositories()
                .iter()
                .any(|repo| repo.path == repository)
        })
    });
}

#[gpui_kit::test]
async fn clone_from_github_without_accounts_offers_to_add_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.host.answer_confirm(true);

    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    app.click(cx, "launcher-github");

    assert!(app.dialog_titles(cx).is_empty());
    let confirm = app.host.confirms().pop().expect("confirmación");
    assert_eq!(confirm.title.as_deref(), Some("No GitHub accounts"));
    assert_eq!(confirm.confirm_text.as_deref(), Some("Add Account"));
    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubAccount))
    );
}

#[gpui_kit::test]
async fn clone_from_github_with_an_account_asks_for_the_picker(cx: &mut TestAppContext) {
    let app = TestApp::with_store(
        seeded_store(
            &[],
            &[account(
                json!({ "id": "account-1", "username": "Spilveru", "token": "ghu_token" }),
            )],
        ),
        cx,
    );
    app.start(cx);

    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    app.click(cx, "launcher-github");

    assert!(app.host.confirms().is_empty());
    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubRepositoryPicker))
    );
}

#[gpui_kit::test]
async fn the_launcher_opens_once_and_reports_the_requests_it_handles(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.in_window(cx, |window, cx| {
        let first = open_repository_launcher(window, cx);
        let second = open_repository_launcher(window, cx);
        assert_eq!(first, second);
    });
    assert_eq!(app.dialog_titles(cx), ["Open repository"]);

    app.in_window(cx, |window, cx| {
        assert!(crate::handle_dialog_request(
            &DialogRequest::Launcher,
            window,
            cx
        ));
        assert!(!crate::handle_dialog_request(
            &DialogRequest::Fetch,
            window,
            cx
        ));
    });
    assert_eq!(app.dialog_titles(cx), ["Open repository"]);
}

#[gpui_kit::test]
async fn enter_on_focused_launcher_option_activates_it(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;

    let app = TestApp::new(cx);
    app.host.answer_directory(None);

    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    assert_eq!(app.dialog_titles(cx), ["Open repository"]);

    let mut focused = false;
    for _ in 0..12 {
        if app.in_window(cx, |window, _| {
            window.find("launcher-local").focused() == Some(true)
        }) {
            focused = true;
            break;
        }
        app.in_window(cx, |window, cx| window.press("tab", cx));
    }
    assert!(focused, "launcher-local should be reachable with Tab");

    app.in_window(cx, |window, cx| window.press("enter", cx));
    app.settle(cx);

    assert!(app.dialog_titles(cx).is_empty());
    assert_eq!(app.host.directory_requests(), 1);
}
