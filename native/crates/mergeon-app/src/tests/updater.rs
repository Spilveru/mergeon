//! Tests de humo del store `updater` y de `useUpdater` (cuya lógica pura se
//! prueba en `mergeon-updater`), con un actualizador guionizado y con el
//! adaptador real sobre `AppUpdater`.

use std::sync::{Arc, Mutex};

use gpui_kit::TestAppContext;
use mergeon_backend::{UpdaterBridge, UpdaterEventSink};
use mergeon_model::notify::{NotifyType, ToastActionKind, ToastId};
use mergeon_updater::{AppUpdater, Arch, Env, Platform, UpdateStatus, UpdaterConfig};
use serde_json::{Value, json};

use super::support::TestApp;
use crate::AppUpdaterBridge;
use crate::testing::TestBackend;
use crate::updater::{UPDATE_ACTION_INSTALL, UPDATE_ACTION_OPEN_RELEASES};

/// Actualizador guionizado: el test decide el estado y lo publica.
#[derive(Default)]
struct ScriptedUpdater {
    state: Mutex<Value>,
    sink: Mutex<Option<UpdaterEventSink>>,
    checks: Mutex<Vec<bool>>,
    installs: Mutex<usize>,
}

impl ScriptedUpdater {
    fn publish(&self, state: Value) {
        *self.state.lock().unwrap() = state.clone();
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink(state);
        }
    }
}

impl UpdaterBridge for ScriptedUpdater {
    fn get_state(&self) -> Value {
        self.state.lock().unwrap().clone()
    }

    fn check_for_updates(&self, manual: bool) {
        self.checks.lock().unwrap().push(manual);
        self.publish(json!({
            "status": "not-available",
            "currentVersion": "0.9.0",
            "latestVersion": "0.9.0",
            "manualCheck": manual
        }));
    }

    fn install_update(&self) -> Value {
        *self.installs.lock().unwrap() += 1;
        json!({ "ok": false, "reason": "unsupported" })
    }

    fn subscribe(&self, sink: UpdaterEventSink) {
        *self.sink.lock().unwrap() = Some(sink);
    }
}

fn scripted(cx: &mut TestAppContext) -> (TestApp, Arc<ScriptedUpdater>) {
    let updater = Arc::new(ScriptedUpdater::default());
    updater.publish(json!({ "status": "idle", "currentVersion": "0.9.0" }));
    let app = TestApp::with_backend(TestBackend::new().with_updater(updater.clone()), cx);
    (app, updater)
}

fn status(app: &TestApp, cx: &mut TestAppContext) -> UpdateStatus {
    app.model
        .read_with(cx, |model, _| model.updater_state().status)
}

#[gpui_kit::test]
async fn reflects_the_updater_state_and_announces_a_manual_check(cx: &mut TestAppContext) {
    let (app, updater) = scripted(cx);

    app.model
        .update(cx, |model, cx| model.load_updater_state(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.updater_state().status, UpdateStatus::Idle);
        assert_eq!(
            model.updater_state().current_version.as_deref(),
            Some("0.9.0")
        );
    });

    // Con el estado en reposo, el botón de Settings vuelve a comprobar.
    app.model
        .update(cx, |model, cx| model.activate_update(cx))
        .await;
    app.wait_for(cx, "the check result", |model| {
        model.updater_state().status == UpdateStatus::NotAvailable
    });

    assert_eq!(*updater.checks.lock().unwrap(), [true]);
    let toasts = app.host.toasts();
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0].kind, NotifyType::Info);
    assert_eq!(
        toasts[0].title,
        "You are up to date: the latest version is 0.9.0."
    );
    // Los avisos del actualizador comparten id: uno nuevo reemplaza al anterior.
    assert_eq!(toasts[0].id, ToastId::from("app-update"));
    app.model.read_with(cx, |model, _| {
        assert!(model.updater_state().is_up_to_date())
    });
}

#[gpui_kit::test]
async fn offers_to_restart_once_when_a_download_is_ready(cx: &mut TestAppContext) {
    let (app, updater) = scripted(cx);
    let downloaded = json!({
        "status": "downloaded",
        "currentVersion": "0.9.0",
        "latestVersion": "1.0.0",
        "canAutoInstall": true
    });

    updater.publish(downloaded.clone());
    updater.publish(downloaded);
    app.wait_for(cx, "the downloaded state", |model| {
        model.updater_state().is_ready()
    });
    cx.run_until_parked();

    // La misma versión no se anuncia dos veces.
    let toasts = app.host.toasts();
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0].kind, NotifyType::Success);
    assert_eq!(toasts[0].title, "Version 1.0.0 is ready to install.");
    let action = toasts[0].action.as_ref().expect("restart action");
    assert_eq!(action.label, "Restart");
    assert_eq!(
        action.kind,
        ToastActionKind::Custom(UPDATE_ACTION_INSTALL.to_string())
    );

    // El botón del aviso instala; como esta plataforma no se instala sola, se
    // cae a la página de descargas.
    assert!(app.model.update(cx, |model, cx| {
        assert!(!model.run_toast_action("something-else", cx));
        model.run_toast_action(UPDATE_ACTION_INSTALL, cx)
    }));
    cx.run_until_parked();
    assert_eq!(*updater.installs.lock().unwrap(), 1);
    assert_eq!(
        app.launcher.launched_args(),
        ["https://github.com/Spilveru/mergeon/releases/latest"]
    );

    // Con la descarga lista y auto-instalación, Settings también instala.
    app.model
        .update(cx, |model, cx| model.activate_update(cx))
        .await;
    assert_eq!(*updater.installs.lock().unwrap(), 2);
}

#[gpui_kit::test]
async fn without_an_updater_there_is_nothing_to_reflect(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.model
        .update(cx, |model, cx| model.load_updater_state(cx))
        .await;
    app.model
        .update(cx, |model, cx| model.check_for_updates(cx))
        .await;
    assert_eq!(status(&app, cx), UpdateStatus::Idle);

    // Instalar sin actualizador lleva a las descargas.
    app.model
        .update(cx, |model, cx| model.install_update(cx))
        .await;
    assert_eq!(app.launcher.launched().len(), 1);
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn the_real_updater_reports_through_the_bridge(cx: &mut TestAppContext) {
    let cache = tempfile::tempdir().unwrap();
    // Empaquetada pero sin canal de actualizaciones configurado.
    let updater = Arc::new(AppUpdater::new(UpdaterConfig::from_env(
        Some("0.9.0".to_string()),
        true,
        Platform::current(),
        Arch::current(),
        &Env::new(),
        cache.path(),
    )));
    let bridge = Arc::new(AppUpdaterBridge::new(updater.clone()));
    assert!(Arc::ptr_eq(bridge.updater(), &updater));
    let app = TestApp::with_backend(TestBackend::new().with_updater(bridge), cx);

    app.model
        .update(cx, |model, cx| model.check_for_updates(cx))
        .await;
    app.wait_for(cx, "the updater event", |model| {
        model.updater_state().status == UpdateStatus::NotConfigured
    });
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.updater_state().current_version.as_deref(),
            Some("0.9.0")
        );
        assert!(model.updater_state().manual_check);
    });
    let toasts = app.host.toasts();
    let notice = toasts.last().expect("manual check notice");
    assert_eq!(
        notice.title,
        "This build does not update itself: download the latest version from the releases page."
    );
    assert_eq!(
        notice.action.as_ref().map(|action| action.kind.clone()),
        Some(ToastActionKind::Custom(
            UPDATE_ACTION_OPEN_RELEASES.to_string()
        ))
    );

    // Sin canal, el botón de Settings lleva directo a las descargas.
    app.model
        .update(cx, |model, cx| model.activate_update(cx))
        .await;
    assert_eq!(app.launcher.launched().len(), 1);
}
