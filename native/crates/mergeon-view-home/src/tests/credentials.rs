//! El diálogo de usuario y contraseña de un remoto HTTPS.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_app::{CredentialsAnswer, CredentialsRequest};

use super::support::TestApp;
use crate::open_credentials_dialog;

fn request(username: Option<&str>, refused: bool, can_remember: bool) -> CredentialsRequest {
    CredentialsRequest {
        origin: "https://git.example.com".to_string(),
        host: "git.example.com".to_string(),
        operation: "push".to_string(),
        username: username.map(str::to_string),
        refused,
        can_remember,
    }
}

type Answers = Rc<RefCell<Vec<Option<CredentialsAnswer>>>>;

fn open(
    app: &TestApp,
    request: CredentialsRequest,
    cx: &mut TestAppContext,
) -> (gpui_kit::Entity<crate::CredentialsDialog>, Answers) {
    let answers: Answers = Rc::default();
    let dialog = app.in_window(cx, {
        let answers = answers.clone();
        move |window, cx| {
            open_credentials_dialog(request, window, cx, move |answer| {
                answers.borrow_mut().push(answer)
            })
        }
    });
    app.settle(cx);
    (dialog, answers)
}

fn type_into(
    app: &TestApp,
    dialog: &gpui_kit::Entity<crate::CredentialsDialog>,
    username: &str,
    password: &str,
    cx: &mut TestAppContext,
) {
    let (username, password) = (username.to_string(), password.to_string());
    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog
                .username
                .update(cx, |state, cx| state.set_value(username, window, cx));
            dialog
                .password
                .update(cx, |state, cx| state.set_value(password, window, cx));
        });
    });
}

#[gpui_kit::test]
async fn signs_in_with_what_was_typed(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, answers) = open(&app, request(None, false, false), cx);
    assert_eq!(app.dialog_titles(cx), ["Sign in to git.example.com"]);
    // Sin credential helper no se ofrece recordarlas.
    assert!(!app.exists(cx, "credentials-remember"));
    assert!(!app.exists(cx, "credentials-refused"));

    type_into(&app, &dialog, "alice", "tok-123", cx);
    app.click(cx, "credentials-sign-in");

    assert_eq!(
        *answers.borrow(),
        [Some(CredentialsAnswer {
            username: "alice".to_string(),
            password: "tok-123".to_string(),
            remember: false,
        })]
    );
    assert!(app.dialog_titles(cx).is_empty());
    // El campo de la contraseña no se queda con ella.
    dialog.read_with(cx, |dialog, cx| {
        assert!(dialog.password.read(cx).value().is_empty());
    });
}

#[gpui_kit::test]
async fn the_password_is_masked_and_enter_signs_in(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    // Con el usuario propuesto, el foco empieza en la contraseña.
    let (dialog, answers) = open(&app, request(Some("alice"), false, false), cx);
    type_into(&app, &dialog, "alice", "tok-123", cx);
    dialog.read_with(cx, |dialog, cx| {
        assert_eq!(dialog.username.read(cx).value().as_ref(), "alice");
    });
    app.in_window(cx, |window, cx| window.press("enter", cx));
    app.settle(cx);

    assert_eq!(answers.borrow().len(), 1);
    assert_eq!(
        answers.borrow()[0]
            .as_ref()
            .map(|answer| answer.password.clone()),
        Some("tok-123".to_string())
    );
}

#[gpui_kit::test]
async fn remember_is_offered_only_with_a_credential_helper(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, answers) = open(&app, request(None, true, true), cx);
    assert!(app.exists(cx, "credentials-refused"));
    assert!(app.exists(cx, "credentials-remember"));

    type_into(&app, &dialog, "alice", "tok-123", cx);
    app.click(cx, "credentials-remember");
    app.click(cx, "credentials-sign-in");

    assert_eq!(
        answers.borrow()[0].as_ref().map(|answer| answer.remember),
        Some(true)
    );
}

#[gpui_kit::test]
async fn asks_for_both_fields_and_cancel_answers_nothing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (dialog, answers) = open(&app, request(None, false, false), cx);

    app.click(cx, "credentials-sign-in");
    assert_eq!(
        app.label(cx, "credentials-error").as_deref(),
        Some("Enter a username.")
    );
    type_into(&app, &dialog, "alice", "", cx);
    app.click(cx, "credentials-sign-in");
    assert_eq!(
        app.label(cx, "credentials-error").as_deref(),
        Some("Enter a password or access token.")
    );
    assert!(answers.borrow().is_empty());

    app.click(cx, "credentials-cancel");
    assert_eq!(*answers.borrow(), [None]);
    assert!(app.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn closing_with_escape_counts_as_cancelling(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_dialog, answers) = open(&app, request(None, false, false), cx);
    app.in_window(cx, |window, cx| window.press("escape", cx));
    app.settle(cx);
    assert_eq!(*answers.borrow(), [None]);
}

#[gpui_kit::test]
async fn warns_when_credentials_are_sent_over_plain_http(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let mut insecure = request(None, false, false);
    insecure.origin = "http://git.example.com".to_string();
    let (_dialog, _) = open(&app, insecure, cx);
    assert!(app.exists(cx, "credentials-insecure-http"));
    app.click(cx, "credentials-cancel");

    let (_dialog, _) = open(&app, request(None, false, false), cx);
    assert!(!app.exists(cx, "credentials-insecure-http"));
}
