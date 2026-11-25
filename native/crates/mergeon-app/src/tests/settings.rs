//! Port de `src/composables/useTheme.test.js` y tests de humo del resto de
//! ajustes (`useAppSettings`, `useDiffPreferences`, anchos, secciones de la
//! barra lateral), cuya lógica pura ya se prueba en `mergeon-model`.

use std::sync::Arc;

use gpui_kit::TestAppContext;
use mergeon_backend::{PlainTextCipher, Store, StoreOptions};
use mergeon_model::diff_preferences::DiffViewMode;
use mergeon_model::notify::NotifyType;
use mergeon_store::Theme;
use serde_json::{Value, json};

use super::support::TestApp;
use crate::AppEvent;
use crate::testing::TestBackend;

fn app_with_settings(settings: Value, cx: &mut TestAppContext) -> TestApp {
    let store = TestBackend::memory_store();
    store.update_settings_json(&settings).unwrap();
    TestApp::with_backend(TestBackend::new().with_store(store), cx)
}

fn theme_changes(app: &TestApp) -> Vec<bool> {
    app.events()
        .into_iter()
        .filter_map(|event| match event {
            AppEvent::ThemeChanged { dark } => Some(dark),
            _ => None,
        })
        .collect()
}

// --- useTheme.test.js --------------------------------------------------------

#[gpui_kit::test]
async fn loads_the_stored_light_theme_and_applies_it(cx: &mut TestAppContext) {
    let app = app_with_settings(json!({ "theme": "light" }), cx);
    app.model.read_with(cx, |model, _| assert!(model.is_dark()));

    app.model.update(cx, |model, cx| model.load_theme(cx)).await;

    app.model
        .read_with(cx, |model, _| assert!(!model.is_dark()));
    assert_eq!(theme_changes(&app), [false]);
}

#[gpui_kit::test]
async fn persists_the_updated_theme_when_toggled(cx: &mut TestAppContext) {
    let app = app_with_settings(json!({ "theme": "dark" }), cx);

    app.model.update(cx, |model, cx| model.load_theme(cx)).await;
    app.model.read_with(cx, |model, _| assert!(model.is_dark()));

    app.model
        .update(cx, |model, cx| model.toggle_theme(cx))
        .await;

    app.model
        .read_with(cx, |model, _| assert!(!model.is_dark()));
    assert_eq!(app.backend.storage().get_settings().theme, Theme::Light);
    assert!(app.host.toasts().is_empty());
}

// En el original era `setTitleBarTheme` en Windows; aquí la cáscara aplica el
// tema (también a los controles de la ventana) al recibir el evento.
#[gpui_kit::test]
async fn announces_the_loaded_theme_and_later_changes_to_the_window(cx: &mut TestAppContext) {
    let app = app_with_settings(json!({ "theme": "dark" }), cx);

    app.model.update(cx, |model, cx| model.load_theme(cx)).await;
    assert_eq!(theme_changes(&app), [true]);

    app.model
        .update(cx, |model, cx| model.toggle_theme(cx))
        .await;
    assert_eq!(theme_changes(&app), [true, false]);

    // Pedir el tema que ya está puesto no hace nada.
    app.model
        .update(cx, |model, cx| model.set_dark(false, cx))
        .await;
    assert_eq!(theme_changes(&app), [true, false]);
}

#[gpui_kit::test]
async fn warns_when_a_setting_cannot_be_saved(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let settings_directory = directory.path().join("config");
    std::fs::create_dir(&settings_directory).unwrap();
    let store = Store::open(
        settings_directory.join("settings.json"),
        StoreOptions::new(Arc::new(PlainTextCipher)).without_electron_import(),
    )
    .unwrap();
    let app = TestApp::with_backend(TestBackend::new().with_store(store), cx);

    // El directorio de la configuración deja de existir: no se puede escribir.
    std::fs::remove_dir_all(&settings_directory).unwrap();
    std::fs::write(&settings_directory, "not a directory").unwrap();

    app.model
        .update(cx, |model, cx| model.toggle_theme(cx))
        .await;

    // El cambio se ve igualmente; el aviso dice que puede no persistir.
    app.model
        .read_with(cx, |model, _| assert!(!model.is_dark()));
    assert_eq!(
        app.host.messages(NotifyType::Warning),
        ["Failed to save the theme setting. The change may not persist after restarting."]
    );
}

// --- useAppSettings ------------------------------------------------------------

#[gpui_kit::test]
async fn loads_the_auto_fetch_settings_once_per_session(cx: &mut TestAppContext) {
    let app = app_with_settings(
        json!({ "autoFetch": false, "autoFetchIntervalMinutes": 15 }),
        cx,
    );
    app.model.read_with(cx, |model, _| {
        assert!(model.auto_fetch_enabled());
        assert_eq!(model.auto_fetch_interval_minutes(), 10);
    });

    app.model
        .update(cx, |model, cx| model.load_app_settings(false, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(!model.auto_fetch_enabled());
        assert_eq!(model.auto_fetch_interval_minutes(), 15);
    });

    // Otra carga no relee salvo que se fuerce.
    app.backend
        .storage()
        .update_settings_json(&json!({ "autoFetch": true, "autoFetchIntervalMinutes": 30 }))
        .unwrap();
    app.model
        .update(cx, |model, cx| model.load_app_settings(false, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.auto_fetch_interval_minutes(), 15)
    });

    app.model
        .update(cx, |model, cx| model.load_app_settings(true, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(model.auto_fetch_enabled());
        assert_eq!(model.auto_fetch_interval_minutes(), 30);
    });
}

#[gpui_kit::test]
async fn saves_the_auto_fetch_settings_normalized(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.model
        .update(cx, |model, cx| {
            model.save_auto_fetch_settings(false, 2.6, cx)
        })
        .await;

    app.model.read_with(cx, |model, _| {
        assert!(!model.auto_fetch_enabled());
        assert_eq!(model.auto_fetch_interval_minutes(), 3);
    });
    let stored = app.backend.storage().get_settings();
    assert!(!stored.auto_fetch);
    assert_eq!(stored.effective_auto_fetch_interval_minutes(), 3);

    // Un intervalo que no es válido vuelve al de por defecto.
    app.model
        .update(cx, |model, cx| {
            model.save_auto_fetch_settings(true, 0.0, cx)
        })
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.auto_fetch_interval_minutes(), 10)
    });
}

// --- useDiffPreferences ----------------------------------------------------------

#[gpui_kit::test]
async fn loads_and_persists_the_diff_preferences(cx: &mut TestAppContext) {
    let app = app_with_settings(
        json!({ "commitView": { "diffViewMode": "split", "ignoreWhitespace": true } }),
        cx,
    );

    app.model
        .update(cx, |model, cx| model.load_diff_preferences(false, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.diff_preferences().diff_view_mode, DiffViewMode::Split);
        assert!(model.diff_preferences().ignore_whitespace);
    });

    app.model
        .update(cx, |model, cx| model.toggle_diff_view(cx))
        .await;
    app.model
        .update(cx, |model, cx| model.toggle_ignore_whitespace(cx))
        .await;

    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.diff_preferences().diff_view_mode,
            DiffViewMode::Unified
        );
        assert!(!model.diff_preferences().ignore_whitespace);
    });
    let stored = app.backend.storage().get_settings().commit_view;
    assert_eq!(stored.diff_view_mode, mergeon_store::DiffViewMode::Unified);
    assert!(!stored.ignore_whitespace);
}

// "Whole file" se recuerda entre sesiones como las demás preferencias del diff.
#[gpui_kit::test]
async fn remembers_whole_file_between_sessions(cx: &mut TestAppContext) {
    let app = app_with_settings(json!({ "commitView": { "diffViewMode": "split" } }), cx);
    app.model
        .update(cx, |model, cx| model.load_diff_preferences(false, cx))
        .await;
    app.model
        .read_with(cx, |model, _| assert!(!model.diff_preferences().whole_file));

    app.model
        .update(cx, |model, cx| model.toggle_whole_file(cx))
        .await;

    app.model.read_with(cx, |model, _| {
        assert!(model.diff_preferences().whole_file);
        // Lo demás no cambia.
        assert_eq!(model.diff_preferences().diff_view_mode, DiffViewMode::Split);
    });
    let stored = app.backend.storage().get_settings().commit_view;
    assert_eq!(stored.extra.get("wholeFile"), Some(&json!(true)));
    assert_eq!(stored.diff_view_mode, mergeon_store::DiffViewMode::Split);

    // Otra sesión lo lee de los ajustes guardados.
    app.model
        .update(cx, |model, cx| model.load_diff_preferences(true, cx))
        .await;
    app.model
        .read_with(cx, |model, _| assert!(model.diff_preferences().whole_file));
}

// --- useSidebarWidth / useFileListWidth -------------------------------------------

#[gpui_kit::test]
async fn the_sidebar_width_is_loaded_dragged_and_saved_on_release(cx: &mut TestAppContext) {
    let app = app_with_settings(json!({ "sidebarWidth": 320 }), cx);

    app.model
        .update(cx, |model, cx| model.load_sidebar_width(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.sidebar_width().width, 320.0)
    });

    // Mientras se arrastra solo cambia el ancho en memoria, y sin avisar a
    // los observadores a cada movimiento: quien arrastra repinta.
    app.model
        .update(cx, |model, cx| model.start_sidebar_resize(cx));
    cx.run_until_parked();
    let notifications = app.count_notifications(cx);
    for pointer_x in [380.0, 390.0, 400.0] {
        let changed = app.model.update(cx, |model, _| {
            model.drag_sidebar_resize(pointer_x, 0.0, 1200.0)
        });
        assert!(changed);
    }
    // Un movimiento que no cambia el ancho (ya está en el máximo o el mínimo)
    // no pide repintar.
    let unchanged = app
        .model
        .update(cx, |model, _| model.drag_sidebar_resize(400.0, 0.0, 1200.0));
    assert!(!unchanged);
    cx.run_until_parked();
    assert_eq!(notifications.get(), 0);
    app.model.read_with(cx, |model, _| {
        assert!(model.sidebar_width().resizing);
        assert_eq!(model.sidebar_width().width, 400.0);
    });
    assert_eq!(
        app.backend.storage().get_settings().sidebar_width,
        Some(320.0)
    );

    app.model
        .update(cx, |model, cx| model.stop_sidebar_resize(cx))
        .await;
    assert_eq!(
        app.backend.storage().get_settings().sidebar_width,
        Some(400.0)
    );
    // Al soltar, un aviso.
    assert_eq!(notifications.get(), 1);

    app.model
        .update(cx, |model, cx| model.nudge_sidebar_width(-16.0, 1200.0, cx))
        .await;
    assert_eq!(
        app.backend.storage().get_settings().sidebar_width,
        Some(384.0)
    );

    app.model
        .update(cx, |model, cx| model.reset_sidebar_width(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.sidebar_width().width, 260.0)
    });
    assert_eq!(
        app.backend.storage().get_settings().sidebar_width,
        Some(260.0)
    );
}

#[gpui_kit::test]
async fn the_file_list_width_is_loaded_and_saved(cx: &mut TestAppContext) {
    let app = app_with_settings(json!({ "commitView": { "fileListWidth": 300 } }), cx);

    app.model
        .update(cx, |model, cx| model.load_file_list_width(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.file_list_width().width, 300.0)
    });

    app.model
        .update(cx, |model, cx| model.nudge_file_list_width(20.0, 0.0, cx))
        .await;
    assert_eq!(
        app.backend
            .storage()
            .get_settings()
            .commit_view
            .file_list_width,
        Some(320.0)
    );

    app.model.update(cx, |model, cx| {
        model.start_file_list_resize(cx);
        assert!(model.drag_file_list_resize(5000.0, 0.0, 1000.0));
    });
    app.model
        .update(cx, |model, cx| model.stop_file_list_resize(cx))
        .await;
    // Nunca se come el espacio mínimo del diff.
    assert_eq!(
        app.backend
            .storage()
            .get_settings()
            .commit_view
            .file_list_width,
        Some(640.0)
    );

    app.model
        .update(cx, |model, cx| model.reset_file_list_width(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.file_list_width().width, 240.0)
    });
}

#[gpui_kit::test]
async fn the_local_changes_preferences_are_loaded_and_saved(cx: &mut TestAppContext) {
    use crate::settings::LocalChangesPrefs;

    let app = app_with_settings(
        json!({ "commitView": { "localChangesWidth": 430, "fileListWidth": 300 } }),
        cx,
    );
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.local_changes_prefs(), LocalChangesPrefs::default())
    });

    app.model
        .update(cx, |model, cx| model.load_local_changes_prefs(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.local_changes_prefs(),
            LocalChangesPrefs {
                width: Some(430.0),
                flat_list: false,
                hints_dismissed: false,
            }
        )
    });

    app.model
        .update(cx, |model, cx| model.set_changes_list_flat(true, cx))
        .await;
    app.model
        .update(cx, |model, cx| model.dismiss_changes_hints(cx))
        .await;
    app.model
        .update(cx, |model, cx| {
            model.save_local_changes_width(Some(512.0), cx)
        })
        .await;

    // Cada preferencia se guarda sin pisar las demás (ni el resto de `commitView`).
    let saved = serde_json::to_value(app.backend.storage().get_settings()).unwrap();
    assert_eq!(saved["commitView"]["localChangesWidth"], json!(512));
    assert_eq!(saved["commitView"]["changesListLayout"], json!("flat"));
    assert_eq!(saved["commitView"]["changesHintsDismissed"], json!(true));
    assert_eq!(saved["commitView"]["fileListWidth"], json!(300));
    assert_eq!(
        LocalChangesPrefs::from_settings(&saved),
        LocalChangesPrefs {
            width: Some(512.0),
            flat_list: true,
            hints_dismissed: true,
        }
    );

    // `None` vuelve al ancho de diseño.
    app.model
        .update(cx, |model, cx| model.save_local_changes_width(None, cx))
        .await;
    let saved = serde_json::to_value(app.backend.storage().get_settings()).unwrap();
    assert_eq!(saved["commitView"]["localChangesWidth"], json!(null));
}

// --- CommitView.vue / Sidebar.vue ----------------------------------------------------

#[gpui_kit::test]
async fn the_history_width_and_sidebar_sections_round_trip(cx: &mut TestAppContext) {
    let app = app_with_settings(
        json!({
            "commitView": { "historyWidth": 480 },
            "sidebarSections": { "tags": true }
        }),
        cx,
    );

    let width = app
        .model
        .update(cx, |model, cx| model.load_history_width(cx))
        .await;
    assert_eq!(width, Some(480.0));
    app.model
        .update(cx, |model, cx| model.persist_history_width(520.0, cx))
        .await;
    assert_eq!(
        app.backend
            .storage()
            .get_settings()
            .commit_view
            .history_width,
        Some(520.0)
    );

    app.model
        .update(cx, |model, cx| model.load_sidebar_sections(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        let sections = model.sidebar_sections();
        assert!(sections.tags);
        // Lo que no estaba guardado toma su valor por defecto.
        assert!(sections.branches && sections.starred);
        assert!(!sections.remotes);
    });

    app.model
        .update(cx, |model, cx| {
            model.update_sidebar_sections(cx, |sections| {
                sections.remotes = true;
                sections.branches = false;
            })
        })
        .await;
    let stored = app.backend.storage().get_settings().sidebar_sections;
    assert!(stored.remotes && stored.tags);
    assert!(!stored.branches);
}
