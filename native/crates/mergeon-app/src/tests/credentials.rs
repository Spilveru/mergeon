//! Usuario y contraseña para remotos HTTPS que no son de GitHub, contra un
//! servidor HTTP real con autenticación básica (`git http-backend`).

#![cfg(unix)]

use gpui_kit::TestAppContext;
use mergeon_gitcmd::credentials::credential_origin;
use mergeon_gitcmd::test_support::AuthHttpServer;
use mergeon_model::notify::NotifyType;
use mergeon_model::operation_types::OperationResult;
use serde_json::json;

use super::support::{TestApp, TestRepo, git_in};
use crate::testing::TestBackend;
use crate::{CredentialsAnswer, FetchRequest, PushRequest};

const SECRET: &str = "s3cret-token-4f9";

fn answer(username: &str, password: &str) -> Option<CredentialsAnswer> {
    Some(CredentialsAnswer {
        username: username.to_string(),
        password: password.to_string(),
        remember: false,
    })
}

/// Repositorio con `origin` en un servidor HTTP que pide `alice` / [`SECRET`].
struct HttpRemote {
    repo: TestRepo,
    server: AuthHttpServer,
    bare: std::path::PathBuf,
    origin: String,
}

fn http_remote() -> HttpRemote {
    let repo = TestRepo::new();
    let served = repo.root().join("served");
    let bare = served.join("demo.git");
    std::fs::create_dir_all(&bare).unwrap();
    git_in(&bare, &["init", "--bare", "-b", "main"]);
    let server = AuthHttpServer::start(&served, "alice", SECRET).unwrap();
    let url = server.repo_url("demo.git");
    repo.git(&["remote", "add", "origin", &url]);
    let origin = credential_origin(&url).unwrap();
    HttpRemote {
        repo,
        server,
        bare,
        origin,
    }
}

fn push_main() -> PushRequest {
    PushRequest {
        branch: "main".to_string(),
        destination: "origin/main".to_string(),
        push_tags: false,
        force: false,
    }
}

/// Todo lo que la app enseña o guarda en texto: avisos y log de operaciones.
fn visible_text(app: &TestApp, cx: &mut TestAppContext) -> String {
    let toasts = app
        .host
        .toasts()
        .iter()
        .map(|toast| {
            format!(
                "{} {:?} {}",
                toast.title, toast.description, toast.full_message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let log = app.model.read_with(cx, |model, _| {
        serde_json::to_string(model.operations_log()).unwrap()
    });
    format!("{toasts}\n{log}")
}

#[gpui_kit::test]
async fn push_asks_for_credentials_retries_and_reuses_them_for_fetch(cx: &mut TestAppContext) {
    let remote = http_remote();
    let app = TestApp::new(cx);
    app.open(&remote.repo, cx).await;
    app.host.answer_credentials(answer("alice", SECRET));

    app.model
        .update(cx, |model, cx| model.handle_push_submit(push_main(), cx))
        .await;
    cx.run_until_parked();

    // Una sola pregunta, con el host del remoto, y el push llegó.
    let requests = app.host.credential_requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].origin, remote.origin);
    assert_eq!(
        requests[0].host,
        remote.origin.trim_start_matches("http://")
    );
    assert_eq!(requests[0].operation, "push");
    assert!(!requests[0].refused);
    assert_eq!(
        git_in(&remote.bare, &["rev-parse", "main"]).trim(),
        remote.repo.head()
    );
    assert!(remote.server.authorized_requests() > 0);
    // El primer intento no deja un aviso de fallo: lo sustituye el diálogo.
    assert!(app.host.messages(NotifyType::Error).is_empty());
    assert!(app.host.messages(NotifyType::Warning).is_empty());
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.http_credential_origins(),
            std::slice::from_ref(&remote.origin)
        );
    });

    // Un fetch después ya no pregunta: usa las de la sesión.
    app.model
        .update(cx, |model, cx| {
            model.handle_fetch_submit(
                FetchRequest {
                    remote: "origin".to_string(),
                    all_remotes: false,
                },
                cx,
            )
        })
        .await;
    cx.run_until_parked();
    assert_eq!(app.host.credential_requests().len(), 1);
    assert!(app.host.messages(NotifyType::Warning).is_empty());

    // Nada del secreto llega a avisos ni al log de operaciones.
    let text = visible_text(&app, cx);
    assert!(!text.contains(SECRET), "{text}");

    // Al cerrar la pestaña se olvidan.
    let repo_id = remote.repo.id();
    app.model.update(cx, |model, cx| {
        model.remove_repository(repo_id.as_str(), cx)
    });
    cx.run_until_parked();
    app.wait_for(cx, "the credentials to be released", |model| {
        model.http_credential_origins().is_empty()
    });
    let origin = remote.origin.clone();
    app.wait_for(cx, "the backend to forget them", move |model| {
        !model.backend.git().has_http_credentials(&origin)
    });
}

#[gpui_kit::test]
async fn refused_credentials_are_asked_again(cx: &mut TestAppContext) {
    let remote = http_remote();
    let app = TestApp::new(cx);
    app.open(&remote.repo, cx).await;
    app.host
        .answer_credentials(answer("alice", "wrong-password"));
    app.host.answer_credentials(answer("alice", SECRET));

    app.model
        .update(cx, |model, cx| model.handle_push_submit(push_main(), cx))
        .await;
    cx.run_until_parked();

    let requests = app.host.credential_requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(!requests[0].refused);
    assert!(requests[1].refused);
    // Propone el usuario tecleado antes.
    assert_eq!(requests[1].username.as_deref(), Some("alice"));
    assert_eq!(
        git_in(&remote.bare, &["rev-parse", "main"]).trim(),
        remote.repo.head()
    );
    let text = visible_text(&app, cx);
    assert!(
        !text.contains("wrong-password") && !text.contains(SECRET),
        "{text}"
    );
}

#[gpui_kit::test]
async fn cancelling_shows_the_original_failure_once(cx: &mut TestAppContext) {
    let remote = http_remote();
    let app = TestApp::new(cx);
    app.open(&remote.repo, cx).await;
    app.host.answer_credentials(None);

    app.model
        .update(cx, |model, cx| model.handle_push_submit(push_main(), cx))
        .await;
    cx.run_until_parked();

    assert_eq!(app.host.credential_requests().len(), 1);
    // Cancelar avisa con un mensaje claro (sin el stderr crudo de git), una sola vez.
    let warnings = app.host.messages(NotifyType::Warning);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("Sign-in to") && warnings[0].contains("cancelled"),
        "{warnings:?}"
    );
    assert!(
        !warnings[0].to_lowercase().contains("terminal prompts"),
        "{warnings:?}"
    );
    assert!(
        git_in(&remote.bare, &["branch", "--list"])
            .trim()
            .is_empty()
    );
    app.model.read_with(cx, |model, _| {
        assert!(model.http_credential_origins().is_empty());
    });
}

#[gpui_kit::test]
async fn clone_asks_for_credentials_and_keeps_them_for_the_new_tab(cx: &mut TestAppContext) {
    let remote = http_remote();
    // Algo que clonar.
    remote
        .repo
        .git(&["push", &remote.bare.to_string_lossy(), "main:main"]);
    let app = TestApp::new(cx);
    app.host.answer_credentials(answer("alice", SECRET));
    let target = remote.repo.root().join("cloned");
    let url = remote.server.repo_url("demo.git");

    let result = app
        .model
        .update(cx, |model, cx| {
            model.clone_repository(&url, &target.to_string_lossy(), None, cx)
        })
        .await;

    assert!(result.success, "{}", result.message);
    assert!(target.join("README.md").exists());
    let requests = app.host.credential_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].operation, "clone");
    assert!(app.host.messages(NotifyType::Error).is_empty());
    assert!(app.host.messages(NotifyType::Warning).is_empty());
    let config = std::fs::read_to_string(target.join(".git/config")).unwrap();
    assert!(!config.contains(SECRET), "{config}");
}

#[gpui_kit::test]
async fn only_http_authentication_failures_ask_for_credentials(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let failed = |message: &str| OperationResult {
        success: false,
        message: Some(message.to_string()),
        ..Default::default()
    };
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model
                .credentials_prompt_origin(&failed(
                    "could not read Username for 'https://gitlab.com': terminal prompts disabled"
                ))
                .as_deref(),
            Some("https://gitlab.com")
        );
        // Sin cuenta de GitHub, github.com también se pregunta.
        assert_eq!(
            model
                .credentials_prompt_origin(&failed(
                    "Authentication failed for 'https://github.com/acme/web.git/'"
                ))
                .as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            model.credentials_prompt_origin(&failed(
                "git@gitlab.com: Permission denied (publickey)."
            )),
            None
        );
        assert_eq!(
            model.credentials_prompt_origin(&failed("! [rejected] main -> main (fetch first)")),
            None
        );
        let ok = OperationResult {
            success: true,
            ..Default::default()
        };
        assert_eq!(model.credentials_prompt_origin(&ok), None);
    });
}

#[gpui_kit::test]
async fn github_with_a_connected_account_keeps_using_its_token(cx: &mut TestAppContext) {
    let store = TestBackend::memory_store();
    let account = serde_json::from_value(json!({
        "id": "7",
        "username": "ana",
        "token": "ghp_secret",
        "isPersonalAccessToken": true
    }))
    .unwrap();
    store.save_github_account(&account).unwrap();
    store.set_active_github_account_id(Some("7")).unwrap();
    let app = TestApp::with_backend(TestBackend::new().with_store(store), cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;

    let failed = |message: &str| OperationResult {
        success: false,
        message: Some(message.to_string()),
        ..Default::default()
    };
    app.model.read_with(cx, |model, _| {
        assert!(model.git_auth_token().is_some());
        assert_eq!(
            model.credentials_prompt_origin(&failed(
                "Authentication failed for 'https://github.com/acme/web.git/'"
            )),
            None
        );
        // Los demás hosts sí.
        assert_eq!(
            model
                .credentials_prompt_origin(&failed(
                    "Authentication failed for 'https://gitlab.com/acme/web.git/'"
                ))
                .as_deref(),
            Some("https://gitlab.com")
        );
    });
}

/// Con un credential helper configurado se ofrece "Remember": marcado, las
/// credenciales acaban en el helper (`git credential approve`); sin marcar,
/// git no las guarda por su cuenta aunque la operación funcione.
#[gpui_kit::test]
async fn remember_stores_in_the_user_helper_only_when_asked(cx: &mut TestAppContext) {
    let remote = http_remote();
    let store = remote.repo.root().join("git-credentials");
    remote.repo.git(&[
        "config",
        "credential.helper",
        &format!("store --file={}", store.display()),
    ]);
    let app = TestApp::new(cx);
    app.open(&remote.repo, cx).await;

    // Sin "Remember": funciona, pero el helper no recibe nada.
    app.host.answer_credentials(answer("alice", SECRET));
    app.model
        .update(cx, |model, cx| model.handle_push_submit(push_main(), cx))
        .await;
    cx.run_until_parked();
    let requests = app.host.credential_requests();
    assert!(requests[0].can_remember, "{requests:?}");
    assert_eq!(
        git_in(&remote.bare, &["rev-parse", "main"]).trim(),
        remote.repo.head()
    );
    assert!(
        std::fs::read_to_string(&store)
            .unwrap_or_default()
            .is_empty(),
        "git stored the credentials without being asked"
    );

    // Se cierran (se olvidan) y se vuelve a pedir, ahora con "Remember".
    let repo_id = remote.repo.id();
    app.model.update(cx, |model, cx| {
        model.remove_repository(repo_id.as_str(), cx)
    });
    let origin = remote.origin.clone();
    app.wait_for(cx, "the credentials to be forgotten", move |model| {
        !model.backend.git().has_http_credentials(&origin)
    });
    app.open(&remote.repo, cx).await;
    remote.repo.commit_file("b.txt", "b\n", "B");
    app.host.answer_credentials(Some(CredentialsAnswer {
        username: "alice".to_string(),
        password: SECRET.to_string(),
        remember: true,
    }));
    app.model
        .update(cx, |model, cx| model.handle_push_submit(push_main(), cx))
        .await;
    cx.run_until_parked();
    assert_eq!(app.host.credential_requests().len(), 2);
    let stored = std::fs::read_to_string(&store).unwrap();
    assert!(
        stored.contains("alice") && stored.contains(SECRET),
        "{stored}"
    );
}
