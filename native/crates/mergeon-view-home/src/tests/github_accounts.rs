//! `GitHubAccountsManager.test.js`.

use std::sync::Arc;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::{AppEvent, DialogRequest};
use serde_json::json;

use super::support::{TestApp, account, seeded_store};
use crate::{AccountAction, AvatarCache, GitHubAccountsEvent, GitHubAccountsManager};

fn two_accounts(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::with_store(
        seeded_store(
            &[],
            &[
                account(json!({
                    "id": "account-1",
                    "name": "Spilveru",
                    "username": "Spilveru",
                    "email": "spilveru@example.com",
                    "avatarUrl": "https://example.com/a.png",
                    "token": "ghu_one",
                    "requiresInstallation": false,
                    "requiresReconnect": false
                })),
                account(json!({
                    "id": "account-2",
                    "name": "Octocat",
                    "username": "octocat",
                    "email": "octocat@example.com",
                    "avatarUrl": "https://example.com/b.png",
                    "token": "ghu_two",
                    "requiresInstallation": false,
                    "requiresReconnect": true
                })),
            ],
        ),
        cx,
    );
    app.start(cx);
    app
}

#[gpui_kit::test]
async fn disables_account_actions_while_setting_an_account_active(cx: &mut TestAppContext) {
    let app = two_accounts(cx);
    let manager = app.show(cx, |_, cx| GitHubAccountsManager::new(true, cx));

    // La cuenta activa no ofrece activarse.
    assert!(!app.exists(cx, "github-account-set-active:account-1"));
    assert!(app.exists(cx, "github-account-set-active:account-2"));

    app.in_window(cx, |window, cx| {
        window.click("github-account-set-active:account-2", cx);
        // Mientras dura, ninguna otra acción responde.
        window.click("github-account-remove:account-1", cx);
        window.click("github-account-remove:account-2", cx);
    });
    manager.read_with(cx, |manager, _| {
        assert_eq!(
            manager.active_action(),
            Some(&AccountAction::SetActive("account-2".to_string()))
        );
    });
    assert!(app.host.confirms().is_empty());

    app.settle(cx);
    manager.read_with(cx, |manager, _| assert_eq!(manager.active_action(), None));
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .active_github_account_id()
            .map(str::to_string)),
        Some("account-2".to_string())
    );
    assert_eq!(
        app.backend
            .storage()
            .get_active_github_account_id()
            .as_deref(),
        Some("account-2")
    );
    assert!(app.has_toast(NotifyType::Success, "Account set as active"));
    assert!(app.exists(cx, "github-account-set-active:account-1"));
    assert!(!app.exists(cx, "github-account-set-active:account-2"));
}

#[gpui_kit::test]
async fn removes_an_account_after_confirming(cx: &mut TestAppContext) {
    let app = two_accounts(cx);
    let manager = app.show(cx, |_, cx| GitHubAccountsManager::new(true, cx));

    // Cancelar deja la cuenta donde estaba.
    app.host.answer_confirm(false);
    app.click(cx, "github-account-remove:account-2");
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.github_accounts().len()),
        2
    );

    // Con la confirmación abierta todavía no hay acción en curso.
    app.host.hold_answers(true);
    app.in_window(cx, |window, cx| {
        window.click("github-account-remove:account-2", cx);
    });
    app.settle(cx);
    manager.read_with(cx, |manager, _| assert_eq!(manager.active_action(), None));
    let confirm = app.host.confirms().pop().expect("confirmación");
    assert_eq!(confirm.title.as_deref(), Some("Remove account"));
    assert_eq!(
        confirm.description.as_deref(),
        Some("Remove @octocat and its stored product session?")
    );
    assert_eq!(confirm.confirm_text.as_deref(), Some("Remove"));
    assert_eq!(confirm.variant.as_deref(), Some("destructive"));

    assert!(app.host.release_confirm(true));
    app.settle(cx);

    manager.read_with(cx, |manager, _| assert_eq!(manager.active_action(), None));
    let remaining: Vec<String> = app.model.read_with(cx, |model, _| {
        model
            .github_accounts()
            .iter()
            .map(|account| account.id.clone())
            .collect()
    });
    assert_eq!(remaining, ["account-1"]);
    assert!(app.has_toast(NotifyType::Success, "Account removed successfully"));
    assert!(!app.exists(cx, "github-account:account-2"));
}

#[gpui_kit::test]
async fn replaces_a_failed_avatar_with_a_local_github_icon(cx: &mut TestAppContext) {
    let app = two_accounts(cx);
    // La descarga falla, como un `@error` de la imagen.
    cx.update(|cx| crate::set_avatar_fetcher(Arc::new(|_| Err("404".to_string())), cx));
    app.show(cx, |_, cx| GitHubAccountsManager::new(true, cx));
    app.in_window(cx, |_, _| {});
    app.settle(cx);

    cx.update(|cx| {
        let avatars = AvatarCache::global(cx);
        assert!(avatars.read(cx).has_failed("https://example.com/a.png"));
        assert!(avatars.read(cx).has_failed("https://example.com/b.png"));
    });
    // La lista sigue completa, con su botón de alta.
    assert!(app.exists(cx, "github-account:account-1"));
    assert!(app.exists(cx, "github-accounts-add"));
    // Incrustado no hay botón de cerrar.
    assert!(!app.exists(cx, "github-accounts-close"));
}

#[gpui_kit::test]
async fn shows_the_empty_state_and_asks_to_add_an_account(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.start(cx);
    let manager = app.show(cx, |_, cx| GitHubAccountsManager::new(true, cx));
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscription = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&manager, move |_, event: &GitHubAccountsEvent, _| {
            events.borrow_mut().push(event.clone());
        })
    });

    assert!(app.exists(cx, "github-accounts-add-first"));
    assert!(!app.exists(cx, "github-accounts-add"));

    app.click(cx, "github-accounts-add-first");
    assert_eq!(*events.borrow(), [GitHubAccountsEvent::AddAccount]);
}

#[gpui_kit::test]
async fn the_dialog_asks_the_shell_for_the_account_dialog_and_closes(cx: &mut TestAppContext) {
    let app = two_accounts(cx);

    app.in_window(cx, |window, cx| {
        crate::open_github_accounts_dialog(window, cx);
    });
    assert_eq!(app.dialog_titles(cx), ["GitHub Accounts"]);

    app.click(cx, "github-accounts-add");
    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubAccount))
    );

    app.click(cx, "github-accounts-close");
    assert!(app.dialog_titles(cx).is_empty());
}
