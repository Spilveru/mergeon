//! Workspaces (grupos nombrados de repositorios): filtrado y orden de
//! pestañas, apertura de repositorios del grupo sin cerrar otros, estado por
//! repositorio conservado, cierre inteligente dentro de un grupo, restauración
//! de sesión y flujos con prompt/confirmación.

use gpui_kit::TestAppContext;
use mergeon_model::workspace_tabs::FileTabDescriptor;

use super::support::{TestApp, TestRepo};
use crate::testing::TestBackend;

fn visible_ids(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    app.model.read_with(cx, |model, _| {
        model
            .visible_repositories()
            .into_iter()
            .map(|repo| repo.id.clone())
            .collect()
    })
}

fn open_ids(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    app.model.read_with(cx, |model, _| {
        model
            .open_repositories()
            .iter()
            .map(|repo| repo.id.clone())
            .collect()
    })
}

fn active_id(app: &TestApp, cx: &mut TestAppContext) -> Option<String> {
    app.model.read_with(cx, |model, _| {
        model.active_repository().map(|repo| repo.id.clone())
    })
}

#[gpui_kit::test]
async fn switching_workspaces_filters_tabs_opens_missing_repos_and_preserves_per_repo_state(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo_a = TestRepo::new();
    let repo_b = TestRepo::new();
    let repo_c = TestRepo::new();

    let id_a = app.open(&repo_a, cx).await;
    let id_b = app.open(&repo_b, cx).await;
    let id_c = repo_c.id();

    // Estado por repositorio: pestaña de archivo y borrador de commit.
    let file_tab = FileTabDescriptor::working("README.md");
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&id_a, &file_tab, false, cx);
        model.set_commit_draft(&id_a, "Draft for A", cx);
        model.set_commit_draft(&id_b, "Draft for B", cx);
    });
    cx.run_until_parked();

    let (ws_backend, ws_frontend) = app.model.update(cx, |model, cx| {
        let backend = model
            .create_workspace("Backend", vec![id_a.clone(), id_c.clone()], cx)
            .expect("backend workspace");
        let frontend = model
            .create_workspace("Frontend", vec![id_b.clone()], cx)
            .expect("frontend workspace");
        (backend, frontend)
    });

    // Cambiar a Backend: abre C sin cerrar B, filtra pestañas en orden [A, C] y activa A.
    app.model
        .update(cx, |model, cx| {
            model.select_workspace(Some(ws_backend.id.clone()), cx)
        })
        .await;
    cx.run_until_parked();

    assert_eq!(visible_ids(&app, cx), vec![id_a.clone(), id_c.clone()]);
    assert_eq!(
        open_ids(&app, cx),
        vec![id_a.clone(), id_b.clone(), id_c.clone()]
    );
    assert_eq!(active_id(&app, cx), Some(id_a.clone()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.repository_workspace(&id_a).tabs.len(), 1);
        assert_eq!(model.commit_draft(&id_a), "Draft for A");
    });

    // Activar C dentro de Backend y cambiar a Frontend.
    app.model
        .update(cx, |model, cx| model.select_visible_repository_tab(1, cx));
    cx.run_until_parked();
    assert_eq!(active_id(&app, cx), Some(id_c.clone()));

    app.model
        .update(cx, |model, cx| {
            model.select_workspace(Some(ws_frontend.id.clone()), cx)
        })
        .await;
    cx.run_until_parked();
    assert_eq!(visible_ids(&app, cx), vec![id_b.clone()]);
    assert_eq!(active_id(&app, cx), Some(id_b.clone()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.commit_draft(&id_b), "Draft for B");
    });

    // Volver a Backend recuerda que el último activo era C.
    app.model
        .update(cx, |model, cx| {
            model.select_workspace(Some(ws_backend.id.clone()), cx)
        })
        .await;
    cx.run_until_parked();
    assert_eq!(active_id(&app, cx), Some(id_c.clone()));
    assert_eq!(visible_ids(&app, cx), vec![id_a.clone(), id_c.clone()]);

    // Volver a "All" muestra todos los repositorios abiertos.
    app.model
        .update(cx, |model, cx| model.select_workspace(None, cx))
        .await;
    cx.run_until_parked();
    assert_eq!(
        visible_ids(&app, cx),
        vec![id_a.clone(), id_b.clone(), id_c.clone()]
    );
}

#[gpui_kit::test]
async fn tab_operations_operate_over_visible_workspace_tabs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repos = [
        TestRepo::new(),
        TestRepo::new(),
        TestRepo::new(),
        TestRepo::new(),
    ];
    for repo in &repos {
        app.open(repo, cx).await;
    }
    let ids: Vec<String> = repos.iter().map(TestRepo::id).collect();

    // Workspace con orden propio: [3, 1, 0]; el repo 2 queda fuera.
    let ws = app.model.update(cx, |model, cx| {
        model
            .create_workspace(
                "Focus",
                vec![ids[3].clone(), ids[1].clone(), ids[0].clone()],
                cx,
            )
            .expect("workspace")
    });
    app.model
        .update(cx, |model, cx| {
            model.select_workspace(Some(ws.id.clone()), cx)
        })
        .await;
    cx.run_until_parked();

    assert_eq!(
        visible_ids(&app, cx),
        vec![ids[3].clone(), ids[1].clone(), ids[0].clone()]
    );
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.repository_tab_labels().len()),
        3
    );

    // Recorrer y elegir por número sigue el orden visible del workspace.
    assert_eq!(active_id(&app, cx), Some(ids[3].clone()));
    app.model
        .update(cx, |model, cx| model.cycle_repository_tab(true, cx));
    cx.run_until_parked();
    assert_eq!(active_id(&app, cx), Some(ids[1].clone()));
    app.model
        .update(cx, |model, cx| model.select_repository_tab_number(3, cx));
    cx.run_until_parked();
    assert_eq!(active_id(&app, cx), Some(ids[0].clone()));
    app.model
        .update(cx, |model, cx| model.select_repository_tab_number(9, cx));
    cx.run_until_parked();
    assert_eq!(active_id(&app, cx), Some(ids[0].clone()));

    // Reordenar pestañas dentro del workspace no altera el orden global.
    app.model
        .update(cx, |model, cx| model.move_repository_tab(0, 2, cx));
    cx.run_until_parked();
    assert_eq!(
        visible_ids(&app, cx),
        vec![ids[1].clone(), ids[0].clone(), ids[3].clone()]
    );
    assert_eq!(open_ids(&app, cx), ids);

    // Close to the right solo afecta a las pestañas a la derecha en el workspace.
    let closed = app
        .model
        .update(cx, |model, cx| {
            model.close_repositories_to_the_right(&ids[0], cx)
        })
        .await;
    assert!(closed);
    cx.run_until_parked();
    assert_eq!(visible_ids(&app, cx), vec![ids[1].clone(), ids[0].clone()]);
    // Repo 2 (fuera del workspace) sigue abierto; repo 3 se cerró porque no estaba en otro workspace.
    assert_eq!(
        open_ids(&app, cx),
        vec![ids[0].clone(), ids[1].clone(), ids[2].clone()]
    );

    // Close others solo afecta a los demás del workspace.
    let closed = app
        .model
        .update(cx, |model, cx| model.close_other_repositories(&ids[1], cx))
        .await;
    assert!(closed);
    cx.run_until_parked();
    assert_eq!(visible_ids(&app, cx), vec![ids[1].clone()]);
    assert_eq!(open_ids(&app, cx), vec![ids[1].clone(), ids[2].clone()]);
}

#[gpui_kit::test]
async fn opening_closing_and_deleting_respect_workspace_membership_rules(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let shared = TestRepo::new();
    let unique = TestRepo::new();
    let added_later = TestRepo::new();

    let id_shared = app.open(&shared, cx).await;
    let id_unique = app.open(&unique, cx).await;

    let (ws1, ws2) = app.model.update(cx, |model, cx| {
        let ws1 = model
            .create_workspace("W1", vec![id_shared.clone(), id_unique.clone()], cx)
            .expect("ws1");
        let ws2 = model
            .create_workspace("W2", vec![id_shared.clone()], cx)
            .expect("ws2");
        (ws1, ws2)
    });
    app.model
        .update(cx, |model, cx| {
            model.select_workspace(Some(ws1.id.clone()), cx)
        })
        .await;
    cx.run_until_parked();

    // Abrir un repositorio nuevo estando en W1 lo añade automáticamente a W1.
    let id_added = app.open(&added_later, cx).await;
    assert_eq!(
        visible_ids(&app, cx),
        vec![id_shared.clone(), id_unique.clone(), id_added.clone()]
    );

    // Cerrar un repositorio que también pertenece a W2 solo lo quita de W1
    // sin cerrarlo ni pedir confirmación aunque tenga borrador de commit.
    app.model.update(cx, |model, cx| {
        model.set_commit_draft(&id_shared, "Shared draft", cx);
        model.set_commit_draft(&id_unique, "Unique draft", cx);
    });
    let closed = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![id_shared.clone()], cx)
        })
        .await;
    assert!(closed);
    assert!(app.host.confirms().is_empty());
    assert_eq!(
        visible_ids(&app, cx),
        vec![id_unique.clone(), id_added.clone()]
    );
    assert!(open_ids(&app, cx).contains(&id_shared));
    assert!(app.model.read_with(cx, |model, _| {
        model.is_repository_in_workspace(&id_shared, &ws2.id)
    }));

    // Cerrar un repositorio que solo está en W1 y tiene borrador pide confirmación.
    app.host.answer_confirm(false);
    let cancelled = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![id_unique.clone()], cx)
        })
        .await;
    assert!(!cancelled);
    assert_eq!(
        visible_ids(&app, cx),
        vec![id_unique.clone(), id_added.clone()]
    );

    app.host.answer_confirm(true);
    let confirmed = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![id_unique.clone()], cx)
        })
        .await;
    assert!(confirmed);
    assert_eq!(visible_ids(&app, cx), vec![id_added.clone()]);
    assert!(!open_ids(&app, cx).contains(&id_unique));

    // Cerrar el último repositorio del workspace lleva a Home (active = None)
    // sin saltar al repositorio abierto de W2.
    let closed_last = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![id_added.clone()], cx)
        })
        .await;
    assert!(closed_last);
    assert!(visible_ids(&app, cx).is_empty());
    assert_eq!(active_id(&app, cx), None);
    assert_eq!(open_ids(&app, cx), vec![id_shared.clone()]);

    // Borrar un workspace cambia a "All" sin cerrar sus repositorios.
    app.model
        .update(cx, |model, cx| {
            model.select_workspace(Some(ws2.id.clone()), cx)
        })
        .await;
    cx.run_until_parked();
    assert_eq!(active_id(&app, cx), Some(id_shared.clone()));
    app.model
        .update(cx, |model, cx| model.delete_workspace(&ws2.id, cx));
    cx.run_until_parked();
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_workspace_id(), None);
        assert_eq!(model.open_repositories().len(), 1);
    });
}

#[gpui_kit::test]
async fn restores_active_workspace_from_storage_without_polluting_its_membership(
    cx: &mut TestAppContext,
) {
    let repo_a = TestRepo::new();
    let repo_b = TestRepo::new();
    let repo_c = TestRepo::new();
    let store = TestBackend::memory_store();
    store
        .set_workspace_session(&mergeon_store::WorkspaceSession {
            open_repositories: vec![repo_a.id(), repo_b.id()],
            active_repository_path: Some(repo_a.id()),
            workspaces: vec![mergeon_store::RepoWorkspace {
                id: "ws-work".into(),
                name: "Work".into(),
                repositories: vec![repo_a.id(), repo_c.id()],
                active_repository_path: Some(repo_c.id()),
            }],
            active_workspace_id: Some("ws-work".into()),
            ..Default::default()
        })
        .unwrap();

    let app = TestApp::with_backend(TestBackend::new().with_store(store), cx);
    app.model.update(cx, |model, cx| model.start(cx)).await;
    cx.run_until_parked();

    // Se restauraron A, B y C, pero el workspace activo sigue conteniendo solo [A, C].
    assert_eq!(
        open_ids(&app, cx),
        vec![repo_a.id(), repo_b.id(), repo_c.id()]
    );
    assert_eq!(visible_ids(&app, cx), vec![repo_a.id(), repo_c.id()]);
    assert_eq!(active_id(&app, cx), Some(repo_a.id()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_workspace_id(), Some("ws-work"));
        assert!(!model.is_repository_in_workspace(&repo_b.id(), "ws-work"));
    });

    // Flujos con prompt / confirmación: crear, renombrar y borrar.
    app.host.answer_prompt("Side Projects");
    let created = app
        .model
        .update(cx, |model, cx| {
            model.prompt_create_workspace(vec![repo_b.id()], cx)
        })
        .await
        .expect("created via prompt");
    cx.run_until_parked();
    assert_eq!(created.name, "Side Projects");
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .active_workspace_id()
            .map(str::to_string)),
        Some(created.id.clone())
    );
    assert_eq!(visible_ids(&app, cx), vec![repo_b.id()]);

    app.host.answer_prompt("Hobby");
    let renamed = app
        .model
        .update(cx, |model, cx| {
            model.prompt_rename_workspace(&created.id, cx)
        })
        .await;
    assert!(renamed);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_workspace().unwrap().name, "Hobby");
    });

    app.host.answer_confirm(true);
    let deleted = app
        .model
        .update(cx, |model, cx| {
            model.confirm_delete_workspace(&created.id, cx)
        })
        .await;
    assert!(deleted);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_workspace_id(), None);
        assert_eq!(model.workspaces().len(), 1);
    });
}
