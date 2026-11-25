//! Lo no puro de `src/stores/repositories.test.js`: lo que el store guardaba
//! (recientes y sesión), aquí contra la configuración en memoria. Las reglas
//! puras del store se prueban en `mergeon-model`.

use gpui_kit::TestAppContext;
use mergeon_model::repositories::SidebarSection;

use super::support::{TestApp, TestRepo};
use crate::testing::TestBackend;
use crate::{AddRepository, AppEvent};

async fn add(app: &TestApp, path: &str, options: AddRepository, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| model.add_repository(path, options, cx))
        .await
        .expect("add repository");
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn refreshes_recent_repositories_even_when_storage_was_already_loaded(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.model
        .update(cx, |model, cx| model.load_repositories_from_storage(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(model.repositories().loaded);
        assert!(model.recent_repositories().is_empty());
    });

    add(
        &app,
        &repo.path_string(),
        AddRepository {
            is_github: true,
            remote_url: Some("https://github.com/demo/repo.git".to_string()),
            ..Default::default()
        },
        cx,
    )
    .await;

    app.model.read_with(cx, |model, _| {
        let recent = model.recent_repositories();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].path, repo.id());
        assert_eq!(recent[0].name, "repo");
        assert!(recent[0].is_github);
        assert_eq!(recent[0].remote_url, "https://github.com/demo/repo.git");
        // La fecha de apertura la pone el almacenamiento.
        assert!(recent[0].last_opened.is_some());
    });
    assert_eq!(app.backend.storage().get_recent_repositories().len(), 1);
}

#[gpui_kit::test]
async fn deduplicates_equivalent_repository_paths_and_keeps_a_stable_repository_id(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = repo.path_string();

    add(&app, &path, AddRepository::default(), cx).await;
    add(&app, &format!("{path}/"), AddRepository::default(), cx).await;
    add(
        &app,
        "/somewhere/else",
        AddRepository {
            canonical_path: Some(path.clone()),
            ..Default::default()
        },
        cx,
    )
    .await;

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.open_repositories().len(), 1);
        assert_eq!(model.open_repositories()[0].id, repo.id());
        assert_eq!(model.active_repository_index(), Some(0));
    });
    assert_eq!(app.backend.storage().get_recent_repositories().len(), 1);
    // Reabrir la misma pestaña no vuelve a hidratar el repositorio.
    assert_eq!(app.recorded_mutations(), ["hydrate-opened-repository"]);
}

#[gpui_kit::test]
async fn persists_the_workspace_session_when_adding_selecting_and_removing_repositories(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let first = TestRepo::new();
    let second = TestRepo::new();
    let stored_session = |app: &TestApp| app.backend.storage().get_workspace_session();

    add(&app, &first.path_string(), AddRepository::default(), cx).await;
    add(&app, &second.path_string(), AddRepository::default(), cx).await;

    let session = stored_session(&app);
    assert_eq!(session.open_repositories, [first.id(), second.id()]);
    assert_eq!(session.active_repository_path, Some(second.id()));

    app.model
        .update(cx, |model, cx| model.set_active_repository(Some(0), cx));
    cx.run_until_parked();
    assert_eq!(
        stored_session(&app).active_repository_path,
        Some(first.id())
    );

    app.model.update(cx, |model, cx| {
        model.remove_repository(first.id().as_str(), cx)
    });
    cx.run_until_parked();
    let session = stored_session(&app);
    assert_eq!(session.open_repositories, [second.id()]);
    assert_eq!(session.active_repository_path, Some(second.id()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().map(|repo| repo.id.clone()),
            Some(second.id())
        );
        assert_eq!(model.workspace_session().open_repositories, [second.id()]);
    });

    app.model
        .update(cx, |model, cx| model.set_active_repository(None, cx));
    cx.run_until_parked();
    assert_eq!(stored_session(&app).active_repository_path, None);

    // Un índice que no existe no cambia nada.
    app.model
        .update(cx, |model, cx| model.set_active_repository(Some(7), cx));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_repository_index(), None)
    });

    let changes: Vec<_> = app
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AppEvent::ActiveRepositoryChanged { repo_id } => Some(repo_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        changes,
        [
            Some(first.id()),
            Some(second.id()),
            Some(first.id()),
            Some(second.id()),
            None
        ]
    );
}

#[gpui_kit::test]
async fn loads_recents_and_session_from_storage_once(cx: &mut TestAppContext) {
    let store = TestBackend::memory_store();
    store
        .add_recent_repository(&mergeon_store::RecentRepository {
            id: "/tmp/demo".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            current_branch: "main".to_string(),
            ..Default::default()
        })
        .unwrap();
    store
        .set_workspace_session(&mergeon_store::WorkspaceSession {
            open_repositories: vec!["/tmp/demo".to_string()],
            active_repository_path: Some("/tmp/demo".to_string()),
            ..Default::default()
        })
        .unwrap();
    let app = TestApp::with_backend(TestBackend::new().with_store(store), cx);

    app.model
        .update(cx, |model, cx| model.load_repositories_from_storage(cx))
        .await;

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.recent_repositories().len(), 1);
        assert_eq!(model.recent_repositories()[0].name, "demo");
        assert_eq!(model.workspace_session().open_repositories, ["/tmp/demo"]);
        // Cargar la sesión no abre nada: eso es `restore_workspace_session`.
        assert!(model.open_repositories().is_empty());
    });

    // Lo que cambie en disco después no se relee.
    app.backend.storage().clear_recent_repositories().unwrap();
    app.model
        .update(cx, |model, cx| model.load_repositories_from_storage(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.recent_repositories().len(), 1)
    });
}

#[gpui_kit::test]
async fn removes_and_clears_recent_repositories(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let first = TestRepo::new();
    let second = TestRepo::new();
    add(&app, &first.path_string(), AddRepository::default(), cx).await;
    add(&app, &second.path_string(), AddRepository::default(), cx).await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.recent_repositories().len(), 2)
    });

    app.model
        .update(cx, |model, cx| {
            model.remove_recent_repository(&first.id(), cx)
        })
        .await
        .unwrap();
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.recent_repositories().len(), 1);
        assert_eq!(model.recent_repositories()[0].path, second.id());
    });
    assert_eq!(app.backend.storage().get_recent_repositories().len(), 1);

    app.model
        .update(cx, |model, cx| model.clear_recent_repositories(cx))
        .await
        .unwrap();
    app.model.read_with(cx, |model, _| {
        assert!(model.recent_repositories().is_empty())
    });
    assert!(app.backend.storage().get_recent_repositories().is_empty());
}

#[gpui_kit::test]
async fn stars_and_unstars_branches(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.seed(&repo, cx);

    let starred = app
        .model
        .update(cx, |model, cx| {
            model.add_starred_branch(&repo_id, "main", cx)
        })
        .await
        .unwrap();
    assert_eq!(starred, ["main"]);
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.starred_branches, ["main"]);
        assert!(repository.sidebar_loaded.starred_branches);
    });
    assert_eq!(
        app.backend.storage().get_starred_branches(&repo_id),
        ["main"]
    );

    let starred = app
        .model
        .update(cx, |model, cx| {
            model.remove_starred_branch(&repo_id, "main", cx)
        })
        .await
        .unwrap();
    assert!(starred.is_empty());
    app.model.read_with(cx, |model, _| {
        assert!(
            model
                .repository(repo_id.as_str())
                .unwrap()
                .starred_branches
                .is_empty()
        );
    });
}

#[gpui_kit::test]
async fn store_actions_reach_the_repository_by_id_or_position(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.seed(&repo, cx);

    app.model.update(cx, |model, cx| {
        model.set_repository_tags_data(repo_id.as_str(), vec!["v1".to_string()], cx);
        model.set_repository_sidebar_loading(0usize, SidebarSection::Stashes, true, cx);
        model.update_repository(repo_id.as_str(), cx, |repository| {
            repository.name = "renamed".to_string();
        });
        // Un repositorio que no existe se ignora sin más.
        model.set_repository_tags_data("missing", vec!["nope".to_string()], cx);
    });

    app.model.read_with(cx, |model, _| {
        let repository = model.repository(0usize).unwrap();
        assert_eq!(repository.tags, ["v1"]);
        assert!(repository.sidebar_loaded.tags);
        assert!(repository.sidebar_loading.stashes);
        assert_eq!(repository.name, "renamed");
        assert_eq!(model.find_repository_index(repo_id.as_str()), Some(0));
        assert_eq!(model.find_repository_index("missing"), None);
    });

    app.model.update(cx, |model, cx| {
        model.invalidate_repository_sidebar_sections(repo_id.as_str(), &[SidebarSection::Tags], cx);
    });
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert!(repository.tags.is_empty());
        assert!(!repository.sidebar_loaded.tags);
    });
}
