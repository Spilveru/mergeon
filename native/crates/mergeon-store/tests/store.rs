// Comportamiento del store sobre un archivo real: las operaciones de
// `store.js`, la persistencia atómica, la tolerancia a archivos rotos, la
// importación desde el `config.json` de Electron y las claves desconocidas.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mergeon_store::{
    CommitViewUpdate, DiffViewMode, ExternalEditorSettings, GitHubAccount, LoadStatus,
    PlainTextCipher, RecentRepository, SecretCipher, SettingsUpdate, SidebarSectionsUpdate, Store,
    StoreError, StoreOptions, TestCipher, Theme, WorkspaceSession, default_config,
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("settings.json")
    }

    fn electron_config(&self) -> PathBuf {
        self.dir.path().join("config.json")
    }

    fn backup(&self) -> PathBuf {
        self.dir.path().join("settings.json.corrupt")
    }

    fn write(&self, contents: &str) {
        fs::write(self.path(), contents).expect("write settings");
    }

    fn write_json(&self, value: &Value) {
        self.write(&serde_json::to_string(value).expect("json"));
    }

    fn open(&self) -> Store {
        self.open_with(Arc::new(PlainTextCipher))
    }

    fn open_with(&self, cipher: Arc<dyn SecretCipher>) -> Store {
        Store::open(self.path(), StoreOptions::new(cipher)).expect("open store")
    }

    fn on_disk(&self) -> Value {
        read_json(&self.path())
    }

    fn file_names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir.path())
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read file")).expect("valid json")
}

fn account(id: &str, username: &str, token: &str) -> GitHubAccount {
    GitHubAccount {
        id: id.into(),
        username: username.into(),
        name: Some(username.to_uppercase()),
        token: token.into(),
        added_at: Some("2026-07-26T00:00:00.000Z".into()),
        ..Default::default()
    }
}

fn repo(path: &str) -> RecentRepository {
    RecentRepository {
        id: path.into(),
        name: path.rsplit('/').next().unwrap_or(path).into(),
        path: path.into(),
        current_branch: "main".into(),
        ..Default::default()
    }
}

fn is_iso_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 24
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
}

// ---- Persistencia ----

#[test]
fn a_missing_file_is_created_with_every_default() {
    let fixture = Fixture::new();

    let store = fixture.open();

    assert_eq!(store.load_report().status, LoadStatus::Created);
    assert_eq!(store.path(), Some(fixture.path().as_path()));
    assert_eq!(fixture.on_disk(), default_config());

    // Mismo formato que escribe `conf`: indentado con tabuladores.
    let text = fs::read_to_string(fixture.path()).expect("read");
    assert!(text.starts_with("{\n\t\""));
    assert!(text.contains("\n\t\"githubAccounts\": [],\n"));
    assert!(text.ends_with('}'));
}

#[test]
fn creates_the_parent_directories_it_needs() {
    let fixture = Fixture::new();
    let path = fixture.dir.path().join("a").join("b").join("settings.json");

    let store =
        Store::open(&path, StoreOptions::new(Arc::new(PlainTextCipher))).expect("open store");
    store.add_starred_branch("/repo", "main").expect("add");

    assert_eq!(
        read_json(&path)["starredBranches"]["/repo"],
        json!(["main"])
    );
}

#[test]
fn every_change_is_on_disk_when_the_call_returns_and_survives_a_reopen() {
    let fixture = Fixture::new();
    let store = fixture.open();

    store
        .add_recent_repository(&repo("/work/alpha"))
        .expect("add recent");
    assert_eq!(
        fixture.on_disk()["recentRepositories"][0]["path"],
        "/work/alpha"
    );

    store
        .add_starred_branch("/work/alpha", "main")
        .expect("star");
    assert_eq!(
        fixture.on_disk()["starredBranches"]["/work/alpha"],
        json!(["main"])
    );

    store
        .update_settings(&SettingsUpdate {
            theme: Some(Theme::Light),
            ..Default::default()
        })
        .expect("settings");
    assert_eq!(fixture.on_disk()["settings"]["theme"], "light");

    store
        .set_active_github_account_id(Some("7"))
        .expect("active");
    assert_eq!(fixture.on_disk()["activeGithubAccountId"], "7");
    drop(store);

    let reopened = fixture.open();
    assert_eq!(reopened.load_report().status, LoadStatus::Loaded);
    assert!(reopened.load_report().problems.is_empty());
    assert_eq!(reopened.get_recent_repositories()[0].path, "/work/alpha");
    assert_eq!(reopened.get_starred_branches("/work/alpha"), vec!["main"]);
    assert_eq!(reopened.get_settings().theme, Theme::Light);
    assert_eq!(
        reopened.get_active_github_account_id().as_deref(),
        Some("7")
    );
}

#[test]
fn writes_leave_no_temporary_files_behind() {
    let fixture = Fixture::new();
    let store = fixture.open();

    for index in 0..20 {
        store
            .add_starred_branch("/repo", &format!("branch-{index}"))
            .expect("star");
    }

    assert_eq!(fixture.file_names(), vec!["settings.json"]);
}

#[cfg(unix)]
#[test]
fn the_file_is_replaced_by_rename_and_only_its_owner_can_read_it() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let fixture = Fixture::new();
    let store = fixture.open();
    let before = fs::metadata(fixture.path()).expect("metadata");

    store.add_starred_branch("/repo", "main").expect("star");
    let after = fs::metadata(fixture.path()).expect("metadata");

    // Un archivo nuevo ocupa el lugar del anterior: nunca se trunca el que hay.
    assert_ne!(before.ino(), after.ino());
    assert_eq!(after.permissions().mode() & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn a_failed_write_changes_neither_the_file_nor_the_state() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let store = fixture.open();
    store.add_starred_branch("/repo", "main").expect("star");
    let before = fs::read(fixture.path()).expect("read");

    let directory = fixture.dir.path();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).expect("read-only dir");
    // Con privilegios de root el directorio sigue siendo escribible: no hay
    // forma de provocar el fallo y el caso no aplica.
    let can_still_write = fs::write(directory.join("probe"), b"").is_ok();
    if can_still_write {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("restore");
        return;
    }

    let star = store.add_starred_branch("/repo", "develop");
    let recent = store.add_recent_repository(&repo("/work/alpha"));
    let settings = store.update_settings(&SettingsUpdate {
        theme: Some(Theme::Dark),
        ..Default::default()
    });
    // Las lecturas que normalizan siguen respondiendo aunque no puedan guardar.
    let read_settings = store.get_settings();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("restore");

    assert!(matches!(star, Err(StoreError::Io { .. })));
    assert!(matches!(recent, Err(StoreError::Io { .. })));
    assert!(matches!(settings, Err(StoreError::Io { .. })));
    assert_eq!(read_settings.theme, Theme::Light);

    assert_eq!(fs::read(fixture.path()).expect("read"), before);
    assert_eq!(store.get_starred_branches("/repo"), vec!["main"]);
    assert!(store.get_recent_repositories().is_empty());
    assert_eq!(store.get_settings().theme, Theme::Light);
    assert_eq!(fixture.file_names(), vec!["settings.json"]);

    // Y en cuanto se puede escribir, vuelve a funcionar.
    assert_eq!(
        store.add_starred_branch("/repo", "develop").expect("star"),
        vec!["main", "develop"]
    );
}

#[test]
fn concurrent_changes_from_several_threads_are_all_kept() {
    let fixture = Fixture::new();
    let store = Arc::new(fixture.open());

    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                for index in 0..10 {
                    store
                        .add_starred_branch("/repo", &format!("w{worker}-b{index}"))
                        .expect("star");
                    store.get_settings();
                    store
                        .add_recent_repository(&repo(&format!("/work/w{worker}")))
                        .expect("recent");
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("worker");
    }

    let starred = store.get_starred_branches("/repo");
    assert_eq!(starred.len(), 80);
    assert_eq!(store.get_recent_repositories().len(), 8);
    assert_eq!(
        fixture.on_disk()["starredBranches"]["/repo"]
            .as_array()
            .expect("array")
            .len(),
        80
    );
    assert_eq!(fixture.file_names(), vec!["settings.json"]);
}

#[test]
fn an_in_memory_store_works_without_a_file() {
    let store = Store::in_memory(StoreOptions::new(Arc::new(PlainTextCipher)));

    assert_eq!(store.load_report().status, LoadStatus::InMemory);
    assert_eq!(store.path(), None);
    store.add_starred_branch("/repo", "main").expect("star");
    assert_eq!(store.get_starred_branches("/repo"), vec!["main"]);
    assert_eq!(store.get_settings().theme, Theme::Light);
}

// ---- Archivo ausente, vacío o corrupto ----

#[test]
fn an_empty_file_starts_from_defaults_without_a_backup() {
    for contents in ["", "  \n\t"] {
        let fixture = Fixture::new();
        fixture.write(contents);

        let store = fixture.open();

        assert_eq!(store.load_report().status, LoadStatus::Empty);
        assert_eq!(store.load_report().backup_path, None);
        assert_eq!(fixture.on_disk(), default_config());
        assert!(!fixture.backup().exists());
    }
}

#[test]
fn a_corrupt_file_starts_from_defaults_and_keeps_a_copy() {
    let fixture = Fixture::new();
    let broken = r#"{"recentRepositories": [{"path": "/work/alpha"}"#;
    fixture.write(broken);

    let store = fixture.open();

    assert_eq!(store.load_report().status, LoadStatus::Corrupt);
    assert_eq!(store.load_report().backup_path, Some(fixture.backup()));
    assert_eq!(store.load_report().problems.len(), 1);
    assert_eq!(
        fs::read_to_string(fixture.backup()).expect("backup"),
        broken
    );
    assert_eq!(fixture.on_disk(), default_config());
    assert!(store.get_recent_repositories().is_empty());

    // Y se puede seguir trabajando con normalidad.
    store.add_starred_branch("/repo", "main").expect("star");
    assert_eq!(
        fixture.on_disk()["starredBranches"]["/repo"],
        json!(["main"])
    );
}

#[test]
fn a_document_that_is_not_an_object_counts_as_corrupt() {
    for contents in ["[]", "null", "42", "\"text\""] {
        let fixture = Fixture::new();
        fixture.write(contents);

        let store = fixture.open();

        assert_eq!(store.load_report().status, LoadStatus::Corrupt);
        assert_eq!(
            fs::read_to_string(fixture.backup()).expect("backup"),
            contents
        );
        assert_eq!(fixture.on_disk(), default_config());
    }
}

#[test]
fn bytes_that_are_not_utf8_count_as_corrupt() {
    let fixture = Fixture::new();
    fs::write(fixture.path(), [0xff, 0xfe, 0x00, 0x7b]).expect("write");

    let store = fixture.open();

    assert_eq!(store.load_report().status, LoadStatus::Corrupt);
    assert_eq!(
        fs::read(fixture.backup()).expect("backup"),
        [0xff, 0xfe, 0x00, 0x7b]
    );
    assert_eq!(fixture.on_disk(), default_config());
}

#[test]
fn a_byte_order_mark_does_not_make_the_file_corrupt() {
    let fixture = Fixture::new();
    fixture.write("\u{feff}{\"starredBranches\": {\"/repo\": [\"main\"]}}");

    let store = fixture.open();

    assert_eq!(store.load_report().status, LoadStatus::Loaded);
    assert_eq!(store.get_starred_branches("/repo"), vec!["main"]);
}

#[test]
fn keys_that_violate_the_schema_go_back_to_their_default_and_the_rest_survives() {
    let fixture = Fixture::new();
    let original = json!({
        "settings": "dark",
        "starredBranches": { "/repo": ["main"] },
        "workspaceSession": { "openRepositories": [1, 2] },
        "activeGithubAccountId": 5
    });
    fixture.write_json(&original);

    let store = fixture.open();

    let report = store.load_report();
    assert_eq!(report.status, LoadStatus::Loaded);
    assert_eq!(
        report.problems,
        vec![
            "`activeGithubAccountId` must be string,null",
            "`workspaceSession/openRepositories/0` must be string",
            "`workspaceSession/openRepositories/1` must be string",
            "`settings` must be object",
        ]
    );
    assert_eq!(report.backup_path, Some(fixture.backup()));
    assert_eq!(read_json(&fixture.backup()), original);

    assert_eq!(store.get_starred_branches("/repo"), vec!["main"]);
    assert_eq!(store.get_settings(), Default::default());
    assert_eq!(store.get_workspace_session(), WorkspaceSession::default());
    assert_eq!(store.get_active_github_account_id(), None);

    let mut expected = default_config();
    expected["starredBranches"] = json!({ "/repo": ["main"] });
    assert_eq!(fixture.on_disk(), expected);
}

#[test]
fn only_the_invalid_items_of_a_list_are_dropped() {
    let fixture = Fixture::new();
    fixture.write_json(&json!({
        "recentRepositories": [
            { "path": "/work/alpha", "name": "alpha" },
            { "path": 7 },
            "garbage",
            { "path": "/work/beta", "name": "beta" }
        ],
        "githubAccounts": [
            { "id": "1", "username": "octocat", "tokenCipher": "plain:gho_one" },
            { "id": "2", "username": "hubot", "expiresAt": 123 }
        ]
    }));

    let store = fixture.open();

    assert_eq!(
        store.load_report().problems,
        vec![
            "`githubAccounts/1/expiresAt` must be string,null",
            "`recentRepositories/1/path` must be string",
            "`recentRepositories/2` must be object",
        ]
    );
    assert!(fixture.backup().exists());

    let recent: Vec<String> = store
        .get_recent_repositories()
        .into_iter()
        .map(|repo| repo.path)
        .collect();
    assert_eq!(recent, vec!["/work/alpha", "/work/beta"]);

    let accounts = store.get_github_accounts().expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].token, "gho_one");
}

#[test]
fn a_valid_file_is_not_rewritten_on_open() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.add_starred_branch("/repo", "main").expect("star");
    drop(store);
    // Mismo contenido con otro formato: si el store lo reescribiera, cambiaría.
    let compact = serde_json::to_string(&fixture.on_disk()).expect("json");
    fixture.write(&compact);

    let store = fixture.open();

    assert_eq!(store.load_report().status, LoadStatus::Loaded);
    assert_eq!(fs::read_to_string(fixture.path()).expect("read"), compact);
    assert_eq!(fixture.file_names(), vec!["settings.json"]);
}

#[test]
fn a_file_missing_some_keys_is_completed_with_their_defaults() {
    let fixture = Fixture::new();
    fixture
        .write_json(&json!({ "starredBranches": { "/repo": ["main"] }, "workspaceSession": {} }));

    let store = fixture.open();

    assert!(store.load_report().problems.is_empty());
    assert_eq!(store.load_report().backup_path, None);
    let on_disk = fixture.on_disk();
    assert_eq!(on_disk["githubAccounts"], json!([]));
    assert_eq!(on_disk["settings"], default_config()["settings"]);
    // Los defaults anidados del esquema no inventan `openRepositories`.
    assert_eq!(
        on_disk["workspaceSession"],
        json!({ "repositoryWorkspaces": {}, "activeRepositoryPath": null })
    );
    assert_eq!(on_disk["starredBranches"], json!({ "/repo": ["main"] }));
}

#[test]
fn a_file_that_cannot_be_read_is_an_error_and_is_left_alone() {
    let fixture = Fixture::new();
    // Un directorio donde debería estar el archivo: existe pero no se puede leer.
    fs::create_dir(fixture.path()).expect("dir");

    let error = Store::open(fixture.path(), StoreOptions::new(Arc::new(PlainTextCipher)))
        .expect_err("unreadable");

    assert!(matches!(error, StoreError::Io { .. }));
    assert!(fixture.path().is_dir());
}

// ---- Editor externo ----

#[test]
fn the_external_editor_is_saved_and_read_back_after_a_reopen() {
    let fixture = Fixture::new();
    let store = fixture.open();
    assert_eq!(
        store.get_settings().external_editor,
        ExternalEditorSettings::default()
    );

    let custom =
        ExternalEditorSettings::custom("/opt/My Editor/bin/edit", "--line {line} \"{path}\"");
    let saved = store
        .update_settings(&SettingsUpdate {
            external_editor: Some(custom.clone()),
            ..Default::default()
        })
        .expect("settings");
    assert_eq!(saved.external_editor, custom);
    // Cambiar otro ajuste no lo toca.
    store
        .update_settings(&SettingsUpdate {
            theme: Some(Theme::Dark),
            ..Default::default()
        })
        .expect("theme");
    drop(store);

    let on_disk = fixture.on_disk();
    assert_eq!(
        on_disk["settings"]["externalEditor"],
        json!({
            "editor": "custom",
            "customCommand": "/opt/My Editor/bin/edit",
            "customArgs": "--line {line} \"{path}\""
        })
    );
    assert_eq!(fixture.open().get_settings().external_editor, custom);
}

#[test]
fn an_external_editor_from_an_older_file_is_migrated_leniently() {
    let fixture = Fixture::new();
    fixture.write_json(&json!({ "settings": { "theme": "dark", "externalEditor": "zed" } }));

    let store = fixture.open();
    assert_eq!(
        store.get_settings().external_editor,
        ExternalEditorSettings::editor("zed")
    );
    // Al leerlo se reescribe ya con su forma completa.
    assert_eq!(
        fixture.on_disk()["settings"]["externalEditor"],
        json!({ "editor": "zed", "customCommand": "", "customArgs": "" })
    );
}

// ---- Claves desconocidas ----

#[test]
fn unknown_keys_survive_every_rewrite() {
    let fixture = Fixture::new();
    fixture.write_json(&json!({
        "windowBounds": { "width": 1400, "height": 900 },
        "__internal__": { "migrations": { "version": "0.9.0" } },
        "settings": {
            "theme": "light",
            "autoFetchIntervalMinutes": 15,
            "experimental": { "graph": true },
            "sidebarSections": { "starred": false, "worktrees": true },
            "commitView": { "diffViewMode": "split", "wrapLines": true }
        },
        "recentRepositories": [{ "path": "/work/alpha", "name": "alpha", "pinned": true }],
        "workspaceSession": {
            "openRepositories": ["/work/alpha"],
            "activeRepositoryPath": "/work/alpha",
            "repositoryWorkspaces": { "/work/alpha": { "layoutVersion": 2, "tabs": [] } },
            "windowState": "maximized"
        },
        "starredBranches": { "/work/alpha": ["main"] },
        "githubAccounts": [{
            "id": "7",
            "username": "octocat",
            "displayName": "The Octocat",
            "tokenCipher": "plain:gho_token",
            "refreshTokenCipher": "",
            "sessionTokenCipher": "",
            "scopes": ["repo"]
        }]
    }));

    let store = fixture.open();
    // Una escritura por cada zona del documento.
    store
        .add_starred_branch("/work/alpha", "develop")
        .expect("star");
    store
        .add_recent_repository(&repo("/work/beta"))
        .expect("recent");
    store
        .update_settings(&SettingsUpdate {
            auto_fetch: Some(false),
            sidebar_sections: Some(SidebarSectionsUpdate {
                tags: Some(true),
                ..Default::default()
            }),
            commit_view: Some(CommitViewUpdate {
                ignore_whitespace: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("settings");
    store
        .save_github_account(&account("8", "hubot", "gho_other"))
        .expect("account");
    drop(store);

    let reopened = fixture.open();
    let on_disk = fixture.on_disk();

    assert_eq!(
        on_disk["windowBounds"],
        json!({ "width": 1400, "height": 900 })
    );
    assert_eq!(on_disk["__internal__"]["migrations"]["version"], "0.9.0");

    assert_eq!(on_disk["settings"]["autoFetchIntervalMinutes"], 15);
    assert_eq!(
        on_disk["settings"]["experimental"],
        json!({ "graph": true })
    );
    assert_eq!(on_disk["settings"]["sidebarSections"]["worktrees"], true);
    assert_eq!(on_disk["settings"]["sidebarSections"]["starred"], false);
    assert_eq!(on_disk["settings"]["sidebarSections"]["tags"], true);
    assert_eq!(on_disk["settings"]["commitView"]["wrapLines"], true);
    assert_eq!(on_disk["settings"]["commitView"]["diffViewMode"], "split");
    assert_eq!(on_disk["settings"]["commitView"]["ignoreWhitespace"], true);

    assert_eq!(on_disk["recentRepositories"][0]["path"], "/work/beta");
    assert_eq!(on_disk["recentRepositories"][1]["pinned"], true);
    assert_eq!(on_disk["workspaceSession"]["windowState"], "maximized");
    assert_eq!(on_disk["githubAccounts"][0]["displayName"], "The Octocat");
    assert_eq!(on_disk["githubAccounts"][0]["scopes"], json!(["repo"]));

    // Y los tipos las exponen en `extra`.
    let settings = reopened.get_settings();
    assert_eq!(settings.effective_auto_fetch_interval_minutes(), 15);
    assert_eq!(settings.extra["experimental"], json!({ "graph": true }));
    assert_eq!(settings.sidebar_sections.extra["worktrees"], true);
    assert_eq!(settings.commit_view.extra["wrapLines"], true);
    assert_eq!(reopened.get_recent_repositories()[1].extra["pinned"], true);
    let accounts = reopened.get_github_accounts().expect("accounts");
    assert_eq!(accounts[0].extra["displayName"], "The Octocat");
}

// ---- Importación desde la app Electron ----

fn electron_config() -> Value {
    json!({
        "githubAccounts": [{
            "id": "7",
            "username": "octocat",
            "tokenCipher": "safe:djEwAAAA",
            "refreshTokenCipher": "",
            "sessionTokenCipher": ""
        }],
        "activeGithubAccountId": "7",
        "recentRepositories": [
            { "id": "/work/alpha", "name": "alpha", "path": "/work/alpha", "lastOpened": "2026-07-26T00:00:00.000Z", "currentBranch": "main", "isGitHub": true, "remoteUrl": "https://github.com/octocat/alpha.git" }
        ],
        "workspaceSession": {
            "repositoryWorkspaces": { "/work/alpha": { "layoutVersion": 2, "tabs": [], "activeTab": "commits", "section": "commits", "selectedCommit": null } },
            "openRepositories": ["/work/alpha"],
            "activeRepositoryPath": "/work/alpha"
        },
        "settings": {
            "theme": "light",
            "autoFetch": false,
            "autoFetchIntervalMinutes": 30,
            "maxRecentRepos": 10,
            "sidebarWidth": 300,
            "sidebarSections": { "starred": true, "branches": true, "remotes": true, "tags": false, "stashes": false, "submodules": false },
            "commitView": { "historyWidth": 420, "fileListWidth": null, "diffViewMode": "split", "ignoreWhitespace": true }
        },
        "starredBranches": { "/work/alpha": ["main", "release"] },
        "__internal__": { "migrations": { "version": "0.9.0" } }
    })
}

#[test]
fn imports_from_the_electron_config_the_first_time() {
    let fixture = Fixture::new();
    let config = serde_json::to_string_pretty(&electron_config()).expect("json");
    fs::write(fixture.electron_config(), &config).expect("config.json");

    let store = fixture.open();

    let report = store.load_report();
    assert_eq!(report.status, LoadStatus::Created);
    let import = report.electron_import.as_ref().expect("import");
    assert_eq!(import.source, fixture.electron_config());
    assert_eq!(
        import.keys,
        vec![
            "recentRepositories",
            "workspaceSession",
            "settings",
            "starredBranches"
        ]
    );

    let recent = store.get_recent_repositories();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].path, "/work/alpha");
    assert!(recent[0].is_github);
    assert_eq!(recent[0].last_opened, "2026-07-26T00:00:00.000Z");

    let session = store.get_workspace_session();
    assert_eq!(session.open_repositories, vec!["/work/alpha"]);
    assert_eq!(
        session.active_repository_path.as_deref(),
        Some("/work/alpha")
    );
    assert_eq!(
        session.repository_workspaces["/work/alpha"]["layoutVersion"],
        2
    );

    let settings = store.get_settings();
    assert_eq!(settings.theme, Theme::Light);
    assert!(!settings.auto_fetch);
    assert_eq!(settings.effective_auto_fetch_interval_minutes(), 30);
    assert_eq!(settings.sidebar_width, Some(300.0));
    assert!(settings.sidebar_sections.remotes);
    assert_eq!(settings.commit_view.history_width, Some(420.0));
    assert_eq!(settings.commit_view.diff_view_mode, DiffViewMode::Split);
    assert!(settings.commit_view.ignore_whitespace);

    assert_eq!(
        store.get_starred_branches("/work/alpha"),
        vec!["main", "release"]
    );

    // Las cuentas no se importan: sus tokens solo los descifra Electron.
    assert!(store.get_github_accounts().expect("accounts").is_empty());
    assert_eq!(store.get_active_github_account_id(), None);
    let on_disk = fixture.on_disk();
    assert_eq!(on_disk["githubAccounts"], json!([]));
    assert_eq!(on_disk["activeGithubAccountId"], json!(null));
    assert!(on_disk.get("__internal__").is_none());
    assert_eq!(
        on_disk["starredBranches"],
        electron_config()["starredBranches"]
    );

    // El archivo de Electron no se toca.
    assert_eq!(
        fs::read_to_string(fixture.electron_config()).expect("config"),
        config
    );
}

#[test]
fn does_not_import_again_once_its_own_file_exists() {
    let fixture = Fixture::new();
    fs::write(fixture.electron_config(), electron_config().to_string()).expect("config.json");

    let store = fixture.open();
    store.clear_recent_repositories().expect("clear");
    store
        .remove_starred_branch("/work/alpha", "release")
        .expect("unstar");
    drop(store);

    let reopened = fixture.open();

    assert_eq!(reopened.load_report().status, LoadStatus::Loaded);
    assert_eq!(reopened.load_report().electron_import, None);
    assert!(reopened.get_recent_repositories().is_empty());
    assert_eq!(reopened.get_starred_branches("/work/alpha"), vec!["main"]);
}

#[test]
fn an_existing_settings_file_wins_over_the_electron_config() {
    let fixture = Fixture::new();
    fs::write(fixture.electron_config(), electron_config().to_string()).expect("config.json");
    fixture.write_json(&json!({ "starredBranches": { "/native": ["trunk"] } }));

    let store = fixture.open();

    assert_eq!(store.load_report().electron_import, None);
    assert!(store.get_recent_repositories().is_empty());
    assert_eq!(store.get_starred_branches("/native"), vec!["trunk"]);
    assert!(store.get_starred_branches("/work/alpha").is_empty());
}

#[test]
fn the_import_can_be_turned_off_or_pointed_elsewhere() {
    let fixture = Fixture::new();
    fs::write(fixture.electron_config(), electron_config().to_string()).expect("config.json");

    let off = Store::open(
        fixture.path(),
        StoreOptions::new(Arc::new(PlainTextCipher)).without_electron_import(),
    )
    .expect("open");
    assert_eq!(off.load_report().electron_import, None);
    assert!(off.get_recent_repositories().is_empty());
    drop(off);

    let elsewhere = fixture.dir.path().join("native").join("settings.json");
    let pointed = Store::open(
        &elsewhere,
        StoreOptions::new(Arc::new(PlainTextCipher))
            .with_electron_config_path(fixture.electron_config()),
    )
    .expect("open");
    assert_eq!(pointed.get_recent_repositories().len(), 1);
    assert_eq!(
        pointed
            .load_report()
            .electron_import
            .as_ref()
            .map(|import| &import.source),
        Some(&fixture.electron_config())
    );
}

#[test]
fn a_broken_electron_config_is_ignored() {
    let fixture = Fixture::new();
    fs::write(fixture.electron_config(), "{ not json").expect("config.json");

    let store = fixture.open();

    assert_eq!(store.load_report().status, LoadStatus::Created);
    assert_eq!(store.load_report().electron_import, None);
    assert_eq!(fixture.on_disk(), default_config());
    assert_eq!(
        fs::read_to_string(fixture.electron_config()).expect("config"),
        "{ not json"
    );
    assert!(!fixture.backup().exists());
}

#[test]
fn only_the_valid_parts_of_the_electron_config_are_imported() {
    let fixture = Fixture::new();
    let config = json!({
        "recentRepositories": [{ "path": "/work/alpha" }, { "path": false }],
        "settings": ["not", "an", "object"],
        "starredBranches": { "/work/alpha": ["main"] }
    });
    fs::write(fixture.electron_config(), config.to_string()).expect("config.json");

    let store = fixture.open();

    let import = store
        .load_report()
        .electron_import
        .as_ref()
        .expect("import");
    assert_eq!(import.keys, vec!["recentRepositories", "starredBranches"]);
    assert_eq!(store.get_recent_repositories().len(), 1);
    assert_eq!(store.get_settings(), Default::default());
    assert_eq!(store.get_starred_branches("/work/alpha"), vec!["main"]);
}

#[test]
fn opening_in_the_user_data_dir_recovers_the_config_of_the_app_before_the_rename() {
    let root = tempfile::tempdir().expect("tempdir");
    let app_data = root.path().join(".config");
    let user_data = app_data.join("Mergeon");
    let legacy = app_data.join("git-gui");
    fs::create_dir_all(&legacy).expect("legacy dir");
    fs::write(legacy.join("config.json"), electron_config().to_string()).expect("legacy config");

    let store = Store::open_in(
        &user_data,
        Some(&app_data),
        StoreOptions::new(Arc::new(PlainTextCipher)),
    )
    .expect("open");

    assert_eq!(
        store.path(),
        Some(user_data.join("settings.json").as_path())
    );
    // La configuración anterior se copia donde la dejaría la app Electron...
    assert_eq!(read_json(&user_data.join("config.json")), electron_config());
    // ...y de ahí se importa, sin las cuentas.
    assert_eq!(store.get_recent_repositories()[0].path, "/work/alpha");
    assert_eq!(store.get_settings().theme, Theme::Light);
    assert!(store.get_github_accounts().expect("accounts").is_empty());
    drop(store);

    // La segunda vez ya no se mira nada de eso.
    fs::write(legacy.join("config.json"), "{}").expect("legacy config");
    fs::remove_file(user_data.join("config.json")).expect("remove config");
    let reopened = Store::open_in(
        &user_data,
        Some(&app_data),
        StoreOptions::new(Arc::new(PlainTextCipher)),
    )
    .expect("open");
    assert_eq!(reopened.load_report().status, LoadStatus::Loaded);
    assert!(!user_data.join("config.json").exists());
    assert_eq!(reopened.get_recent_repositories()[0].path, "/work/alpha");
}

// ---- Cuentas de GitHub ----

#[test]
fn without_encryption_tokens_are_stored_with_the_plain_scheme() {
    let fixture = Fixture::new();
    let store = fixture.open();

    let mut new_account = account("7", "octocat", "gho_token");
    new_account.refresh_token = "ghr_token".into();
    let saved = store.save_github_account(&new_account).expect("save");

    assert_eq!(saved.token, "gho_token");
    assert_eq!(saved.token_cipher, "plain:gho_token");
    assert_eq!(saved.refresh_token, "ghr_token");
    assert_eq!(saved.refresh_token_cipher, "plain:ghr_token");
    assert_eq!(saved.session_token, "");
    assert_eq!(saved.session_token_cipher, "");

    // En el archivo solo quedan los `*Cipher`.
    let stored = &fixture.on_disk()["githubAccounts"][0];
    assert_eq!(stored["tokenCipher"], "plain:gho_token");
    assert_eq!(stored["refreshTokenCipher"], "plain:ghr_token");
    assert_eq!(stored["sessionTokenCipher"], "");
    assert!(stored.get("token").is_none());
    assert!(stored.get("refreshToken").is_none());
    assert!(stored.get("sessionToken").is_none());
    assert_eq!(stored["username"], "octocat");
    assert_eq!(stored["name"], "OCTOCAT");

    assert_eq!(store.get_github_accounts().expect("accounts"), vec![saved]);
}

#[test]
fn with_encryption_tokens_are_stored_with_the_safe_scheme() {
    let fixture = Fixture::new();
    let cipher = Arc::new(TestCipher::new());
    let store = fixture.open_with(cipher.clone());

    store
        .save_github_account(&account("7", "octocat", "gho_token"))
        .expect("save");

    let stored = fixture.on_disk()["githubAccounts"][0]["tokenCipher"]
        .as_str()
        .expect("cipher")
        .to_string();
    assert!(stored.starts_with("safe:"));
    assert!(
        !fs::read_to_string(fixture.path())
            .expect("read")
            .contains("gho_token")
    );
    drop(store);

    let reopened = fixture.open_with(cipher);
    let accounts = reopened.get_github_accounts().expect("accounts");
    assert_eq!(accounts[0].token, "gho_token");
    assert_eq!(accounts[0].token_cipher, stored);
}

#[test]
fn an_encrypted_token_survives_while_encryption_is_unavailable() {
    let fixture = Fixture::new();
    let cipher = Arc::new(TestCipher::new());
    let store = fixture.open_with(cipher.clone());
    store
        .save_github_account(&account("7", "octocat", "gho_token"))
        .expect("save");
    let stored = fixture.on_disk()["githubAccounts"][0]["tokenCipher"].clone();

    // El llavero deja de estar disponible: el token no se puede leer...
    cipher.set_available(false);
    let mut unreadable = store.get_github_accounts().expect("accounts").remove(0);
    assert_eq!(unreadable.token, "");
    assert_eq!(Value::String(unreadable.token_cipher.clone()), stored);

    // ...pero guardar la cuenta sin modificar su token no pierde su cifrado,
    // mientras que intentar persistir un token nuevo en claro falla sin
    // autorización explícita.
    unreadable.requires_reconnect = true;
    store.save_github_account(&unreadable).expect("save");
    assert!(matches!(
        store.save_github_account(&account("8", "hubot", "gho_other")),
        Err(StoreError::KeyringUnavailable)
    ));
    let on_disk = fixture.on_disk();
    assert_eq!(on_disk["githubAccounts"][0]["tokenCipher"], stored);
    assert_eq!(on_disk["githubAccounts"][0]["requiresReconnect"], true);
    assert_eq!(on_disk["githubAccounts"].as_array().unwrap().len(), 1);

    // Con consentimiento explícito sí permite el fallback a `plain:`.
    cipher.set_allow_plaintext_fallback(true);
    store
        .save_github_account(&account("8", "hubot", "gho_other"))
        .expect("save with explicit plaintext fallback");
    let on_disk = fixture.on_disk();
    assert_eq!(
        on_disk["githubAccounts"][1]["tokenCipher"],
        "plain:gho_other"
    );

    // Cuando vuelve, el token se lee otra vez.
    cipher.set_available(true);
    let accounts = store.get_github_accounts().expect("accounts");
    assert_eq!(accounts[0].token, "gho_token");
    assert_eq!(accounts[1].token, "gho_other");
}

#[test]
fn legacy_plain_text_tokens_are_migrated_when_first_read() {
    let fixture = Fixture::new();
    fixture.write_json(&json!({
        "githubAccounts": [
            { "id": "1", "username": "octocat", "token": "gho_legacy", "refreshToken": "ghr_legacy" },
            { "id": "2", "username": "hubot", "sessionToken": "session_legacy" }
        ]
    }));
    let store = fixture.open();
    // Abrir no migra: los tokens siguen como estaban hasta la primera lectura.
    assert_eq!(
        fixture.on_disk()["githubAccounts"][0]["token"],
        "gho_legacy"
    );

    let accounts = store.get_github_accounts().expect("accounts");

    assert_eq!(accounts[0].token, "gho_legacy");
    assert_eq!(accounts[0].token_cipher, "plain:gho_legacy");
    assert_eq!(accounts[0].refresh_token, "ghr_legacy");
    assert_eq!(accounts[1].session_token, "session_legacy");
    assert_eq!(accounts[1].token, "");

    assert_eq!(
        fixture.on_disk()["githubAccounts"],
        json!([
            {
                "id": "1",
                "username": "octocat",
                "tokenCipher": "plain:gho_legacy",
                "refreshTokenCipher": "plain:ghr_legacy",
                "sessionTokenCipher": ""
            },
            {
                "id": "2",
                "username": "hubot",
                "tokenCipher": "",
                "refreshTokenCipher": "",
                "sessionTokenCipher": "plain:session_legacy"
            }
        ])
    );
}

#[test]
fn reading_accounts_that_need_no_migration_does_not_rewrite_the_file() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .save_github_account(&account("7", "octocat", "gho_token"))
        .expect("save");
    drop(store);
    let compact = serde_json::to_string(&fixture.on_disk()).expect("json");
    fixture.write(&compact);

    let store = fixture.open();
    let accounts = store.get_github_accounts().expect("accounts");

    assert_eq!(accounts[0].token, "gho_token");
    assert_eq!(fs::read_to_string(fixture.path()).expect("read"), compact);
}

#[test]
fn saving_replaces_the_account_with_the_same_id_and_appends_the_rest() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .save_github_account(&account("1", "octocat", "gho_one"))
        .expect("save");
    store
        .save_github_account(&account("2", "hubot", "gho_two"))
        .expect("save");

    let mut renamed = account("1", "octocat", "gho_one");
    renamed.name = Some("The Octocat".into());
    renamed.active_installation_id = Some(42);
    store.save_github_account(&renamed).expect("save");

    let accounts = store.get_github_accounts().expect("accounts");
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].id, "1");
    assert_eq!(accounts[0].name.as_deref(), Some("The Octocat"));
    assert_eq!(accounts[0].active_installation_id, Some(42));
    assert_eq!(accounts[1].id, "2");
    assert_eq!(accounts[1].token, "gho_two");
}

#[test]
fn a_refreshed_token_replaces_the_cipher_that_was_read_before() {
    // Quien refresca la sesión parte de la cuenta que leyó del store, que trae
    // el `*Cipher` del token anterior, y le pone encima los tokens nuevos.
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut original = account("7", "octocat", "gho_old");
    original.refresh_token = "ghr_old".into();
    store.save_github_account(&original).expect("save");

    let mut refreshed = store.get_github_accounts().expect("accounts").remove(0);
    assert_eq!(refreshed.token_cipher, "plain:gho_old");
    refreshed.token = "gho_new".into();
    refreshed.refresh_token = "ghr_new".into();
    let saved = store.save_github_account(&refreshed).expect("save");

    assert_eq!(saved.token, "gho_new");
    assert_eq!(saved.token_cipher, "plain:gho_new");
    let stored = &fixture.on_disk()["githubAccounts"][0];
    assert_eq!(stored["tokenCipher"], "plain:gho_new");
    assert_eq!(stored["refreshTokenCipher"], "plain:ghr_new");
    let accounts = store.get_github_accounts().expect("accounts");
    assert_eq!(accounts[0].token, "gho_new");
    assert_eq!(accounts[0].refresh_token, "ghr_new");
}

#[test]
fn removing_an_account_clears_the_active_id_only_if_it_was_the_active_one() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .save_github_account(&account("1", "octocat", "gho_one"))
        .expect("save");
    store
        .save_github_account(&account("2", "hubot", "gho_two"))
        .expect("save");
    store
        .save_github_account(&account("3", "monalisa", "gho_three"))
        .expect("save");
    assert_eq!(store.get_active_github_account_id(), None);
    store
        .set_active_github_account_id(Some("2"))
        .expect("active");

    store.remove_github_account("1").expect("remove");
    assert_eq!(store.get_active_github_account_id().as_deref(), Some("2"));

    store.remove_github_account("2").expect("remove");
    assert_eq!(store.get_active_github_account_id(), None);
    assert_eq!(fixture.on_disk()["activeGithubAccountId"], json!(null));

    let accounts = store.get_github_accounts().expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, "3");
    assert_eq!(accounts[0].token, "gho_three");

    // Quitar una que no existe no cambia nada.
    store.remove_github_account("missing").expect("remove");
    assert_eq!(store.get_github_accounts().expect("accounts").len(), 1);

    store
        .set_active_github_account_id(Some("3"))
        .expect("active");
    store.set_active_github_account_id(None).expect("active");
    assert_eq!(store.get_active_github_account_id(), None);
}

#[test]
fn a_cipher_that_fails_to_encrypt_fails_the_save_and_writes_nothing() {
    struct BrokenCipher;
    impl SecretCipher for BrokenCipher {
        fn is_available(&self) -> bool {
            true
        }
        fn encrypt(&self, _plain_text: &str) -> Option<String> {
            None
        }
        fn decrypt(&self, _payload: &str) -> Option<String> {
            None
        }
    }

    let fixture = Fixture::new();
    let store = fixture.open_with(Arc::new(BrokenCipher));
    let before = fs::read(fixture.path()).expect("read");

    let error = store
        .save_github_account(&account("7", "octocat", "gho_token"))
        .expect_err("encryption failure");

    assert!(matches!(error, StoreError::Encryption));
    assert_eq!(fs::read(fixture.path()).expect("read"), before);
    assert!(store.get_github_accounts().expect("accounts").is_empty());
}

// ---- Repositorios recientes ----

#[test]
fn adding_a_recent_repository_puts_it_first_with_the_time_it_was_opened() {
    let fixture = Fixture::new();
    let store = fixture.open();
    assert!(store.get_recent_repositories().is_empty());

    store
        .add_recent_repository(&repo("/work/alpha"))
        .expect("add");
    let mut beta = repo("/work/beta");
    beta.is_github = true;
    beta.remote_url = "https://github.com/octocat/beta.git".into();
    // Lo que traiga `lastOpened` se ignora: lo pone el store.
    beta.last_opened = "1999-01-01T00:00:00.000Z".into();
    let recent = store.add_recent_repository(&beta).expect("add");

    assert_eq!(recent, store.get_recent_repositories());
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].path, "/work/beta");
    assert_eq!(recent[0].name, "beta");
    assert!(recent[0].is_github);
    assert_eq!(recent[0].remote_url, "https://github.com/octocat/beta.git");
    assert!(
        is_iso_timestamp(&recent[0].last_opened),
        "{}",
        recent[0].last_opened
    );
    assert_ne!(recent[0].last_opened, "1999-01-01T00:00:00.000Z");
    assert_eq!(recent[1].path, "/work/alpha");

    assert_eq!(
        fixture.on_disk()["recentRepositories"][0],
        json!({
            "id": "/work/beta",
            "name": "beta",
            "path": "/work/beta",
            "lastOpened": recent[0].last_opened,
            "currentBranch": "main",
            "isGitHub": true,
            "remoteUrl": "https://github.com/octocat/beta.git"
        })
    );
}

#[test]
fn reopening_a_repository_moves_it_to_the_front_without_duplicating_it() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .add_recent_repository(&repo("/work/alpha"))
        .expect("add");
    store
        .add_recent_repository(&repo("/work/beta"))
        .expect("add");

    let mut alpha = repo("/work/alpha");
    alpha.current_branch = "develop".into();
    let recent = store.add_recent_repository(&alpha).expect("add");

    let paths: Vec<&str> = recent.iter().map(|repo| repo.path.as_str()).collect();
    assert_eq!(paths, vec!["/work/alpha", "/work/beta"]);
    assert_eq!(recent[0].current_branch, "develop");
}

#[test]
fn recent_repositories_are_capped_at_max_recent_repos() {
    let fixture = Fixture::new();
    let store = fixture.open();

    for index in 0..12 {
        store
            .add_recent_repository(&repo(&format!("/work/repo-{index}")))
            .expect("add");
    }
    let recent = store.get_recent_repositories();
    assert_eq!(recent.len(), 10);
    assert_eq!(recent[0].path, "/work/repo-11");
    assert_eq!(recent[9].path, "/work/repo-2");

    // El límite sale de los ajustes y se aplica en la siguiente apertura.
    store
        .update_settings(&SettingsUpdate {
            max_recent_repos: Some(3),
            ..Default::default()
        })
        .expect("settings");
    assert_eq!(store.get_recent_repositories().len(), 10);
    let recent = store
        .add_recent_repository(&repo("/work/last"))
        .expect("add");
    let paths: Vec<&str> = recent.iter().map(|repo| repo.path.as_str()).collect();
    assert_eq!(paths, vec!["/work/last", "/work/repo-11", "/work/repo-10"]);
}

#[test]
fn recent_repositories_can_be_removed_and_cleared() {
    let fixture = Fixture::new();
    let store = fixture.open();
    for path in ["/work/alpha", "/work/beta", "/work/gamma"] {
        store.add_recent_repository(&repo(path)).expect("add");
    }

    store
        .remove_recent_repository("/work/beta")
        .expect("remove");
    let paths: Vec<String> = store
        .get_recent_repositories()
        .into_iter()
        .map(|repo| repo.path)
        .collect();
    assert_eq!(paths, vec!["/work/gamma", "/work/alpha"]);

    store
        .remove_recent_repository("/work/unknown")
        .expect("remove");
    assert_eq!(store.get_recent_repositories().len(), 2);

    store.clear_recent_repositories().expect("clear");
    assert!(store.get_recent_repositories().is_empty());
    assert_eq!(fixture.on_disk()["recentRepositories"], json!([]));
}

// ---- Sesión de workspace ----

#[test]
fn the_workspace_session_starts_empty() {
    let fixture = Fixture::new();
    let store = fixture.open();

    let session = store.get_workspace_session();

    assert!(session.open_repositories.is_empty());
    assert_eq!(session.active_repository_path, None);
    assert!(session.repository_workspaces.is_empty());
}

#[test]
fn setting_the_workspace_session_drops_blank_paths() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut workspaces = Map::new();
    workspaces.insert(
        "/work/alpha".into(),
        json!({ "layoutVersion": 2, "tabs": [], "activeTab": "changes", "section": "changes", "selectedCommit": null }),
    );

    let saved = store
        .set_workspace_session(&WorkspaceSession {
            repository_workspaces: workspaces.clone(),
            open_repositories: vec![
                "/work/alpha".into(),
                "".into(),
                "  \t".into(),
                // Lo que `trim()` de JavaScript considera espacio.
                "\u{a0}\u{feff}\u{2003}".into(),
                "/work/beta".into(),
            ],
            active_repository_path: Some("   ".into()),
            ..Default::default()
        })
        .expect("set");

    assert_eq!(saved.open_repositories, vec!["/work/alpha", "/work/beta"]);
    assert_eq!(saved.active_repository_path, None);
    assert_eq!(saved.repository_workspaces, workspaces);
    assert_eq!(store.get_workspace_session(), saved);
    assert_eq!(
        fixture.on_disk()["workspaceSession"],
        json!({
            "repositoryWorkspaces": { "/work/alpha": workspaces["/work/alpha"] },
            "openRepositories": ["/work/alpha", "/work/beta"],
            "activeRepositoryPath": null
        })
    );

    let saved = store
        .set_workspace_session(&WorkspaceSession {
            repository_workspaces: Map::new(),
            open_repositories: vec!["/work/beta".into()],
            active_repository_path: Some("/work/beta".into()),
            ..Default::default()
        })
        .expect("set");
    assert_eq!(saved.active_repository_path.as_deref(), Some("/work/beta"));
    assert_eq!(
        fixture.on_disk()["workspaceSession"]["activeRepositoryPath"],
        "/work/beta"
    );
}

#[test]
fn workspace_groups_persist_and_validate_without_breaking_legacy_configs() {
    let fixture = Fixture::new();
    let store = fixture.open();

    let saved = store
        .set_workspace_session(&WorkspaceSession {
            open_repositories: vec!["/work/alpha".into(), "/work/beta".into()],
            active_repository_path: Some("/work/alpha".into()),
            workspaces: vec![
                mergeon_store::RepoWorkspace {
                    id: "ws-work".into(),
                    name: "  Work  ".into(),
                    repositories: vec![
                        "/work/alpha".into(),
                        " ".into(),
                        "/work/alpha".into(),
                        "/work/beta".into(),
                    ],
                    active_repository_path: Some("/work/alpha".into()),
                },
                mergeon_store::RepoWorkspace {
                    id: "  ".into(),
                    name: "Ignored".into(),
                    repositories: vec![],
                    active_repository_path: None,
                },
            ],
            active_workspace_id: Some("ws-work".into()),
            ..Default::default()
        })
        .expect("set with workspaces");

    assert_eq!(saved.workspaces.len(), 1);
    assert_eq!(saved.workspaces[0].name, "Work");
    assert_eq!(
        saved.workspaces[0].repositories,
        vec!["/work/alpha", "/work/beta"]
    );
    assert_eq!(saved.active_workspace_id.as_deref(), Some("ws-work"));
    drop(store);

    let reopened = fixture.open();
    assert!(reopened.load_report().problems.is_empty());
    let loaded = reopened.get_workspace_session();
    assert_eq!(loaded, saved);
}

#[test]
fn clearing_the_workspace_session_goes_back_to_the_schema_default() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .set_workspace_session(&WorkspaceSession {
            repository_workspaces: Map::new(),
            open_repositories: vec!["/work/alpha".into()],
            active_repository_path: Some("/work/alpha".into()),
            ..Default::default()
        })
        .expect("set");

    store.clear_workspace_session().expect("clear");

    assert_eq!(store.get_workspace_session(), WorkspaceSession::default());
    assert_eq!(
        fixture.on_disk()["workspaceSession"],
        json!({ "openRepositories": [], "activeRepositoryPath": null, "repositoryWorkspaces": {} })
    );
}

// ---- Ajustes ----

#[test]
fn settings_start_with_the_defaults() {
    let fixture = Fixture::new();
    let store = fixture.open();

    let settings = store.get_settings();

    assert_eq!(settings.theme, Theme::Light);
    assert!(settings.auto_fetch);
    assert_eq!(settings.effective_auto_fetch_interval_minutes(), 10);
    assert_eq!(settings.max_recent_repos, 10);
    assert_eq!(settings.sidebar_width, None);
    assert!(settings.sidebar_sections.starred);
    assert!(settings.sidebar_sections.branches);
    assert!(!settings.sidebar_sections.remotes);
    assert!(!settings.sidebar_sections.tags);
    assert!(!settings.sidebar_sections.stashes);
    assert!(!settings.sidebar_sections.submodules);
    assert_eq!(settings.commit_view.history_width, None);
    assert_eq!(settings.commit_view.file_list_width, None);
    assert_eq!(settings.commit_view.diff_view_mode, DiffViewMode::Unified);
    assert!(!settings.commit_view.ignore_whitespace);
    assert!(settings.extra.is_empty());
}

#[test]
fn reading_settings_normalizes_what_was_stored_and_saves_it() {
    let fixture = Fixture::new();
    // Ajustes de una versión anterior: faltan claves y hay valores fuera de rango.
    fixture.write_json(&json!({
        "settings": {
            "theme": "light",
            "sidebarWidth": -20,
            "commitView": { "historyWidth": "wide", "diffViewMode": "side-by-side", "ignoreWhitespace": "yes" }
        }
    }));
    let store = fixture.open();
    assert_eq!(fixture.on_disk()["settings"]["sidebarWidth"], -20);

    let settings = store.get_settings();

    assert_eq!(settings.theme, Theme::Light);
    assert!(settings.auto_fetch);
    assert_eq!(settings.sidebar_width, None);
    assert_eq!(settings.commit_view.history_width, None);
    assert_eq!(settings.commit_view.diff_view_mode, DiffViewMode::Unified);
    assert!(settings.commit_view.ignore_whitespace);
    assert_eq!(
        fixture.on_disk()["settings"],
        json!({
            "theme": "light",
            "autoFetch": true,
            "maxRecentRepos": 10,
            "sidebarWidth": null,
            "sidebarSections": {
                "starred": true,
                "branches": true,
                "remotes": false,
                "tags": false,
                "stashes": false,
                "submodules": false
            },
            "commitView": {
                "historyWidth": null,
                "fileListWidth": null,
                "diffViewMode": "unified",
                "ignoreWhitespace": true
            }
        })
    );
}

#[test]
fn updating_settings_only_touches_what_the_update_carries() {
    let fixture = Fixture::new();
    let store = fixture.open();

    // Cada composable del renderer manda su trozo: tema, auto-fetch, anchos, diff.
    store
        .update_settings(&SettingsUpdate {
            theme: Some(Theme::Light),
            ..Default::default()
        })
        .expect("theme");
    store
        .update_settings(&SettingsUpdate {
            auto_fetch: Some(false),
            auto_fetch_interval_minutes: Some(25),
            ..Default::default()
        })
        .expect("auto-fetch");
    store
        .update_settings(&SettingsUpdate {
            sidebar_width: Some(Some(312.0)),
            ..Default::default()
        })
        .expect("sidebar width");
    store
        .update_settings(&SettingsUpdate {
            commit_view: Some(CommitViewUpdate {
                file_list_width: Some(Some(280.0)),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("file list width");
    store
        .update_settings(&SettingsUpdate {
            commit_view: Some(CommitViewUpdate {
                history_width: Some(Some(450.0)),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("history width");
    store
        .update_settings(&SettingsUpdate {
            commit_view: Some(CommitViewUpdate {
                diff_view_mode: Some(DiffViewMode::Split),
                ignore_whitespace: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("diff preferences");
    let settings = store
        .update_settings(&SettingsUpdate {
            sidebar_sections: Some(SidebarSectionsUpdate {
                starred: Some(false),
                stashes: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("sidebar sections");

    assert_eq!(settings, store.get_settings());
    assert_eq!(settings.theme, Theme::Light);
    assert!(!settings.auto_fetch);
    assert_eq!(settings.auto_fetch_interval_minutes, Some(25.0));
    assert_eq!(settings.max_recent_repos, 10);
    assert_eq!(settings.sidebar_width, Some(312.0));
    assert_eq!(settings.commit_view.file_list_width, Some(280.0));
    assert_eq!(settings.commit_view.history_width, Some(450.0));
    assert_eq!(settings.commit_view.diff_view_mode, DiffViewMode::Split);
    assert!(settings.commit_view.ignore_whitespace);
    assert!(!settings.sidebar_sections.starred);
    assert!(settings.sidebar_sections.branches);
    assert!(settings.sidebar_sections.stashes);
    assert_eq!(
        fixture.on_disk()["settings"]["commitView"]["diffViewMode"],
        "split"
    );
    assert_eq!(
        fixture.on_disk()["settings"]["autoFetchIntervalMinutes"],
        25
    );

    // Volver al ancho de diseño.
    let settings = store
        .update_settings(&SettingsUpdate {
            sidebar_width: Some(None),
            commit_view: Some(CommitViewUpdate {
                history_width: Some(None),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("reset widths");
    assert_eq!(settings.sidebar_width, None);
    assert_eq!(settings.commit_view.history_width, None);
    assert_eq!(settings.commit_view.file_list_width, Some(280.0));

    // Un update vacío no cambia nada.
    assert_eq!(
        store
            .update_settings(&SettingsUpdate::default())
            .expect("noop"),
        settings
    );
}

#[test]
fn updating_settings_with_json_follows_the_renderer_contract() {
    let fixture = Fixture::new();
    let store = fixture.open();

    let settings = store
        .update_settings_json(&json!({
            "theme": "light",
            "futureFlag": true,
            "sidebarWidth": "wide",
            "sidebarSections": { "remotes": true },
            "commitView": { "diffViewMode": "sideways", "historyWidth": 500 }
        }))
        .expect("update");

    assert_eq!(settings.theme, Theme::Light);
    assert_eq!(settings.extra["futureFlag"], true);
    assert_eq!(settings.sidebar_width, None);
    assert!(settings.sidebar_sections.remotes);
    assert_eq!(settings.commit_view.diff_view_mode, DiffViewMode::Unified);
    assert_eq!(settings.commit_view.history_width, Some(500.0));

    // Unos cambios que no son un objeto dejan los ajustes como estaban.
    assert_eq!(
        store.update_settings_json(&json!(null)).expect("noop"),
        settings
    );
    assert_eq!(fixture.on_disk()["settings"]["futureFlag"], true);
}

// ---- Ramas destacadas ----

#[test]
fn starred_branches_are_kept_per_repository() {
    let fixture = Fixture::new();
    let store = fixture.open();
    assert!(store.get_starred_branches("/work/alpha").is_empty());

    assert_eq!(
        store
            .add_starred_branch("/work/alpha", "main")
            .expect("add"),
        vec!["main"]
    );
    assert_eq!(
        store
            .add_starred_branch("/work/alpha", "release/1.0")
            .expect("add"),
        vec!["main", "release/1.0"]
    );
    // Destacar dos veces la misma no la duplica.
    assert_eq!(
        store
            .add_starred_branch("/work/alpha", "main")
            .expect("add"),
        vec!["main", "release/1.0"]
    );
    store
        .add_starred_branch("/work/beta", "develop")
        .expect("add");

    assert_eq!(
        store.get_starred_branches("/work/alpha"),
        vec!["main", "release/1.0"]
    );
    assert_eq!(store.get_starred_branches("/work/beta"), vec!["develop"]);
    assert_eq!(
        fixture.on_disk()["starredBranches"],
        json!({ "/work/alpha": ["main", "release/1.0"], "/work/beta": ["develop"] })
    );
}

#[test]
fn removing_a_starred_branch_returns_what_is_left() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .add_starred_branch("/work/alpha", "main")
        .expect("add");
    store
        .add_starred_branch("/work/alpha", "develop")
        .expect("add");

    assert_eq!(
        store
            .remove_starred_branch("/work/alpha", "main")
            .expect("remove"),
        vec!["develop"]
    );
    assert_eq!(
        store
            .remove_starred_branch("/work/alpha", "unknown")
            .expect("remove"),
        vec!["develop"]
    );
    assert!(
        store
            .remove_starred_branch("/work/alpha", "develop")
            .expect("remove")
            .is_empty()
    );
    // Igual que el original: el repo queda con una lista vacía.
    assert_eq!(
        fixture.on_disk()["starredBranches"],
        json!({ "/work/alpha": [] })
    );

    // Y quitar de un repo sin destacadas también lo deja anotado.
    assert!(
        store
            .remove_starred_branch("/work/beta", "main")
            .expect("remove")
            .is_empty()
    );
    assert_eq!(
        fixture.on_disk()["starredBranches"],
        json!({ "/work/alpha": [], "/work/beta": [] })
    );
}

#[test]
fn a_repository_path_with_dots_is_a_single_key() {
    // El original indexa `starredBranches[repoPath]` directamente, sin la
    // notación con puntos de `electron-store`.
    let fixture = Fixture::new();
    let store = fixture.open();

    store
        .add_starred_branch("/work/my.project.v2", "main")
        .expect("add");

    assert_eq!(
        store.get_starred_branches("/work/my.project.v2"),
        vec!["main"]
    );
    assert_eq!(
        fixture.on_disk()["starredBranches"],
        json!({ "/work/my.project.v2": ["main"] })
    );
}
