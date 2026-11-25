//! `GitHubRepositoryPicker.test.js`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, ScrollDelta, SharedString, TestAppContext, point, px};
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::testing::FakeGitHubTransport;
use mergeon_app::{AppEvent, DialogRequest};
use mergeon_store::GitHubAccount;
use serde_json::{Value, json};

use super::support::{TestApp, Workspace, account, seeded_store};
use crate::{GitHubRepositoryPicker, open_github_repository_picker};

fn install_required() -> GitHubAccount {
    account(json!({
        "id": "account-1",
        "username": "Spilveru",
        "name": "Spilveru",
        "avatarUrl": "https://example.com/avatar.png",
        "token": "ghu_token",
        "refreshToken": "ghr_token",
        "activeInstallationId": null,
        "installations": [],
        "requiresInstallation": true,
        "installUrl": "https://github.com/apps/app-git-service/installations/new"
    }))
}

/// La misma cuenta ya instalada en `installations` (la primera es la activa).
fn installed(installations: Value) -> GitHubAccount {
    let active = installations[0]["id"].clone();
    account(json!({
        "id": "account-1",
        "username": "Spilveru",
        "name": "Spilveru",
        "avatarUrl": "https://example.com/avatar.png",
        "token": "ghu_token",
        "refreshToken": "ghr_token",
        "activeInstallationId": active,
        "installations": installations,
        "requiresInstallation": false,
        "installUrl": "https://github.com/apps/app-git-service/installations/new"
    }))
}

fn start(accounts: &[GitHubAccount], cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::with_store(seeded_store(&[], accounts), cx);
    // Las cuentas se cargan sin refrescar sus sesiones.
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .detach();
    app.settle(cx);
    app
}

fn open(app: &TestApp, cx: &mut TestAppContext) -> Entity<GitHubRepositoryPicker> {
    let picker = app.in_window(cx, open_github_repository_picker);
    app.settle(cx);
    picker
}

/// La instalación de una petición `/user/installations/<id>/repositories`.
fn installation_of(url: &str) -> Option<u64> {
    let rest = url.split("/user/installations/").nth(1)?;
    rest.split('/').next()?.parse().ok()
}

fn page_of(url: &str) -> u32 {
    url.split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("page="))
        .and_then(|page| page.parse().ok())
        .unwrap_or(1)
}

fn repositories(
    items: Value,
) -> Result<mergeon_github::HttpResponse, mergeon_github::TransportError> {
    let total = items.as_array().map_or(0, Vec::len);
    FakeGitHubTransport::json(200, json!({ "total_count": total, "repositories": items }))
}

fn repo_card(id: u64) -> SharedString {
    SharedString::from(format!("github-repo:{id}"))
}

fn names(picker: &Entity<GitHubRepositoryPicker>, cx: &mut TestAppContext) -> Vec<String> {
    picker.read_with(cx, |picker, _| {
        picker
            .all_repositories()
            .iter()
            .map(|item| item.repository.full_name.clone().unwrap_or_default())
            .collect()
    })
}

#[gpui_kit::test]
async fn shows_an_installation_required_state_and_avoids_fetching_repositories(
    cx: &mut TestAppContext,
) {
    let app = start(&[install_required()], cx);

    let picker = open(&app, cx);

    assert_eq!(app.dialog_titles(cx), ["Clone From GitHub"]);
    assert!(app.exists(cx, "github-picker-install-required"));
    assert!(app.exists(cx, "github-picker-install"));
    assert!(app.exists(cx, "github-picker-refresh"));
    assert!(app.github.requests().is_empty());
    picker.read_with(cx, |picker, _| {
        assert_eq!(picker.selected_account_id(), Some("account-1"));
        assert!(picker.all_repositories().is_empty());
    });

    app.click(cx, "github-picker-install");
    assert!(
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/apps/app-git-service/installations/new")
    );
}

#[gpui_kit::test]
async fn ignores_repository_loads_that_were_cancelled_by_closing_the_dialog(
    cx: &mut TestAppContext,
) {
    let app = start(
        &[installed(json!([{ "id": 1, "accountLogin": "Spilveru" }]))],
        cx,
    );
    let stale = Arc::new(AtomicBool::new(true));
    app.github.respond_with({
        let stale = stale.clone();
        move |_| {
            if stale.load(Ordering::SeqCst) {
                repositories(json!([{ "id": 1, "name": "repo", "full_name": "old/repo", "description": "stale" }]))
            } else {
                repositories(json!([]))
            }
        }
    });

    // La carga sigue en vuelo cuando el diálogo se cierra.
    let first = app.in_window(cx, |window, cx| {
        let picker = open_github_repository_picker(window, cx);
        window.render_frame(cx);
        window.click("github-picker-cancel", cx);
        picker
    });
    app.settle(cx);
    assert!(app.dialog_titles(cx).is_empty());
    assert!(names(&first, cx).is_empty());

    // Al reabrir empieza una carga nueva.
    stale.store(false, Ordering::SeqCst);
    let second = open(&app, cx);

    assert_ne!(first, second);
    assert!(names(&second, cx).is_empty());
    assert!(!app.exists(cx, repo_card(1)));
    assert!(app.exists(cx, "github-picker-empty"));
}

#[gpui_kit::test]
async fn aggregates_repositories_from_every_installation_newest_first_iterating_the_active_one_last(
    cx: &mut TestAppContext,
) {
    let app = start(
        &[installed(json!([
            { "id": 1, "accountLogin": "Spilveru" },
            { "id": 2, "accountLogin": "other-owner" }
        ]))],
        cx,
    );
    app.github.respond_with(|request| {
        if installation_of(&request.url) == Some(1) {
            repositories(json!([{
                "id": 1,
                "name": "own-repo",
                "full_name": "Spilveru/own-repo",
                "private": false,
                "updated_at": "2026-01-01T00:00:00Z",
                "owner": { "login": "Spilveru", "avatar_url": "https://example.com/spilveru.png" }
            }]))
        } else {
            repositories(json!([{
                "id": 2,
                "name": "shared-repo",
                "full_name": "other-owner/shared-repo",
                "private": true,
                "updated_at": "2026-06-01T00:00:00Z",
                "owner": { "login": "other-owner", "avatar_url": "https://example.com/other.png" }
            }]))
        }
    });

    let picker = open(&app, cx);

    assert_eq!(
        names(&picker, cx),
        ["other-owner/shared-repo", "Spilveru/own-repo"]
    );
    assert!(app.exists(cx, repo_card(1)));
    assert!(app.exists(cx, repo_card(2)));
    // El más reciente va arriba.
    let own = app.find(cx, repo_card(1)).expect("own repo");
    let shared = app.find(cx, repo_card(2)).expect("shared repo");
    assert!(shared.bounds().top() < own.bounds().top());

    // Cada repositorio recuerda la instalación por la que llegó.
    picker.read_with(cx, |picker, _| {
        let installations: Vec<u64> = picker
            .all_repositories()
            .iter()
            .map(|item| item.installation_id)
            .collect();
        assert_eq!(installations, [2, 1]);
    });

    let requested: Vec<u64> = app
        .github
        .requested_urls()
        .iter()
        .filter_map(|url| installation_of(url))
        .collect();
    assert_eq!(requested, [2, 1]);
}

#[gpui_kit::test]
async fn virtualizes_the_repository_list_so_a_large_account_only_builds_the_cards_in_view(
    cx: &mut TestAppContext,
) {
    let app = start(
        &[installed(json!([{ "id": 1, "accountLogin": "Spilveru" }]))],
        cx,
    );
    let many: Vec<Value> = (1..=120)
        .map(|index| {
            json!({
                "id": index,
                "name": format!("repo-{index}"),
                "full_name": format!("Spilveru/repo-{index}"),
                "private": false,
                // Del más reciente al más antiguo, para un orden estable.
                "updated_at": format!("2026-01-01T00:{:02}:{:02}Z", (120 - index) / 60, (120 - index) % 60),
                "owner": { "login": "Spilveru", "avatar_url": "https://example.com/spilveru.png" }
            })
        })
        .collect();
    app.github.respond_with(move |request| {
        if page_of(&request.url) == 1 {
            repositories(Value::Array(many.clone()))
        } else {
            repositories(json!([]))
        }
    });

    let picker = open(&app, cx);

    let rendered = |app: &TestApp, cx: &mut TestAppContext| {
        (1..=120u64)
            .filter(|id| app.exists(cx, repo_card(*id)))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&picker, cx).len(), 120);
    // Arriba, las primeras; solo las que caben en la lista (320px).
    let visible = rendered(&app, cx);
    assert!(visible.contains(&1), "{visible:?}");
    assert!(visible.len() < 20, "{visible:?}");
    // Ya no hay tandas: todo se alcanza desplazándose.
    assert!(!app.exists(cx, "github-picker-show-more"));

    // Al final de la lista, las últimas.
    app.in_window(cx, |window, cx| {
        window.scroll(
            "github-picker-results",
            ScrollDelta::Pixels(point(px(0.), px(-100_000.))),
            cx,
        );
    });
    app.settle(cx);
    let visible = rendered(&app, cx);
    assert!(visible.contains(&120), "{visible:?}");
    assert!(!visible.contains(&1), "{visible:?}");
    assert!(visible.len() < 20, "{visible:?}");

    // Una tarjeta del final se elige con un clic.
    app.click(cx, repo_card(120));
    picker.read_with(cx, |picker, _| {
        assert_eq!(
            picker
                .selected_repository()
                .and_then(|item| item.repository.id),
            Some(120)
        );
    });

    // Buscar filtra sobre el conjunto completo y vuelve arriba.
    app.in_window(cx, |window, cx| {
        picker.update(cx, |picker, cx| {
            picker
                .search
                .update(cx, |state, cx| state.focus(window, cx));
        });
        window.input("repo-12", cx);
    });
    app.settle(cx);
    picker.read_with(cx, |picker, cx| {
        assert_eq!(picker.search.read(cx).value(), "repo-12");
    });
    // repo-12 y repo-120.
    assert_eq!(rendered(&app, cx), [12, 120]);
}

/// Abierto y en reposo, los avisos del modelo que no le tocan (un toast, un
/// repositorio que se abre) no repintan el selector.
#[gpui_kit::test]
async fn unrelated_model_notifications_do_not_repaint_the_open_picker(cx: &mut TestAppContext) {
    let app = start(
        &[installed(json!([{ "id": 1, "accountLogin": "Spilveru" }]))],
        cx,
    );
    app.github.respond_with(|request| {
        if page_of(&request.url) == 1 {
            repositories(json!([{
                "id": 1,
                "name": "own-repo",
                "full_name": "Spilveru/own-repo",
                "private": false,
                "updated_at": "2026-01-01T00:00:00Z",
                "owner": { "login": "Spilveru", "avatar_url": "https://example.com/spilveru.png" }
            }]))
        } else {
            repositories(json!([]))
        }
    });
    let picker = open(&app, cx);
    assert!(app.exists(cx, repo_card(1)));
    let renders = |cx: &mut TestAppContext| picker.read_with(cx, |picker, _| picker.render_count());
    // Que termine lo que quede en vuelo (avatares...).
    let mut last = renders(cx);
    let mut still = 0;
    while still < 10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.settle(cx);
        let now = renders(cx);
        still = if now == last { still + 1 } else { 0 };
        last = now;
    }

    let before = renders(cx);
    for index in 0..3 {
        app.model.update(cx, |model, cx| {
            model.notify_info(&format!("Something else happened {index}"), cx);
            cx.notify();
        });
        app.settle(cx);
    }
    let workspace = Workspace::new();
    let repository = workspace.repository("work/other", None);
    app.open_repository(&repository, cx);
    assert_eq!(renders(cx), before);

    // Lo suyo sí: una cuenta menos.
    app.model
        .update(cx, |model, cx| model.remove_account("account-1", cx))
        .detach();
    app.settle(cx);
    assert!(renders(cx) > before);
}

#[gpui_kit::test]
async fn keeps_repositories_from_healthy_installations_when_another_installation_fails(
    cx: &mut TestAppContext,
) {
    let app = start(
        &[installed(json!([
            { "id": 1, "accountLogin": "Spilveru" },
            { "id": 2, "accountLogin": "other-owner" }
        ]))],
        cx,
    );
    app.github.respond_with(|request| {
        if installation_of(&request.url) == Some(2) {
            return FakeGitHubTransport::json(403, json!({ "message": "installation suspended" }));
        }
        repositories(json!([{
            "id": 1,
            "name": "own-repo",
            "full_name": "Spilveru/own-repo",
            "private": false,
            "owner": { "login": "Spilveru" }
        }]))
    });

    let picker = open(&app, cx);

    assert_eq!(names(&picker, cx), ["Spilveru/own-repo"]);
    assert!(app.exists(cx, repo_card(1)));
    assert!(app.has_toast(
        NotifyType::Warning,
        "Some repositories could not be loaded (other-owner)"
    ));
}

#[gpui_kit::test]
async fn reports_a_failed_load_when_every_installation_fails(cx: &mut TestAppContext) {
    let app = start(
        &[installed(json!([{ "id": 1, "accountLogin": "Spilveru" }]))],
        cx,
    );
    app.github
        .respond_with(|_| FakeGitHubTransport::json(500, json!({ "message": "boom" })));

    let picker = open(&app, cx);

    assert!(names(&picker, cx).is_empty());
    assert!(
        app.host
            .messages(NotifyType::Error)
            .iter()
            .any(|message| message.starts_with("Failed to load repositories:"))
    );
    assert!(app.exists(cx, "github-picker-empty"));
}

#[gpui_kit::test]
async fn clones_collaborator_repositories_and_opens_the_clone(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let remote = workspace.bare_remote("shared-repo");
    let destination = workspace.directory("workspace");
    let destination_dir = workspace.base.join("workspace");
    let app = start(
        &[installed(json!([
            { "id": 1, "accountLogin": "Spilveru" },
            { "id": 2, "accountLogin": "other-owner" }
        ]))],
        cx,
    );
    app.github.respond_with(move |request| {
        if installation_of(&request.url) == Some(2) {
            repositories(json!([{
                "id": 2,
                "name": "shared-repo",
                "full_name": "other-owner/shared-repo",
                "clone_url": remote,
                "private": true,
                "owner": { "login": "other-owner" }
            }]))
        } else {
            repositories(json!([]))
        }
    });
    app.host.answer_directory(Some(&destination_dir));

    let picker = open(&app, cx);
    app.click(cx, repo_card(2));
    app.click(cx, "github-picker-browse");
    assert_eq!(app.host.directory_requests(), 1);
    let clone_input = destination_dir
        .join("shared-repo")
        .to_string_lossy()
        .into_owned();
    let clone_path = format!("{destination}/shared-repo");
    picker.read_with(cx, |picker, cx| {
        assert_eq!(picker.clone_path(), clone_input);
        assert_eq!(picker.destination.read(cx).value(), clone_input.as_str());
        // El repositorio se clona con la instalación por la que se listó.
        assert_eq!(
            picker
                .selected_repository()
                .map(|item| item.installation_id),
            Some(2)
        );
    });

    app.click(cx, "github-picker-clone");

    app.wait_for(cx, "the clone to open", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .open_repositories()
                .iter()
                .any(|repo| repo.path == clone_path)
        })
    });
    assert!(destination_dir.join("shared-repo").join(".git").exists());
    picker.read_with(cx, |picker, _| assert!(!picker.is_cloning()));
    // Clonado y abierto, el selector se cierra.
    assert!(app.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn a_failed_clone_keeps_the_picker_open(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let destination = workspace.directory("workspace");
    let missing_remote = workspace.path("remotes/missing.git");
    let app = start(
        &[installed(json!([{ "id": 1, "accountLogin": "Spilveru" }]))],
        cx,
    );
    app.github.respond_with(move |_| {
        repositories(json!([{
            "id": 2,
            "name": "new-repo",
            "full_name": "new/repo",
            "clone_url": missing_remote,
            "private": false
        }]))
    });
    app.host.answer_directory(Some(Path::new(&destination)));

    let picker = open(&app, cx);
    app.click(cx, repo_card(2));
    app.click(cx, "github-picker-browse");
    app.click(cx, "github-picker-clone");
    app.wait_for(cx, "the clone to fail", |cx| {
        picker.read_with(cx, |picker, _| !picker.is_cloning())
            && !app.host.messages(NotifyType::Error).is_empty()
    });

    assert_eq!(app.dialog_titles(cx), ["Clone From GitHub"]);
    assert!(
        app.model
            .read_with(cx, |model, _| model.open_repositories().is_empty())
    );
}

#[gpui_kit::test]
async fn refreshes_the_selected_account_and_loads_its_repositories(cx: &mut TestAppContext) {
    let app = start(&[install_required()], cx);
    app.github.respond_with(|request| {
        let url = request.url.as_str();
        if url.ends_with("/login/oauth/access_token") {
            return FakeGitHubTransport::json(
                200,
                json!({ "access_token": "ghu_new", "refresh_token": "ghr_new", "expires_in": 28800 }),
            );
        }
        if url.ends_with("/user") {
            return FakeGitHubTransport::json(
                200,
                json!({ "id": "account-1", "login": "Spilveru", "name": "Spilveru" }),
            );
        }
        let listed = json!([{
            "id": 2,
            "name": "new-repo",
            "full_name": "new/repo",
            "clone_url": "https://github.com/new/repo.git",
            "private": false
        }]);
        if url.contains("/user/repos") {
            return FakeGitHubTransport::json(200, listed);
        }
        if url.contains("/repositories") {
            return repositories(listed);
        }
        FakeGitHubTransport::json(
            200,
            json!({ "installations": [{ "id": 1, "account": { "login": "Spilveru" } }] }),
        )
    });

    let picker = open(&app, cx);
    assert!(app.exists(cx, "github-picker-refresh"));

    // Mientras dura el refresco, el botón no vuelve a lanzarlo.
    app.in_window(cx, |window, cx| {
        window.click("github-picker-refresh", cx);
        window.click("github-picker-refresh", cx);
    });
    picker.read_with(cx, |picker, _| assert!(picker.is_refreshing_account()));

    app.settle(cx);

    picker.read_with(cx, |picker, _| assert!(!picker.is_refreshing_account()));
    let refreshes = app
        .github
        .requested_urls()
        .iter()
        .filter(|url| url.ends_with("/login/oauth/access_token"))
        .count();
    assert_eq!(refreshes, 1);
    // Ya instalada, la cuenta lista sus repositorios.
    assert_eq!(names(&picker, cx), ["new/repo"]);
    assert!(!app.exists(cx, "github-picker-install-required"));
    assert!(app.exists(cx, repo_card(2)));
}

#[gpui_kit::test]
async fn lets_users_choose_a_destination_first_and_updates_the_folder_when_selecting_another_repository(
    cx: &mut TestAppContext,
) {
    let workspace = Workspace::new();
    let first_remote = workspace.bare_remote("first");
    let second_remote = workspace.bare_remote("second");
    let destination = workspace.directory("Projects");
    let destination_dir = workspace.base.join("Projects");
    let app = start(
        &[installed(json!([{ "id": 1, "accountLogin": "Spilveru" }]))],
        cx,
    );
    app.github.respond_with(move |_| {
        repositories(json!([
            { "id": 1, "name": "first", "full_name": "Spilveru/first", "clone_url": first_remote, "updated_at": "2026-02-01T00:00:00Z" },
            { "id": 2, "name": "second", "full_name": "Spilveru/second", "clone_url": second_remote, "updated_at": "2026-01-01T00:00:00Z" }
        ]))
    });
    app.host.answer_directory(Some(&destination_dir));

    let picker = open(&app, cx);
    let shown = |cx: &mut TestAppContext| {
        picker.read_with(cx, |picker, cx| {
            picker.destination.read(cx).value().to_string()
        })
    };
    assert_eq!(shown(cx), "");

    app.click(cx, "github-picker-browse");
    assert_eq!(app.host.directory_requests(), 1);
    assert_eq!(shown(cx), destination_dir.to_string_lossy().as_ref());

    // Sin repositorio elegido no se puede clonar.
    app.click(cx, "github-picker-clone");
    picker.read_with(cx, |picker, _| assert!(!picker.is_cloning()));

    app.click(cx, repo_card(1));
    assert_eq!(
        shown(cx),
        destination_dir.join("first").to_string_lossy().as_ref()
    );
    app.click(cx, repo_card(2));
    assert_eq!(
        shown(cx),
        destination_dir.join("second").to_string_lossy().as_ref()
    );

    app.click(cx, "github-picker-clone");
    let clone_path = format!("{destination}/second");
    app.wait_for(cx, "the second repository to be cloned", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .open_repositories()
                .iter()
                .any(|repo| repo.path == clone_path)
        })
    });
    assert!(!destination_dir.join("first").exists());
}

#[gpui_kit::test]
async fn changing_the_account_reloads_and_forgets_the_selection(cx: &mut TestAppContext) {
    let app = start(
        &[
            account(json!({
                "id": "account-1",
                "username": "Spilveru",
                "name": "Spilveru",
                "token": "token-one",
                "installations": [],
                "requiresInstallation": false
            })),
            account(json!({
                "id": "account-2",
                "username": "octocat",
                "name": "Octocat",
                "token": "token-two",
                "installations": [],
                "requiresInstallation": false
            })),
        ],
        cx,
    );
    // Sin instalaciones (token personal) se lista por afiliación.
    app.github.respond_with(|request| {
        assert!(request.url.contains("/user/repos"));
        if request.header("authorization") == Some("Bearer token-one") {
            FakeGitHubTransport::json(
                200,
                json!([{ "id": 1, "name": "one", "full_name": "Spilveru/one" }]),
            )
        } else {
            FakeGitHubTransport::json(
                200,
                json!([{ "id": 2, "name": "two", "full_name": "octocat/two" }]),
            )
        }
    });

    let picker = open(&app, cx);
    assert_eq!(names(&picker, cx), ["Spilveru/one"]);
    app.click(cx, repo_card(1));
    picker.read_with(cx, |picker, _| {
        assert!(picker.selected_repository().is_some())
    });

    app.in_window(cx, |window, cx| {
        picker.update(cx, |picker, cx| {
            picker.handle_account_change("account-2", window, cx);
        });
    });
    app.settle(cx);

    assert_eq!(names(&picker, cx), ["octocat/two"]);
    picker.read_with(cx, |picker, _| {
        assert_eq!(picker.selected_account_id(), Some("account-2"));
        assert!(picker.selected_repository().is_none());
    });
}

#[gpui_kit::test]
async fn add_account_opens_the_account_dialog_on_top(cx: &mut TestAppContext) {
    let app = start(&[install_required()], cx);
    open(&app, cx);

    app.click(cx, "github-picker-add-account");

    assert_eq!(
        app.dialog_titles(cx),
        ["Clone From GitHub", "Connect GitHub Account"]
    );
    // Pedir de nuevo el selector no abre un segundo.
    app.in_window(cx, |window, cx| {
        assert!(crate::handle_dialog_request(
            &DialogRequest::GitHubRepositoryPicker,
            window,
            cx
        ));
    });
    assert_eq!(
        app.dialog_titles(cx),
        ["Clone From GitHub", "Connect GitHub Account"]
    );
    assert!(
        !app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubAccount))
    );
}

#[test]
fn search_matches_names_and_descriptions() {
    use mergeon_app::github::InstallationRepository;
    let repository = |name: &str, description: &str| InstallationRepository {
        repository: serde_json::from_value(json!({
            "fullName": name,
            "description": description,
            "owner": {}
        }))
        .expect("repositorio"),
        installation_id: 0,
    };
    let all = vec![
        repository("acme/api-server", "HTTP API"),
        repository("ana/dotfiles", "Shell for Acme laptops"),
        repository("ana/website", "Landing page"),
    ];

    let found = |query: &str| -> Vec<String> {
        crate::filter_repositories(&all, query)
            .into_iter()
            .map(|item| item.repository.full_name.clone().unwrap_or_default())
            .collect()
    };
    assert_eq!(found("").len(), 3);
    assert_eq!(found("  ACME "), ["acme/api-server", "ana/dotfiles"]);
    assert_eq!(found("landing"), ["ana/website"]);
    assert!(found("nothing").is_empty());
}
