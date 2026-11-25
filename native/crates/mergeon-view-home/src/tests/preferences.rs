//! `PreferencesDialog.test.js`.

use gpui_kit::{Entity, TestAppContext};
use mergeon_app::backend::ExternalEditorSettings;
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::testing::TestBackend;
use mergeon_ui::SelectOption;

use super::support::{TestApp, Workspace, git_in, reset_global_identity};
use crate::{PreferencesPanel, PreferencesSection, editor_options};

/// Un repositorio abierto y el panel con sus tres secciones, como el diálogo.
fn preferences(cx: &mut TestAppContext) -> (TestApp, Workspace, String, Entity<PreferencesPanel>) {
    let workspace = Workspace::new();
    let repository = workspace.repository("work/demo", None);
    let app = TestApp::new(cx);
    app.start(cx);
    app.open_repository(&repository, cx);
    let panel = app.show(cx, |window, cx| {
        PreferencesPanel::new(PreferencesSection::All, window, cx)
    });
    (app, workspace, repository, panel)
}

fn form(panel: &Entity<PreferencesPanel>, cx: &mut TestAppContext) -> (String, String) {
    panel.read_with(cx, |panel, cx| {
        (
            panel.identity_name(cx).to_string(),
            panel.identity_email(cx).to_string(),
        )
    })
}

#[gpui_kit::test]
async fn loads_the_identity_and_prefills_the_global_scope(cx: &mut TestAppContext) {
    let _identity = reset_global_identity();
    let (_app, _workspace, _repository, panel) = preferences(cx);

    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.identity_scope(), "global");
        assert!(!panel.is_loading_identity());
    });
    assert_eq!(
        form(&panel, cx),
        ("Global User".to_string(), "global@example.com".to_string())
    );
}

#[gpui_kit::test]
async fn switching_to_repository_scope_shows_the_local_empty_identity(cx: &mut TestAppContext) {
    let _identity = reset_global_identity();
    let (app, _workspace, _repository, panel) = preferences(cx);
    assert_eq!(form(&panel, cx).0, "Global User");
    assert!(!app.exists(cx, "prefs-inherit-hint"));

    app.click_within(cx, "prefs-identity-scope", "radio-local");
    panel.read_with(cx, |panel, _| assert_eq!(panel.identity_scope(), "local"));

    assert_eq!(form(&panel, cx), (String::new(), String::new()));
    // "Empty fields inherit the global identity."
    assert!(app.exists(cx, "prefs-inherit-hint"));
    // El valor heredado se enseña como sugerencia.
    panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.identity_placeholders(),
            ("Global User".into(), "global@example.com".into())
        );
    });
}

#[gpui_kit::test]
async fn saves_the_identity_with_the_selected_scope(cx: &mut TestAppContext) {
    let _identity = reset_global_identity();
    let (app, _workspace, repository, panel) = preferences(cx);
    assert_eq!(form(&panel, cx).0, "Global User");

    app.in_window(cx, |window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .name
                .update(cx, |state, cx| state.set_value("New Name", window, cx));
            panel.email.update(cx, |state, cx| {
                state.set_value("new@example.com", window, cx)
            });
        });
    });
    app.click(cx, "prefs-save-identity");

    let repo = std::path::Path::new(&repository);
    assert_eq!(
        git_in(repo, &["config", "--global", "user.name"]).trim(),
        "New Name"
    );
    assert_eq!(
        git_in(repo, &["config", "--global", "user.email"]).trim(),
        "new@example.com"
    );
    // Tras guardar se vuelve a leer la identidad.
    assert_eq!(
        form(&panel, cx),
        ("New Name".to_string(), "new@example.com".to_string())
    );

    // En el ámbito del repositorio se guarda en su propia configuración.
    app.in_window(cx, |window, cx| {
        panel.update(cx, |panel, cx| {
            panel.set_identity_scope("local".into(), window, cx);
            panel
                .name
                .update(cx, |state, cx| state.set_value("Repo Name", window, cx));
        });
    });
    app.click(cx, "prefs-save-identity");
    assert_eq!(
        git_in(repo, &["config", "--local", "user.name"]).trim(),
        "Repo Name"
    );
    assert_eq!(
        git_in(repo, &["config", "--global", "user.name"]).trim(),
        "New Name"
    );
}

#[gpui_kit::test]
async fn persists_auto_fetch_changes_as_they_are_made(cx: &mut TestAppContext) {
    let (app, _workspace, _repository, _panel) = preferences(cx);
    assert!(app.backend.storage().get_settings().auto_fetch);

    app.click(cx, "prefs-auto-fetch");

    let settings = app.backend.storage().get_settings();
    assert!(!settings.auto_fetch);
    assert_eq!(settings.effective_auto_fetch_interval_minutes(), 10);
    app.model.read_with(cx, |model, _| {
        assert!(!model.auto_fetch_enabled());
        assert_eq!(model.auto_fetch_interval_minutes(), 10);
    });

    app.click(cx, "prefs-auto-fetch");
    assert!(app.backend.storage().get_settings().auto_fetch);
}

#[gpui_kit::test]
async fn desktop_notifications_are_on_by_default_and_can_be_turned_off(cx: &mut TestAppContext) {
    let (app, _workspace, _repository, _panel) = preferences(cx);
    assert!(app.exists(cx, "prefs-notifications-section"));
    assert!(
        app.model
            .read_with(cx, |model, _| model.desktop_notifications_enabled())
    );

    app.click(cx, "prefs-desktop-notifications");

    assert!(
        !app.model
            .read_with(cx, |model, _| model.desktop_notifications_enabled())
    );
    assert_eq!(
        app.backend
            .storage()
            .get_settings()
            .extra
            .get("desktopNotifications"),
        Some(&serde_json::Value::Bool(false))
    );

    app.click(cx, "prefs-desktop-notifications");
    assert!(
        app.model
            .read_with(cx, |model, _| model.desktop_notifications_enabled())
    );
}

#[gpui_kit::test]
async fn without_a_repository_the_identity_cannot_be_edited(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.start(cx);
    let panel = app.show(cx, |window, cx| {
        PreferencesPanel::new(PreferencesSection::Git, window, cx)
    });

    assert_eq!(form(&panel, cx), (String::new(), String::new()));
    panel.read_with(cx, |panel, _| assert!(!panel.is_loading_identity()));

    // El botón está deshabilitado: no guarda ni avisa.
    app.click(cx, "prefs-save-identity");
    assert!(app.host.messages(NotifyType::Warning).is_empty());
    assert!(app.host.messages(NotifyType::Success).is_empty());
}

#[gpui_kit::test]
async fn the_theme_is_applied_and_saved_as_it_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.start(cx);
    let _panel = app.show(cx, |window, cx| {
        PreferencesPanel::new(PreferencesSection::Appearance, window, cx)
    });
    // Una instalación nueva arranca en el tema claro.
    assert!(!app.model.read_with(cx, |model, _| model.is_dark()));
    // En Appearance solo está el tema.
    assert!(app.exists(cx, "prefs-theme-section"));
    assert!(!app.exists(cx, "prefs-identity"));
    assert!(!app.exists(cx, "prefs-auto-fetch-section"));

    app.click_within(cx, "prefs-theme", "radio-dark");

    assert!(
        app.events()
            .contains(&mergeon_app::AppEvent::ThemeChanged { dark: true })
    );
    assert_eq!(
        app.backend.storage().get_settings().theme,
        mergeon_store::Theme::Dark
    );
}

#[gpui_kit::test]
async fn the_identity_follows_the_active_repository(cx: &mut TestAppContext) {
    let _identity = reset_global_identity();
    let (app, workspace, _repository, panel) = preferences(cx);
    let other = workspace.repository("work/other", None);
    git_in(
        std::path::Path::new(&other),
        &["config", "user.name", "Other Author"],
    );

    app.open_repository(&other, cx);
    app.in_window(cx, |window, cx| {
        panel.update(cx, |panel, cx| {
            panel.set_identity_scope("local".into(), window, cx);
        });
    });
    app.settle(cx);

    assert_eq!(form(&panel, cx).0, "Other Author");
}

// ---------------------------------------------------------------------------
// Editor externo
// ---------------------------------------------------------------------------

/// El panel de Settings → Git sobre un sistema con `programs` instalados.
fn editor_panel(programs: &[&str], cx: &mut TestAppContext) -> (TestApp, Entity<PreferencesPanel>) {
    let app = TestApp::with_backend(TestBackend::new().with_installed(programs), cx);
    app.start(cx);
    let panel = app.show(cx, |window, cx| {
        PreferencesPanel::new(PreferencesSection::Git, window, cx)
    });
    (app, panel)
}

fn labels(options: &[SelectOption]) -> Vec<String> {
    options
        .iter()
        .map(|option| option.label().to_string())
        .collect()
}

#[gpui_kit::test]
async fn the_git_section_lists_the_detected_editors(cx: &mut TestAppContext) {
    let (app, panel) = editor_panel(&["subl", "code"], cx);
    assert!(app.exists(cx, "prefs-editor-section"));
    assert!(app.exists(cx, "prefs-editor"));
    // Sin comando propio no hay campos de comando.
    assert!(!app.exists(cx, "prefs-save-editor"));

    let options = panel.read_with(cx, |panel, _| {
        assert_eq!(panel.editor_choice().as_ref(), "auto");
        editor_options(panel.detected_editors(), panel.editor_choice())
    });
    assert_eq!(
        labels(&options),
        [
            "Automatic (Visual Studio Code)",
            "Visual Studio Code",
            "Sublime Text",
            "System default app",
            "Custom command…",
        ]
    );
    assert_eq!(
        app.label(cx, "prefs-editor-hint").as_deref(),
        Some("Opens files with Visual Studio Code (/test-bin/code).")
    );

    // Appearance no lo muestra.
    let appearance = app.show(cx, |window, cx| {
        PreferencesPanel::new(PreferencesSection::Appearance, window, cx)
    });
    let _ = appearance;
    assert!(!app.exists(cx, "prefs-editor-section"));
}

#[gpui_kit::test]
async fn choosing_an_editor_saves_it_and_open_in_editor_uses_it(cx: &mut TestAppContext) {
    let (app, panel) = editor_panel(&["code", "zed"], cx);

    panel.update(cx, |panel, cx| panel.choose_editor("zed".into(), cx));
    app.settle(cx);

    assert_eq!(
        app.backend.storage().get_settings().external_editor,
        ExternalEditorSettings::editor("zed")
    );
    app.model
        .update(cx, |model, cx| {
            model.open_in_editor("/work/my repo/src/main.rs", Some(12), cx)
        })
        .detach();
    app.settle(cx);
    let launched = app.launcher.launched();
    assert_eq!(launched.len(), 1, "{launched:?}");
    assert_eq!(launched[0].command, "/test-bin/zed");
    assert_eq!(launched[0].args, ["/work/my repo/src/main.rs:12:1"]);
}

#[gpui_kit::test]
async fn a_custom_command_is_saved_with_its_arguments(cx: &mut TestAppContext) {
    let (app, panel) = editor_panel(&[], cx);
    assert_eq!(
        app.label(cx, "prefs-editor-hint").as_deref(),
        Some("No editor detected: files open with the system's default app.")
    );

    panel.update(cx, |panel, cx| panel.choose_editor("custom".into(), cx));
    app.settle(cx);
    // Elegir "Custom command…" sin comando todavía no guarda nada.
    assert_eq!(
        app.backend.storage().get_settings().external_editor,
        ExternalEditorSettings::default()
    );
    assert!(app.exists(cx, "prefs-save-editor"));

    app.in_window(cx, |window, cx| {
        panel.update(cx, |panel, cx| {
            panel.editor_command.update(cx, |state, cx| {
                state.set_value("/opt/My Editor/edit", window, cx)
            });
            panel.editor_args.update(cx, |state, cx| {
                state.set_value("-l {line} \"{path}\"", window, cx)
            });
        });
    });
    app.click(cx, "prefs-save-editor");

    let custom = ExternalEditorSettings::custom("/opt/My Editor/edit", "-l {line} \"{path}\"");
    assert_eq!(app.backend.storage().get_settings().external_editor, custom);
    app.model
        .read_with(cx, |model, _| assert_eq!(model.external_editor(), &custom));

    app.model
        .update(cx, |model, cx| {
            model.open_in_editor("/r/a b.txt", Some(4), cx)
        })
        .detach();
    app.settle(cx);
    let launched = app.launcher.launched();
    assert_eq!(launched[0].command, "/opt/My Editor/edit");
    assert_eq!(launched[0].args, ["-l", "4", "/r/a b.txt"]);

    // Pasar a otro editor conserva el comando, y volver a "Custom command…"
    // lo recupera sin tener que escribirlo otra vez.
    panel.update(cx, |panel, cx| panel.choose_editor("system".into(), cx));
    app.settle(cx);
    assert_eq!(
        app.backend.storage().get_settings().external_editor,
        ExternalEditorSettings {
            editor: "system".to_string(),
            ..custom.clone()
        }
    );
    panel.update(cx, |panel, cx| panel.choose_editor("custom".into(), cx));
    app.settle(cx);
    assert_eq!(app.backend.storage().get_settings().external_editor, custom);

    // Un panel nuevo (otra visita a Settings) muestra lo guardado.
    let again = app.show(cx, |window, cx| {
        PreferencesPanel::new(PreferencesSection::Git, window, cx)
    });
    again.read_with(cx, |panel, cx| {
        assert_eq!(panel.editor_choice().as_ref(), "custom");
        assert_eq!(
            panel.editor_command.read(cx).value().as_ref(),
            "/opt/My Editor/edit"
        );
    });
}

#[gpui_kit::test]
async fn a_missing_editor_is_reported_clearly(cx: &mut TestAppContext) {
    let (app, panel) = editor_panel(&[], cx);
    panel.update(cx, |panel, cx| panel.choose_editor("sublime".into(), cx));
    app.settle(cx);
    assert_eq!(
        app.label(cx, "prefs-editor-hint").as_deref(),
        Some("Not found on this system: install it or choose another editor.")
    );
    let options = panel.read_with(cx, |panel, _| {
        editor_options(panel.detected_editors(), panel.editor_choice())
    });
    assert!(labels(&options).contains(&"Sublime Text (not found)".to_string()));

    app.launcher
        .fail_with(mergeon_app::backend::LaunchError::not_found("subl"));
    app.model
        .update(cx, |model, cx| model.open_in_editor("/r/a.txt", None, cx))
        .detach();
    app.settle(cx);
    assert!(app.has_toast(
        NotifyType::Warning,
        "Sublime Text was not found (subl). Install it or choose another editor in Settings → Git → External editor."
    ));
}

/// Elegir "Custom command…" en el desplegable deja el cursor en "Command"
/// (QA 0.12.0: el primer clic en el campo recién aparecido se perdía).
#[gpui_kit::test]
async fn choosing_a_custom_command_focuses_the_command_field(cx: &mut TestAppContext) {
    use gpui_kit::Focusable as _;
    use gpui_kit::test::TestWindowExt as _;

    let (app, panel) = editor_panel(&[], cx);
    app.click(cx, "prefs-editor");
    // Automatic, System default app, Custom command…
    app.in_window(cx, |window, cx| {
        window.within("popup-menu").click(2, cx);
    });
    app.settle(cx);

    assert!(app.exists(cx, "prefs-save-editor"));
    let focused = app.in_window(cx, |window, cx| {
        panel
            .read(cx)
            .editor_command
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    });
    assert!(focused, "the Command field should have the focus");
}
