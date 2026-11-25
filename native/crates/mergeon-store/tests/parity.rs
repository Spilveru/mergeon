// Mismo guion de operaciones que se ejecutó contra el `electron/store.js`
// original, con el `conf` real escribiendo en disco y `safeStorage` no
// disponible (`electron` sustituido por un doble mínimo). Lo que devolvió cada
// llamada y lo que quedó en el archivo está en
// `fixtures/electron-store-parity.json`; aquí se comprueba que el port devuelve
// y escribe lo mismo.

use std::fs;
use std::sync::Arc;

use mergeon_store::{
    CommitViewUpdate, DiffViewMode, GitHubAccount, GitHubInstallation, PlainTextCipher,
    RecentRepository, SettingsUpdate, SidebarSectionsUpdate, Store, StoreOptions, Theme,
    WorkspaceSession,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// `lastOpened` es la hora de la llamada: se compara su forma, no su valor.
fn mask_times(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if key == "lastOpened" {
                    let text = entry.as_str().expect("lastOpened is a string");
                    assert_eq!(text.len(), 24, "{text}");
                    assert!(text.ends_with('Z') && &text[10..11] == "T", "{text}");
                    *entry = json!("<time>");
                } else {
                    mask_times(entry);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_times),
        _ => {}
    }
}

fn masked(mut value: Value) -> Value {
    mask_times(&mut value);
    value
}

fn json_of<T: Serialize>(value: &T) -> Value {
    masked(serde_json::to_value(value).expect("json"))
}

fn repo(
    path: &str,
    name: &str,
    branch: &str,
    is_github: bool,
    remote_url: &str,
) -> RecentRepository {
    RecentRepository {
        id: path.into(),
        name: name.into(),
        path: path.into(),
        current_branch: branch.into(),
        is_github,
        remote_url: remote_url.into(),
        ..Default::default()
    }
}

fn account(id: &str, username: &str, token: &str) -> GitHubAccount {
    GitHubAccount {
        id: id.into(),
        username: username.into(),
        name: Some(username.to_uppercase()),
        email: Some(String::new()),
        avatar_url: Some("https://avatar".into()),
        token: token.into(),
        is_personal_access_token: Some(false),
        requires_installation: Some(false),
        added_at: Some("2026-07-26T00:00:00.000Z".into()),
        ..Default::default()
    }
}

#[test]
fn behaves_like_the_electron_store_for_the_same_operations() {
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/electron-store-parity.json")).expect("fixture");
    let mut expected = masked(expected);
    // Diferencia buscada con Electron: una instalación nueva arranca en el tema
    // claro (Señal), no en el oscuro. El resto del guion se compara tal cual.
    expected["fresh"]["settings"]["theme"] = json!("light");
    expected["settings0"]["theme"] = json!("light");

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    let store = Store::open(&path, StoreOptions::new(Arc::new(PlainTextCipher))).expect("open");
    let disk =
        || masked(serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json"));

    assert_eq!(disk(), expected["fresh"]);

    let r1 = store
        .add_recent_repository(&repo("/w/a", "a", "main", false, ""))
        .expect("r1");
    assert_eq!(json_of(&r1), expected["r1"]);
    store
        .add_recent_repository(&repo(
            "/w/b",
            "b",
            "HEAD",
            true,
            "https://github.com/o/b.git",
        ))
        .expect("add");
    store
        .add_recent_repository(&repo("/w/c", "c", "main", false, ""))
        .expect("add");
    let r2 = store
        .add_recent_repository(&repo("/w/a", "a", "dev", false, ""))
        .expect("r2");
    assert_eq!(json_of(&r2), expected["r2"]);
    store.remove_recent_repository("/w/c").expect("remove");

    let mut workspaces = Map::new();
    workspaces.insert("/w/a".into(), json!({ "layoutVersion": 2, "tabs": [] }));
    let session = store
        .set_workspace_session(&WorkspaceSession {
            repository_workspaces: workspaces,
            open_repositories: vec!["/w/a".into(), "".into(), "  ".into(), "/w/b".into()],
            active_repository_path: Some(" ".into()),
            ..Default::default()
        })
        .expect("session");
    assert_eq!(json_of(&session), expected["session"]);
    assert_eq!(
        json_of(&store.get_workspace_session()),
        expected["sessionRead"]
    );

    assert_eq!(json_of(&store.get_settings()), expected["settings0"]);
    store
        .update_settings(&SettingsUpdate {
            theme: Some(Theme::Light),
            ..Default::default()
        })
        .expect("settings");
    store
        .update_settings(&SettingsUpdate {
            commit_view: Some(CommitViewUpdate {
                file_list_width: Some(Some(280.0)),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("settings");
    store
        .update_settings(&SettingsUpdate {
            commit_view: Some(CommitViewUpdate {
                diff_view_mode: Some(DiffViewMode::Split),
                ignore_whitespace: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("settings");
    store
        .update_settings(&SettingsUpdate {
            sidebar_sections: Some(SidebarSectionsUpdate {
                tags: Some(true),
                starred: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("settings");
    store
        .update_settings(&SettingsUpdate {
            auto_fetch: Some(false),
            auto_fetch_interval_minutes: Some(25),
            ..Default::default()
        })
        .expect("settings");
    store
        .update_settings(&SettingsUpdate {
            sidebar_width: Some(Some(312.0)),
            ..Default::default()
        })
        .expect("settings");
    let settings1 = store
        .update_settings(&SettingsUpdate {
            commit_view: Some(CommitViewUpdate {
                history_width: Some(Some(450.0)),
                ..Default::default()
            }),
            max_recent_repos: Some(2),
            ..Default::default()
        })
        .expect("settings1");
    assert_eq!(json_of(&settings1), expected["settings1"]);
    let settings2 = store
        .update_settings(&SettingsUpdate {
            sidebar_width: Some(None),
            commit_view: Some(CommitViewUpdate {
                history_width: Some(None),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("settings2");
    assert_eq!(json_of(&settings2), expected["settings2"]);

    let r3 = store
        .add_recent_repository(&repo("/w/d", "d", "main", false, ""))
        .expect("r3");
    assert_eq!(json_of(&r3), expected["r3"]);

    assert_eq!(
        json_of(&store.add_starred_branch("/w/a", "main").expect("star")),
        expected["star1"]
    );
    store.add_starred_branch("/w/a", "dev").expect("star");
    assert_eq!(
        json_of(&store.add_starred_branch("/w/a", "main").expect("star")),
        expected["star2"]
    );
    assert_eq!(
        json_of(&store.remove_starred_branch("/w/a", "main").expect("unstar")),
        expected["star3"]
    );
    assert_eq!(
        json_of(
            &store
                .remove_starred_branch("/w/zzz", "main")
                .expect("unstar")
        ),
        expected["star4"]
    );

    let saved1 = store
        .save_github_account(&account("1", "octocat", "gho_one"))
        .expect("saved1");
    assert_eq!(json_of(&saved1), expected["saved1"]);
    store
        .set_active_github_account_id(Some("1"))
        .expect("active");

    let mut second = account("2", "hubot", "gho_two");
    second.refresh_token = "ghr_two".into();
    second.expires_at = Some("2026-07-26T08:00:00.000Z".into());
    second.installations = vec![GitHubInstallation {
        id: 42,
        account: Some("hubot".into()),
        avatar_url: None,
        repository_selection: Some("all".into()),
        extra: Map::new(),
    }];
    second.active_installation_id = Some(42);
    second.install_url = Some("https://github.com/apps/x/installations/new".into());
    let saved2 = store.save_github_account(&second).expect("saved2");
    assert_eq!(json_of(&saved2), expected["saved2"]);

    assert_eq!(
        json_of(&store.get_github_accounts().expect("accounts")),
        expected["accounts"]
    );
    assert_eq!(disk(), expected["beforeRemove"]);

    store.remove_github_account("1").expect("remove");
    assert_eq!(
        json_of(&store.get_active_github_account_id()),
        expected["active"]
    );
    store.clear_workspace_session().expect("clear");
    assert_eq!(
        json_of(&store.get_workspace_session()),
        expected["sessionCleared"]
    );

    assert_eq!(disk(), expected["final"]);
}
