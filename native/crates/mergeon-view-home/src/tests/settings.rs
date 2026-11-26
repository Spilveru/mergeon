//! `SettingsView.vue` y lo que aloja: navegación, Help / About y las
//! actualizaciones (`UpdateSettings.test.js`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;

use gpui_kit::{Entity, TestAppContext};
use mergeon_app::backend::{UpdaterBridge, UpdaterEventSink};
use mergeon_app::testing::TestBackend;
use mergeon_app::{AppEvent, DialogRequest, WindowRequest};
use mergeon_updater::{ErrorPhase, UpdateStatus, UpdaterState};
use serde_json::{Value, json};

use super::support::TestApp;
use crate::{SettingsEvent, SettingsSection, SettingsView};

/// Un actualizador que solo registra lo que se le pide.
#[derive(Default)]
struct RecordingUpdater {
    calls: Mutex<Vec<String>>,
    install_ok: bool,
}

impl RecordingUpdater {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl UpdaterBridge for RecordingUpdater {
    fn get_state(&self) -> Value {
        Value::Null
    }

    fn check_for_updates(&self, manual: bool) {
        self.calls.lock().unwrap().push(format!("check:{manual}"));
    }

    fn install_update(&self) -> Value {
        self.calls.lock().unwrap().push("install".to_string());
        json!({ "ok": self.install_ok })
    }

    fn subscribe(&self, _sink: UpdaterEventSink) {}
}

fn settings(
    section: &str,
    cx: &mut TestAppContext,
) -> (TestApp, Arc<RecordingUpdater>, Entity<SettingsView>) {
    let updater = Arc::new(RecordingUpdater {
        install_ok: true,
        ..Default::default()
    });
    let app = TestApp::with_backend(TestBackend::new().with_updater(updater.clone()), cx);
    app.start(cx);
    app.model
        .update(cx, |model, cx| model.open_settings(section, cx));
    let view = app.show(cx, SettingsView::new);
    (app, updater, view)
}

fn apply(app: &TestApp, cx: &mut TestAppContext, state: UpdaterState) {
    app.model
        .update(cx, |model, cx| model.apply_updater_state(&state, cx));
    app.settle(cx);
}

#[gpui_kit::test]
async fn opens_on_the_requested_section_and_navigates(cx: &mut TestAppContext) {
    let (app, _updater, view) = settings("appearance", cx);

    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Appearance)
    });
    assert!(app.exists(cx, "prefs-theme-section"));
    assert!(!app.exists(cx, "prefs-identity"));

    app.click(cx, "git");
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Git)
    });
    assert!(app.exists(cx, "prefs-identity"));
    assert!(app.exists(cx, "prefs-auto-fetch-section"));
    assert!(!app.exists(cx, "prefs-theme-section"));

    app.click(cx, "accounts");
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Accounts)
    });
    assert!(app.exists(cx, "github-accounts-add-first"));
    assert!(!app.exists(cx, "preferences"));

    app.click(cx, "help");
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Help)
    });
    assert!(app.exists(cx, "about-version"));
    assert!(app.exists(cx, "update-action"));
    assert!(app.exists(cx, "Open repository"));
    assert!(app.exists(cx, "settings-minimize"));
}

#[gpui_kit::test]
async fn follows_the_section_the_app_asks_for(cx: &mut TestAppContext) {
    let (app, _updater, view) = settings("help", cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Help)
    });

    // `openSettings('accounts')` desde el inicio.
    app.model
        .update(cx, |model, cx| model.open_settings("accounts", cx));
    app.settle(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Accounts)
    });

    // Navegar a mano no cambia lo que pidió la app, así que no se pisa.
    app.click(cx, "git");
    app.model.update(cx, |_, cx| cx.notify());
    app.settle(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Git)
    });
}

#[gpui_kit::test]
async fn back_asks_the_shell_to_leave_settings(cx: &mut TestAppContext) {
    let (app, _updater, view) = settings("appearance", cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&view, move |_, event: &SettingsEvent, _| {
            events.borrow_mut().push(event.clone());
        })
    });

    app.click(cx, "settings-back");

    assert_eq!(*events.borrow(), [SettingsEvent::Close]);
    // La pestaña sigue abierta: cerrarla es cosa de la barra de pestañas.
    assert!(
        app.model
            .read_with(cx, |model, _| model.settings_tab_open())
    );
}

#[gpui_kit::test]
async fn accounts_asks_for_the_account_dialog_as_settings(cx: &mut TestAppContext) {
    let (app, _updater, _view) = settings("accounts", cx);

    app.click(cx, "github-accounts-add-first");

    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubAccount))
    );
}

#[gpui_kit::test]
async fn window_actions_and_releases_reach_the_shell(cx: &mut TestAppContext) {
    let (app, _updater, _view) = settings("help", cx);

    // Las acciones de ventana están al final de la página.
    app.in_window(cx, |window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.scroll(
            "settings-body",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-10_000.),
            )),
            cx,
        );
    });
    app.click(cx, "settings-minimize");
    app.click(cx, "settings-quit");
    assert!(
        app.events()
            .contains(&AppEvent::Window(WindowRequest::Minimize))
    );
    assert!(
        app.events()
            .contains(&AppEvent::Window(WindowRequest::Quit))
    );

    app.in_window(cx, |window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.scroll(
            "settings-body",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(10_000.))),
            cx,
        );
    });
    app.click(cx, "about-releases");
    assert!(
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/Spilveru/mergeon/releases")
    );
}

#[gpui_kit::test]
async fn shows_the_running_version(cx: &mut TestAppContext) {
    let (app, _updater, _view) = settings("help", cx);
    assert_eq!(
        app.label(cx, "about-version").as_deref(),
        Some("Version dev")
    );

    apply(
        &app,
        cx,
        UpdaterState {
            current_version: Some("0.9.0".to_string()),
            ..UpdaterState::default()
        },
    );
    assert_eq!(
        app.label(cx, "about-version").as_deref(),
        Some("Version 0.9.0")
    );
}

/// `UpdateSettings.test.js`: mensaje neutro tras fallar una comprobación
/// posterior, y reintento.
#[gpui_kit::test]
async fn keeps_the_latest_version_message_neutral_after_a_failed_follow_up_check_and_allows_retry(
    cx: &mut TestAppContext,
) {
    let (app, updater, _view) = settings("help", cx);
    apply(
        &app,
        cx,
        UpdaterState {
            status: UpdateStatus::Error,
            current_version: Some("0.7.0".to_string()),
            latest_version: Some("0.7.0".to_string()),
            error_phase: Some(ErrorPhase::Check),
            ..UpdaterState::default()
        },
    );

    assert!(
        app.label(cx, "update-status")
            .is_some_and(|text| text.contains("Latest is v0.7.0. Could not check again."))
    );

    app.click(cx, "update-action");
    assert_eq!(updater.calls(), ["check:true"]);
}

/// `UpdateSettings.test.js`: progreso sin segunda acción y, ya lista, la
/// instalación.
#[gpui_kit::test]
async fn shows_progress_without_allowing_a_second_action_then_offers_installation_when_ready(
    cx: &mut TestAppContext,
) {
    let (app, updater, _view) = settings("help", cx);
    apply(
        &app,
        cx,
        UpdaterState {
            status: UpdateStatus::Downloading,
            percent: 42,
            latest_version: Some("0.8.0".to_string()),
            ..UpdaterState::default()
        },
    );

    assert!(app.exists(cx, "update-progress"));
    assert_eq!(
        app.label(cx, "update-status").as_deref(),
        Some("Downloading v0.8.0… 42%")
    );
    app.click(cx, "update-action");
    assert!(updater.calls().is_empty());

    apply(
        &app,
        cx,
        UpdaterState {
            status: UpdateStatus::Downloaded,
            latest_version: Some("0.8.0".to_string()),
            can_auto_install: true,
            ..UpdaterState::default()
        },
    );
    assert!(!app.exists(cx, "update-progress"));
    app.click(cx, "update-action");
    assert_eq!(updater.calls(), ["install"]);
}

/// `UpdateSettings.test.js`: donde no hay instalación automática se ofrece la
/// descarga manual.
#[gpui_kit::test]
async fn offers_a_manual_download_when_the_platform_cannot_install_automatically(
    cx: &mut TestAppContext,
) {
    let (app, updater, _view) = settings("help", cx);
    apply(
        &app,
        cx,
        UpdaterState {
            status: UpdateStatus::Downloaded,
            latest_version: Some("0.8.0".to_string()),
            can_auto_install: false,
            releases_url: "https://example.com/releases/latest".to_string(),
            ..UpdaterState::default()
        },
    );

    assert_eq!(
        app.label(cx, "update-status").as_deref(),
        Some("v0.8.0 is available to download.")
    );
    app.click(cx, "update-action");

    assert!(updater.calls().is_empty());
    assert!(
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://example.com/releases/latest")
    );
}

#[gpui_kit::test]
async fn shortcuts_section_records_detects_conflicts_resets_and_syncs_help(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;

    let (app, _updater, view) = settings("shortcuts", cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.section(), SettingsSection::Shortcuts)
    });
    assert!(app.exists(cx, "shortcuts-panel"));
    assert!(app.exists(cx, "shortcuts-direct-actions-note"));
    assert!(app.exists(cx, "shortcut-row-fetch"));
    assert!(app.exists(cx, "shortcut-row-pull"));
    assert!(!app.exists(cx, "shortcuts-conflict-banner"));

    // Click to record Fetch, press Escape to cancel without changing it.
    app.click(cx, "shortcut-record-fetch");
    assert_eq!(
        app.label(cx, "shortcut-record-fetch").as_deref(),
        Some("Press keys…")
    );
    app.in_window(cx, |window, cx| window.press("escape", cx));
    app.settle(cx);
    assert_ne!(
        app.label(cx, "shortcut-record-fetch").as_deref(),
        Some("Press keys…")
    );

    // Record an invalid bare key -> shows error and stays recording.
    app.click(cx, "shortcut-record-fetch");
    app.in_window(cx, |window, cx| window.press("a", cx));
    app.settle(cx);
    assert!(app.exists(cx, "shortcuts-recording-error"));
    assert_eq!(
        app.label(cx, "shortcut-record-fetch").as_deref(),
        Some("Press keys…")
    );

    // Record a valid shortcut with an arrow key (`secondary-shift-down`).
    app.in_window(cx, |window, cx| window.press("secondary-shift-down", cx));
    app.settle(cx);
    assert!(!app.exists(cx, "shortcuts-recording-error"));
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .keyboard_shortcuts()
            .get("fetch")
            .cloned()),
        Some("mod+shift+down".to_string())
    );
    assert_eq!(
        app.backend
            .storage()
            .get_settings()
            .keyboard_shortcuts
            .get("fetch")
            .map(String::as_str),
        Some("mod+shift+down")
    );

    // Switch to Help and verify the cheatsheet reflects the customized Fetch shortcut.
    app.click(cx, "help");
    let expected_fetch = mergeon_app::model_types::keyboard_shortcuts::combo_label(
        "mod+shift+down",
        cfg!(target_os = "macos"),
    );
    assert_eq!(
        app.label(cx, "Fetch").as_deref(),
        Some(format!("Fetch: {expected_fetch}").as_str())
    );

    // Back to Shortcuts: create a conflict by binding Pull to Fetch's custom combo.
    app.click(cx, "shortcuts");
    app.click(cx, "shortcut-record-pull");
    app.in_window(cx, |window, cx| window.press("secondary-shift-down", cx));
    app.settle(cx);
    assert!(app.exists(cx, "shortcuts-conflict-banner"));
    assert!(app.exists(cx, "shortcut-conflict-fetch"));
    assert!(app.exists(cx, "shortcut-conflict-pull"));

    // Clear (unbind) Pull via Backspace during recording.
    app.click(cx, "shortcut-record-pull");
    app.in_window(cx, |window, cx| window.press("backspace", cx));
    app.settle(cx);
    assert_eq!(
        app.label(cx, "shortcut-record-pull").as_deref(),
        Some("Unassigned")
    );
    assert!(!app.exists(cx, "shortcuts-conflict-banner"));

    // Reset Fetch individually, then Reset all to restore Pull too.
    app.click(cx, "shortcut-reset-fetch");
    assert!(!app.model.read_with(cx, |model, _| {
        model.keyboard_shortcuts().contains_key("fetch")
    }));
    assert!(app.model.read_with(cx, |model, _| {
        model.keyboard_shortcuts().contains_key("pull")
    }));

    app.click(cx, "shortcuts-reset-all");
    assert!(
        app.model
            .read_with(cx, |model, _| model.keyboard_shortcuts().is_empty())
    );
}
