//! `Dashboard.test.js` y lo que el inicio hace además.

use gpui_kit::{AppContext as _, TestAppContext};
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::testing::{FakeGitHubTransport, TestBackend};
use serde_json::json;

use super::support::{TestApp, Workspace, account, repo_row, seeded_store};
use crate::HomeView;

fn install_required_account() -> mergeon_store::GitHubAccount {
    account(json!({
        "id": "account-1",
        "username": "Spilveru",
        "avatarUrl": "https://example.com/avatar.png",
        "token": "ghu_token",
        "refreshToken": "ghr_token",
        "requiresReconnect": false,
        "requiresInstallation": true,
        "installUrl": "https://github.com/apps/app-git-service/installations/new",
        "activeInstallationId": null,
        "installations": []
    }))
}

/// Un repositorio abierto (`git_gui`) y otro reciente (`backend_git_gui`),
/// como el `beforeEach` de `Dashboard.test.js`.
fn dashboard(
    cx: &mut TestAppContext,
) -> (
    TestApp,
    Workspace,
    String,
    String,
    gpui_kit::Entity<HomeView>,
) {
    let workspace = Workspace::new();
    let open = workspace.repository(
        "work/git_gui",
        Some("# Demo Project\n\n- item one\n- item two"),
    );
    let recent = workspace.repository("work/backend_git_gui", None);
    let app = TestApp::with_store(seeded_store(&[&recent], &[install_required_account()]), cx);
    app.start(cx);
    app.open_repository(&open, cx);
    let home = app.show(cx, HomeView::new);
    (app, workspace, open, recent, home)
}

#[gpui_kit::test]
async fn renders_the_repositories_tree_and_loads_the_selected_repository_summary(
    cx: &mut TestAppContext,
) {
    let (app, _workspace, open, recent, home) = dashboard(cx);

    // Los dos repositorios cuelgan de su carpeta, desplegada.
    assert!(app.exists(cx, "repo-group:work"));
    assert!(app.exists(cx, repo_row(&open)));
    assert!(app.exists(cx, repo_row(&recent)));
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(open.as_str()));
        assert_eq!(home.active_summary_tab(), "summary");
        assert!(home.is_group_expanded("work"));
    });

    // El resumen del repositorio activo se leyó y se muestra con su README.
    let summary = app
        .model
        .read_with(cx, |model, _| model.repository_summary(&open).cloned())
        .expect("resumen del repositorio activo");
    assert!(summary.is_valid);
    assert_eq!(summary.name, "git_gui");
    assert_eq!(
        summary.readme_content.as_deref(),
        Some("# Demo Project\n\n- item one\n- item two")
    );
    assert!(app.exists(cx, "summary-content"));
    assert!(app.exists(cx, "readme-panel"));
    assert!(app.exists(cx, "tab-summary"));
    assert!(app.exists(cx, "tab-statistics"));

    // El inicio no pide nada a GitHub por su cuenta: lo único que salió es el
    // refresco de sesión del arranque de la app.
    assert!(
        app.github
            .requested_urls()
            .iter()
            .all(|url| url.ends_with("/login/oauth/access_token"))
    );
}

#[gpui_kit::test]
async fn shows_the_statistics_of_the_selected_repository(cx: &mut TestAppContext) {
    let (app, _workspace, open, _recent, home) = dashboard(cx);

    app.click(cx, "tab-statistics");
    home.read_with(cx, |home, _| {
        assert_eq!(home.active_summary_tab(), "statistics")
    });

    assert!(app.exists(cx, "Changed Files"));
    assert!(app.exists(cx, "Repository Size"));
    assert!(!app.exists(cx, "readme-panel"));
    let summary = app
        .model
        .read_with(cx, |model, _| model.repository_summary(&open).cloned())
        .expect("resumen");
    let rows = crate::stat_rows(&summary);
    assert_eq!(rows[0], ("Changed Files", "0".to_string()));
    assert_eq!(rows[2], ("Commits", "1".to_string()));
    assert_eq!(rows[3], ("Remotes", "0".to_string()));

    // Elegir otro repositorio vuelve a la pestaña Summary.
    app.click(cx, repo_row(&_recent));
    home.read_with(cx, |home, _| {
        assert_eq!(home.active_summary_tab(), "summary")
    });
}

#[gpui_kit::test]
async fn selects_a_repository_and_opens_it_from_the_summary_action(cx: &mut TestAppContext) {
    let (app, _workspace, _open, recent, home) = dashboard(cx);

    app.click(cx, repo_row(&recent));
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(recent.as_str()));
    });
    assert!(
        app.model
            .read_with(cx, |model, _| model.repository_summary(&recent).is_some())
    );
    // Sin README se muestra el texto por defecto.
    assert!(app.exists(cx, "summary-text"));

    // Solo un repositorio que no está abierto ofrece abrirse.
    app.click(cx, "open-repository");
    app.wait_for(cx, "the recent repository to open", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .open_repositories()
                .iter()
                .any(|repo| repo.path == recent)
        })
    });
    assert!(app.has_toast(NotifyType::Success, "Repository opened successfully"));
}

#[gpui_kit::test]
async fn an_open_repository_does_not_offer_to_open_again(cx: &mut TestAppContext) {
    let (app, _workspace, _open, _recent, _home) = dashboard(cx);

    // El elegido al entrar es el repositorio activo, que ya está abierto.
    assert!(!app.exists(cx, "open-repository"));
}

#[gpui_kit::test]
async fn opens_a_repository_when_its_row_is_double_clicked(cx: &mut TestAppContext) {
    let (app, _workspace, _open, recent, home) = dashboard(cx);

    app.double_click(cx, repo_row(&recent));

    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(recent.as_str()));
    });
    app.wait_for(cx, "the double-clicked repository to open", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .open_repositories()
                .iter()
                .any(|repo| repo.path == recent)
        })
    });
}

#[gpui_kit::test]
async fn shows_the_invalid_repository_state_and_removes_it_from_recent(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let missing = workspace.path("work/old-prototype");
    let app = TestApp::with_store(seeded_store(&[&missing], &[]), cx);
    app.start(cx);
    let home = app.show(cx, HomeView::new);

    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(missing.as_str()));
    });
    assert!(app.exists(cx, "summary-invalid"));
    assert!(!app.exists(cx, "summary-content"));

    app.click(cx, "remove-from-recent");

    app.model.read_with(cx, |model, _| {
        assert!(model.recent_repositories().is_empty());
        assert!(model.repository_summary(&missing).is_none());
    });
    assert!(app.backend.storage().get_recent_repositories().is_empty());
    assert!(app.has_toast(NotifyType::Success, "Repository removed from recent list"));
    home.read_with(cx, |home, _| assert_eq!(home.selected_repo_path(), None));
    assert!(!app.exists(cx, "summary-invalid"));
}

#[gpui_kit::test]
async fn keeps_the_github_installation_required_state_as_a_compact_section(
    cx: &mut TestAppContext,
) {
    let (app, _workspace, _open, _recent, _home) = dashboard(cx);

    assert_eq!(
        app.label(cx, "github-note").as_deref(),
        Some("GitHub App installation required.")
    );
    assert!(app.exists(cx, "github-install-app"));
    assert!(!app.exists(cx, "github-reconnect"));

    app.click(cx, "github-install-app");
    assert!(
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/apps/app-git-service/installations/new")
    );
}

/// Respuestas de GitHub para refrescar la sesión de `Spilveru` con éxito.
fn script_successful_refresh(github: &FakeGitHubTransport) {
    github.respond_with(|request| {
        if request.url.ends_with("/login/oauth/access_token") {
            return FakeGitHubTransport::json(
                200,
                json!({ "access_token": "ghu_new", "refresh_token": "ghr_new", "expires_in": 28800 }),
            );
        }
        if request.url.ends_with("/user") {
            return FakeGitHubTransport::json(
                200,
                json!({ "id": "account-1", "login": "Spilveru" }),
            );
        }
        FakeGitHubTransport::json(200, json!({ "installations": [] }))
    });
}

#[gpui_kit::test]
async fn shows_loading_and_ignores_refresh_while_the_account_refresh_is_in_flight(
    cx: &mut TestAppContext,
) {
    let (app, _workspace, _open, _recent, home) = dashboard(cx);
    script_successful_refresh(&app.github);
    // Al arrancar la app ya intentó refrescar la sesión una vez.
    let refreshes = |app: &TestApp| {
        app.github
            .requested_urls()
            .iter()
            .filter(|url| url.ends_with("/login/oauth/access_token"))
            .count()
    };
    let before = refreshes(&app);

    // El clic lanza el refresco; hasta que termine, el botón queda bloqueado.
    app.in_window(cx, |window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.click("github-refresh", cx);
        window.click("github-refresh", cx);
    });
    home.read_with(cx, |home, _| assert!(home.is_refreshing_github_account()));

    app.settle(cx);
    home.read_with(cx, |home, _| assert!(!home.is_refreshing_github_account()));
    // El segundo clic no lanzó otro refresco.
    assert_eq!(refreshes(&app) - before, 1);
    assert!(app.has_toast(NotifyType::Success, "GitHub account refreshed"));
}

#[gpui_kit::test]
async fn shows_reconnect_guidance_when_refresh_reports_an_invalid_session(cx: &mut TestAppContext) {
    let (app, _workspace, _open, _recent, _home) = dashboard(cx);
    app.github.respond_with(|_| {
        FakeGitHubTransport::json(
            200,
            json!({ "error": "bad_refresh_token", "error_description": "Session not found" }),
        )
    });

    app.click(cx, "github-refresh");

    assert!(app.has_toast(
        NotifyType::Error,
        "GitHub session expired. Reconnect the account to continue."
    ));
    // La cuenta quedó marcada y el inicio ofrece reconectar.
    assert_eq!(
        app.label(cx, "github-note").as_deref(),
        Some("Re-authentication required for this GitHub account.")
    );
    assert!(app.exists(cx, "github-reconnect"));
}

#[gpui_kit::test]
async fn reports_an_unreachable_github_when_refresh_fails_transiently(cx: &mut TestAppContext) {
    let (app, _workspace, _open, _recent, _home) = dashboard(cx);
    // Sin respuesta guionizada el transporte falla como una red caída.

    app.click(cx, "github-refresh");

    assert!(app.has_toast(NotifyType::Error, "GitHub is unreachable. Try again."));
}

#[gpui_kit::test]
async fn manage_and_reconnect_open_the_accounts_settings(cx: &mut TestAppContext) {
    let (app, _workspace, _open, _recent, _home) = dashboard(cx);

    app.click(cx, "github-manage");

    app.model.read_with(cx, |model, _| {
        assert!(model.show_preferences());
        assert_eq!(model.settings_section(), "accounts");
    });
    assert!(app.events().contains(&mergeon_app::AppEvent::OpenSettings {
        section: "accounts".to_string()
    }));
}

#[gpui_kit::test]
async fn collapses_and_expands_a_repository_group(cx: &mut TestAppContext) {
    let (app, _workspace, open, recent, home) = dashboard(cx);

    app.click(cx, "repo-group:work");
    home.read_with(cx, |home, _| assert!(!home.is_group_expanded("work")));
    assert!(!app.exists(cx, repo_row(&open)));
    assert!(!app.exists(cx, repo_row(&recent)));

    app.click(cx, "repo-group:work");
    home.read_with(cx, |home, _| assert!(home.is_group_expanded("work")));
    assert!(app.exists(cx, repo_row(&open)));
}

#[gpui_kit::test]
async fn clears_the_recent_repositories_after_confirming(cx: &mut TestAppContext) {
    let (app, _workspace, open, recent, home) = dashboard(cx);
    app.click(cx, repo_row(&recent));

    // Cancelar no toca nada.
    app.host.answer_confirm(false);
    app.click(cx, "clear-recent");
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.recent_repositories().len()),
        2
    );

    app.host.answer_confirm(true);
    app.click(cx, "clear-recent");

    let confirm = app.host.confirms().pop().expect("confirmación");
    assert_eq!(confirm.title.as_deref(), Some("Clear recent repositories"));
    assert_eq!(
        confirm.description.as_deref(),
        Some("This will remove every repository from the recent list.")
    );
    assert_eq!(confirm.confirm_text.as_deref(), Some("Clear all"));
    assert_eq!(confirm.variant.as_deref(), Some("destructive"));

    app.model.read_with(cx, |model, _| {
        assert!(model.recent_repositories().is_empty());
        // Solo se conservan los resúmenes de los repositorios abiertos.
        assert!(model.repository_summary(&open).is_some());
        assert!(model.repository_summary(&recent).is_none());
    });
    assert!(app.has_toast(NotifyType::Success, "Recent repositories cleared"));
    // El elegido ya no existe: se vuelve al repositorio abierto.
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(open.as_str()));
    });
    assert!(!app.exists(cx, repo_row(&recent)));
    assert!(!app.exists(cx, "clear-recent"));
}

#[gpui_kit::test]
async fn an_empty_home_invites_to_pick_a_repository(cx: &mut TestAppContext) {
    let app = TestApp::with_backend(TestBackend::new(), cx);
    app.start(cx);
    let home = app.show(cx, HomeView::new);

    home.read_with(cx, |home, _| assert_eq!(home.selected_repo_path(), None));
    assert_eq!(
        app.label(cx, "github-note").as_deref(),
        Some("Add an account to clone repositories from GitHub.")
    );
    assert!(!app.exists(cx, "clear-recent"));
    assert!(!app.exists(cx, "github-refresh"));
    assert!(app.exists(cx, "github-manage"));
    assert!(!app.exists(cx, "summary-content"));
}

#[gpui_kit::test]
async fn follows_the_repositories_as_they_open(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let first = workspace.repository("alpha/first", None);
    let second = workspace.repository("beta/second", None);
    let app = TestApp::new(cx);
    app.start(cx);
    let home = app.show(cx, HomeView::new);
    home.read_with(cx, |home, _| assert_eq!(home.selected_repo_path(), None));

    // Al abrirse un repositorio aparece su grupo, desplegado, y queda elegido.
    app.open_repository(&first, cx);
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(first.as_str()));
        assert!(home.is_group_expanded("alpha"));
    });
    assert!(app.exists(cx, repo_row(&first)));

    // El elegido se conserva mientras siga abierto, aunque el activo sea otro
    // (`ensureSelectedRepository` solo cambia cuando deja de ser candidato).
    app.open_repository(&second, cx);
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(first.as_str()));
        assert!(home.is_group_expanded("beta"));
    });
    assert!(app.exists(cx, repo_row(&second)));

    // Al cerrarse el elegido se pasa al activo y se lee su resumen.
    app.model
        .update(cx, |model, cx| model.remove_repository(first.as_str(), cx));
    app.settle(cx);
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(second.as_str()));
    });
    app.wait_for(cx, "the summary of the new selection", |cx| {
        app.model
            .read_with(cx, |model, _| model.repository_summary(&second).is_some())
    });
}

// ---------------------------------------------------------------------------
// Repintados
// ---------------------------------------------------------------------------

fn renders(home: &gpui_kit::Entity<HomeView>, cx: &mut TestAppContext) -> usize {
    home.read_with(cx, |home, _| home.render_count())
}

/// Avisos del modelo que no cambian nada del inicio: un toast, lo que se
/// prepara en el repositorio abierto.
fn unrelated_notifications(app: &TestApp, workspace_repo: &str, cx: &mut TestAppContext) {
    for index in 0..3 {
        app.model.update(cx, |model, cx| {
            model.notify_info(&format!("Something else happened {index}"), cx);
            cx.notify();
        });
        app.settle(cx);
    }
    std::fs::write(
        std::path::Path::new(workspace_repo).join("scratch.txt"),
        "scratch\n",
    )
    .expect("write a file");
    app.model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(
                mergeon_app::StatusSyncReason::FsWatchWorkingTree,
                cx,
            )
        })
        .detach();
    app.settle(cx);
}

/// El inicio se repintaba con cada aviso del modelo, también tapado por un
/// repositorio. Ahora solo lo hace lo que cambia algo de lo que enseña, y
/// tapado no se repinta: al volver sale al día.
#[gpui_kit::test]
async fn the_home_repaints_only_for_what_it_shows_and_not_while_hidden(cx: &mut TestAppContext) {
    let (app, _workspace, open, recent, home) = dashboard(cx);
    assert!(app.exists(cx, repo_row(&recent)));

    // Que termine lo que quede en vuelo (el resumen, el avatar...).
    let mut last = renders(&home, cx);
    let mut still = 0;
    while still < 10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.settle(cx);
        let now = renders(&home, cx);
        still = if now == last { still + 1 } else { 0 };
        last = now;
    }

    // A la vista y en reposo: nada ajeno lo repinta.
    let idle = renders(&home, cx);
    unrelated_notifications(&app, &open, cx);
    assert_eq!(renders(&home, cx), idle);

    // Lo que enseña sí: un reciente menos.
    app.model
        .update(cx, |model, cx| model.remove_recent_repository(&recent, cx))
        .detach();
    app.settle(cx);
    let removed = renders(&home, cx);
    assert!(removed > idle, "{idle} -> {removed}");
    assert!(!app.exists(cx, repo_row(&recent)));

    // Tapado (otra vista en la ventana): ni lo ajeno ni lo suyo lo repintan.
    app.put(cx, None);
    let hidden = renders(&home, cx);
    unrelated_notifications(&app, &open, cx);
    // Se abre el que se había quitado: vuelve a la lista (abierto).
    app.open_repository(&recent, cx);
    assert!(
        app.model.read_with(cx, |model, _| model
            .open_repositories()
            .iter()
            .any(|repo| repo.path == recent)),
        "the repository is open"
    );
    unrelated_notifications(&app, &open, cx);
    assert_eq!(renders(&home, cx), hidden);

    // De vuelta en el inicio, al día.
    app.put(cx, Some(home.clone().into()));
    assert!(app.exists(cx, repo_row(&recent)));
    assert!(renders(&home, cx) > hidden);

    // Y sigue repintándose con lo suyo: se cierra el repositorio.
    let shown = renders(&home, cx);
    let recent_id = app.model.read_with(cx, |model, _| {
        model
            .open_repositories()
            .iter()
            .find(|repo| repo.path == recent)
            .map(|repo| repo.id.clone())
            .expect("the repository is open")
    });
    app.model.update(cx, |model, cx| {
        model.remove_repository(recent_id.as_str(), cx)
    });
    app.settle(cx);
    assert!(renders(&home, cx) > shown);
}

#[gpui_kit::test]
async fn home_shows_workspaces_progressively_and_allows_creating_and_switching(
    cx: &mut TestAppContext,
) {
    let (app, _workspace, open, _recent, _home) = dashboard(cx);

    // Sin workspaces: el inicio permanece limpio, solo ofrece el botón discreto de crear.
    assert!(!app.exists(cx, "home-workspace-all"));
    assert!(app.exists(cx, "home-new-workspace"));

    app.host.answer_prompt("Core");
    app.click(cx, "home-new-workspace");
    app.wait_for(cx, "workspace created", |cx| {
        app.model
            .read_with(cx, |model, _| model.workspaces().len() == 1)
    });
    app.settle(cx);

    let ws_id = app
        .model
        .read_with(cx, |model, _| model.workspaces()[0].id.clone());
    assert!(app.exists(cx, "home-workspace-all"));
    assert!(app.exists(cx, format!("home-workspace:{ws_id}")));
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .active_workspace_id()
            .map(str::to_string)),
        Some(ws_id.clone())
    );

    // Cambiar a "All" desde Home desactiva el filtro de workspace.
    app.click(cx, "home-workspace-all");
    app.settle(cx);
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .active_workspace_id()
            .map(str::to_string)),
        None
    );

    // Añadir un repo al workspace y seleccionarlo desde Home activa el repo.
    app.model.update(cx, |model, cx| {
        model.add_repository_to_workspace(&open, &ws_id, cx);
    });
    app.settle(cx);
    app.click(cx, format!("home-workspace:{ws_id}"));
    app.settle(cx);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_workspace_id(), Some(ws_id.as_str()));
        assert_eq!(
            model.active_repository().map(|repo| repo.path.as_str()),
            Some(open.as_str())
        );
    });
}

#[gpui_kit::test]
async fn home_repository_context_menu_assigns_and_toggles_multiple_workspaces(
    cx: &mut TestAppContext,
) {
    let (app, _workspace, open, recent, _home) = dashboard(cx);

    // Sin workspaces: el menú ofrece crear uno con ese repositorio y activarlo lo crea.
    let items_before = cx.update(|cx| crate::home::home_repository_menu(&app.model, &recent, cx));
    let create_entry = items_before
        .iter()
        .find(|item| item.text().as_ref() == "New workspace with this repository…")
        .expect("create workspace item");
    app.host.answer_prompt("Work");
    assert!(
        cx.update_window(app.window, |_, window, cx| create_entry
            .activate(window, cx))
            .unwrap()
    );
    app.wait_for(cx, "workspace Work created from home repo menu", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .workspaces()
                .iter()
                .any(|ws| ws.name == "Work" && model.is_repository_in_workspace(&recent, &ws.id))
        })
    });

    // Con un segundo workspace: el menú expone el submenú "Workspaces" y permite asignar varios.
    let (work_id, personal_id) = app.model.update(cx, |model, cx| {
        let work_id = model.workspaces()[0].id.clone();
        let personal = model
            .create_workspace("Personal", Vec::new(), cx)
            .expect("personal");
        (work_id, personal.id)
    });
    app.settle(cx);

    let items_with_ws = cx.update(|cx| crate::home::home_repository_menu(&app.model, &open, cx));
    assert!(
        items_with_ws
            .iter()
            .any(|item| item.text().as_ref() == "Open repository")
    );
    assert!(
        items_with_ws
            .iter()
            .any(|item| item.text().as_ref() == "Workspaces")
    );

    // Asignar `open` tanto a Work como a Personal activando los ítems del submenú.
    let sub_items =
        cx.update(|cx| crate::home::home_repository_workspace_items(&app.model, &open, cx));
    assert_eq!(sub_items[0].text().as_ref(), "Work");
    assert_eq!(sub_items[1].text().as_ref(), "Personal");
    cx.update_window(app.window, |_, window, cx| {
        assert!(sub_items[0].activate(window, cx));
        assert!(sub_items[1].activate(window, cx));
    })
    .unwrap();
    app.settle(cx);
    app.model.read_with(cx, |model, _| {
        assert!(model.is_repository_in_workspace(&open, &work_id));
        assert!(model.is_repository_in_workspace(&open, &personal_id));
    });

    // Volver a activar Work lo desmarca sin quitarlo de Personal.
    let sub_items_after =
        cx.update(|cx| crate::home::home_repository_workspace_items(&app.model, &open, cx));
    cx.update_window(app.window, |_, window, cx| {
        assert!(sub_items_after[0].activate(window, cx));
    })
    .unwrap();
    app.settle(cx);
    app.model.read_with(cx, |model, _| {
        assert!(!model.is_repository_in_workspace(&open, &work_id));
        assert!(model.is_repository_in_workspace(&open, &personal_id));
    });
}
