//! Remotos y tags creados o borrados fuera de la app: el watcher avisa con
//! `fs-watch-config` / `fs-watch-refs` y la app relee remotos, tags y PRs.

use std::time::Duration;

use gpui_kit::TestAppContext;
use mergeon_backend::{BackendEvent, StatusChange, WatchReason};

use super::support::{TestApp, TestRepo};
use crate::MetadataRefresh;

fn notify(app: &TestApp, repo: &TestRepo, reason: WatchReason, cx: &mut TestAppContext) {
    let repo_path = repo.path_string();
    app.model.update(cx, |model, cx| {
        model.handle_backend_event(
            BackendEvent::StatusChanged(StatusChange { repo_path, reason }),
            cx,
        );
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
}

fn remote_names(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    app.model.read_with(cx, |model, _| {
        model
            .active_repository()
            .unwrap()
            .remotes
            .iter()
            .map(|remote| remote.name.clone())
            .collect()
    })
}

#[gpui_kit::test]
async fn a_remote_added_in_the_terminal_shows_up_and_rechecks_pull_requests(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    app.open(&repo, cx).await;
    app.clear_events();
    assert_eq!(remote_names(&app, cx), ["origin"]);
    let refreshes = app
        .model
        .read_with(cx, |model, _| model.pull_request_refresh_count());

    repo.git(&[
        "remote",
        "add",
        "upstream",
        "https://github.com/acme/web.git",
    ]);
    notify(&app, &repo, WatchReason::FsWatchConfig, cx);
    app.wait_for(cx, "the new remote", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.remotes.iter().any(|r| r.name == "upstream"))
    });
    // Apareció un remoto (de GitHub): la sección de PRs vuelve a mirar.
    app.wait_for(cx, "a pull request refresh", |model| {
        model.pull_request_refresh_count() > refreshes
    });
    // Solo se releyeron remotos: no cuenta como mutación del historial.
    assert!(
        app.recorded_mutations().is_empty(),
        "{:?}",
        app.recorded_mutations()
    );

    // Un cambio de config que no toca remotos no vuelve a preguntar a GitHub.
    let refreshes = app
        .model
        .read_with(cx, |model, _| model.pull_request_refresh_count());
    repo.git(&["config", "core.ignorecase", "false"]);
    notify(&app, &repo, WatchReason::FsWatchConfig, cx);
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.pull_request_refresh_count()),
        refreshes
    );

    repo.git(&["remote", "remove", "upstream"]);
    notify(&app, &repo, WatchReason::PollConfig, cx);
    app.wait_for(cx, "the removed remote to go", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.remotes.iter().all(|r| r.name != "upstream"))
    });
}

#[gpui_kit::test]
async fn a_tag_created_in_the_terminal_shows_up_and_refreshes_the_history(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    app.clear_events();

    repo.git(&["tag", "v9.9"]);
    notify(&app, &repo, WatchReason::FsWatchRefs, cx);
    app.wait_for(cx, "the new tag", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.tags.iter().any(|tag| tag == "v9.9"))
    });
    // El historial vuelve a leerse para pintar la etiqueta.
    assert_eq!(app.recorded_mutations(), ["external-refs-change"]);

    repo.git(&["tag", "-d", "v9.9"]);
    notify(&app, &repo, WatchReason::FsWatchRefs, cx);
    app.wait_for(cx, "the deleted tag to go", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.tags.is_empty())
    });
}

#[gpui_kit::test]
async fn a_stash_made_in_the_terminal_shows_up_and_dropping_it_removes_it(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    app.clear_events();
    let stash_count = |model: &crate::AppModel| {
        model
            .active_repository()
            .map(|repository| repository.stashes.len())
            .unwrap_or_default()
    };
    assert_eq!(app.model.read_with(cx, |model, _| stash_count(model)), 0);

    // Un cambio que la app ya vio…
    repo.write("README.md", "# Changed\n");
    notify(&app, &repo, WatchReason::FsWatchWorkingTree, cx);
    app.wait_for(cx, "the change", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.changes_count == 1)
    });

    // …se guarda en un stash desde la terminal: llegan a la vez el aviso de
    // working tree y el de stash, y ninguno deja al otro con datos viejos.
    repo.git(&["stash", "push", "-m", "from the terminal"]);
    app.model.update(cx, |model, cx| {
        for reason in [WatchReason::FsWatchWorkingTree, WatchReason::FsWatchStash] {
            model.handle_backend_event(
                BackendEvent::StatusChanged(StatusChange {
                    repo_path: repo.path_string(),
                    reason,
                }),
                cx,
            );
        }
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    app.wait_for(cx, "the new stash", |model| stash_count(model) == 1);
    app.wait_for(cx, "the clean working tree", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.changes_count == 0)
    });
    app.clear_events();
    notify(&app, &repo, WatchReason::FsWatchStash, cx);
    // El historial también vuelve a leerse (pinta `refs/stash`).
    assert_eq!(app.recorded_mutations(), ["external-refs-change"]);

    repo.git(&["stash", "drop"]);
    notify(&app, &repo, WatchReason::PollStash, cx);
    app.wait_for(cx, "the dropped stash to go", |model| {
        stash_count(model) == 0
    });
}

#[test]
fn metadata_refreshes_merge_and_pick_what_to_read() {
    let refs = MetadataRefresh::from_watch_reason(WatchReason::FsWatchRefs);
    let config = MetadataRefresh::from_watch_reason(WatchReason::PollConfig);
    assert!(refs.refs && !refs.config);
    assert!(config.config && !config.refs);
    assert!(MetadataRefresh::from_watch_reason(WatchReason::FsWatchHead).is_empty());
    let both = refs.merge(config);
    assert!(both.refs && both.config);

    let request = config.sync_request();
    assert!(request.remotes && !request.tags && !request.branches && request.operation.is_none());
    assert!(!request.stashes);
    let stash = MetadataRefresh::from_watch_reason(WatchReason::FsWatchStash);
    assert!(stash.stash && !stash.refs && !stash.config);
    let request = stash.sync_request();
    assert!(request.stashes && !request.remotes && !request.tags);
    assert!(request.status && request.changes_count);
    assert_eq!(request.operation.as_deref(), Some("external-refs-change"));
    let request = refs.sync_request();
    assert!(request.remotes && request.tags && request.branches);
    assert_eq!(request.operation.as_deref(), Some("external-refs-change"));
}
