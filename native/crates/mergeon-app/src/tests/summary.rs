//! Tests de humo de `useRepositorySummary`.

use gpui_kit::TestAppContext;

use super::support::{TestApp, TestRepo};

#[gpui_kit::test]
async fn reads_a_summary_once_and_serves_it_from_the_cache(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = repo.path_string();

    let (first, second) = app.model.update(cx, |model, cx| {
        let first = model.load_repository_summary(&path, cx);
        assert_eq!(model.loading_summary_path(), Some(path.as_str()));
        (first, model.load_repository_summary(&path, cx))
    });
    let (first, second) = (
        first.await.expect("summary"),
        second.await.expect("summary"),
    );

    assert_eq!(first, second);
    assert!(first.is_valid);
    assert_eq!(first.name, "repo");
    assert_eq!(first.commit_count, 1);
    assert_eq!(first.current_branch.as_deref(), Some("main"));
    assert_eq!(first.readme_content.as_deref(), Some("# Demo\n"));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.loading_summary_path(), None);
        assert_eq!(model.repository_summary(&path), Some(&first));
    });

    // Mientras HEAD y las ramas no cambien se sirve el guardado sin releer:
    // un archivo nuevo en el working tree no aparece en `changes_count`.
    repo.write("untracked.txt", "x\n");
    let cached = app
        .model
        .update(cx, |model, cx| model.load_repository_summary(&path, cx))
        .await
        .unwrap();
    assert_eq!(cached, first);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.loading_summary_path(), None);
    });

    // Un commit mueve HEAD: se relee sin pasar por el estado de carga.
    repo.commit_file("a.txt", "a\n", "Second");
    let fresh = app
        .model
        .update(cx, |model, cx| {
            let fresh = model.load_repository_summary(&path, cx);
            assert_eq!(model.loading_summary_path(), None);
            fresh
        })
        .await
        .unwrap();
    assert_eq!(fresh.commit_count, 2);
    assert_eq!(fresh.changes_count, 1);
    assert_eq!(fresh.head_oid.as_deref(), Some(repo.head().as_str()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.repository_summary(&path), Some(&fresh));
    });

    // Limpiar la entrada obliga a leer de cero.
    app.model
        .update(cx, |model, cx| model.clear_repository_summary(&path, cx));
    let reread = app
        .model
        .update(cx, |model, cx| model.load_repository_summary(&path, cx))
        .await
        .unwrap();
    assert_eq!(reread, fresh);
}

#[gpui_kit::test]
async fn describes_paths_that_are_not_repositories_and_prunes_the_cache(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let plain = TestRepo::plain_directory();

    assert_eq!(
        app.model
            .update(cx, |model, cx| model.load_repository_summary("", cx))
            .await,
        None
    );

    let invalid = app
        .model
        .update(cx, |model, cx| {
            model.load_repository_summary(&plain.path_string(), cx)
        })
        .await
        .expect("summary");
    assert!(!invalid.is_valid);
    assert_eq!(invalid.name, "repo");

    app.model
        .update(cx, |model, cx| {
            model.load_repository_summary(&repo.path_string(), cx)
        })
        .await;
    app.model.update(cx, |model, cx| {
        assert_eq!(model.summary_cache().len(), 2);
        model.retain_repository_summaries(cx, |_, summary| summary.is_valid);
        assert_eq!(model.summary_cache().len(), 1);
        assert!(model.repository_summary(&repo.path_string()).is_some());
    });
}

/// Cada resumen lleva el README entero: la caché tiene tope y olvida el que
/// hace más que no se pide.
#[gpui_kit::test]
async fn the_summary_cache_forgets_the_least_recently_requested(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let root = TestRepo::plain_directory();
    // Rutas que no son repositorios: su resumen (no válido) sale sin git.
    let paths: Vec<String> = (0..=crate::summary::MAX_SUMMARIES)
        .map(|index| {
            root.path
                .join(format!("missing-{index}"))
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    for path in &paths[..crate::summary::MAX_SUMMARIES] {
        app.model
            .update(cx, |model, cx| model.load_repository_summary(path, cx))
            .await;
    }
    // Se vuelve a pedir el primero: pasa a ser el más reciente.
    app.model
        .update(cx, |model, cx| model.load_repository_summary(&paths[0], cx))
        .await;
    app.model
        .update(cx, |model, cx| {
            model.load_repository_summary(&paths[crate::summary::MAX_SUMMARIES], cx)
        })
        .await;

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.summary_cache().len(), crate::summary::MAX_SUMMARIES);
        assert!(model.repository_summary(&paths[0]).is_some());
        assert!(model.repository_summary(&paths[1]).is_none());
        assert!(
            model
                .repository_summary(&paths[crate::summary::MAX_SUMMARIES])
                .is_some()
        );
    });
}

/// Al cerrar un repositorio que ya no está entre los recientes su resumen
/// no se volverá a ver: se olvida. Uno reciente lo conserva para el inicio.
#[gpui_kit::test]
async fn closing_a_repository_drops_its_summary_unless_it_is_recent(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let recent = TestRepo::new();
    let forgotten = TestRepo::new();
    let recent_id = app.open(&recent, cx).await;
    let forgotten_id = app.open(&forgotten, cx).await;
    app.model
        .update(cx, |model, cx| {
            model.remove_recent_repository(&forgotten.path_string(), cx)
        })
        .await
        .unwrap();
    for repo in [&recent, &forgotten] {
        app.model
            .update(cx, |model, cx| {
                model.load_repository_summary(&repo.path_string(), cx)
            })
            .await;
    }

    app.model.update(cx, |model, cx| {
        model.remove_repository(forgotten_id.as_str(), cx);
        model.remove_repository(recent_id.as_str(), cx);
    });

    app.model.read_with(cx, |model, _| {
        assert!(model.repository_summary(&recent.path_string()).is_some());
        assert!(model.repository_summary(&forgotten.path_string()).is_none());
    });
}
