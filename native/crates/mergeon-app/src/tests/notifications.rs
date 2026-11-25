//! Cuántas veces avisa el modelo a sus observadores. Cada aviso hace que las
//! vistas que leen `AppModel` repinten la ventana entera, así que una
//! relectura tiene que avisar pocas veces y una que no trae nada nuevo,
//! ninguna.

use gpui_kit::TestAppContext;

use super::support::{TestApp, TestRepo};
use crate::{AppEvent, StatusSyncReason};

/// Abrir (o restaurar) un repositorio y leerlo entero —estado, ramas,
/// remotos, ramas destacadas— avisaba ~14 veces: cada sección avisaba al
/// marcarse "cargando", al registrar su lectura en vuelo, al retirarla y al
/// aplicar el dato. Ahora los pasos que corren seguidos se agrupan y la
/// lectura en vuelo no avisa: un aviso por sección que cambia.
#[gpui_kit::test]
async fn hydrating_an_opened_repository_notifies_once_per_section_read(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    app.seed(&repo, cx);
    cx.run_until_parked();

    let notifications = app.count_notifications(cx);
    app.model
        .update(cx, |model, cx| model.hydrate_opened_repository(cx))
        .await;
    cx.run_until_parked();

    let count = notifications.get();
    eprintln!("hydrate_opened_repository: {count} notifications");
    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().unwrap();
        assert!(repository.status.is_some());
        assert!(repository.sidebar_loaded.branches);
        assert!(repository.sidebar_loaded.remotes);
        assert!(repository.sidebar_loaded.starred_branches);
        assert!(!repository.sidebar_loading.branches);
        assert!(!repository.sidebar_loading.remotes);
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("hydrate-opened-repository")
        );
    });
    // Estado, ramas, remotos y destacadas: una por sección, como mucho.
    assert!((1..=4).contains(&count), "hydrate notified {count} times");
}

/// El watcher relee el estado con cualquier cambio en el disco. Si lo que
/// trae es lo que ya había, no avisa a `observe` (la toolbar, la barra de
/// estado, la cáscara... no repintan); la mutación sí se anuncia como evento
/// para quien recargue cifras o diff.
#[gpui_kit::test]
async fn a_status_watch_that_finds_nothing_new_does_not_notify(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.seed(&repo, cx);
    app.model
        .update(cx, |model, cx| model.hydrate_opened_repository(cx))
        .await;
    cx.run_until_parked();
    app.clear_events();

    let notifications = app.count_notifications(cx);
    app.model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(StatusSyncReason::FsWatchWorkingTree, cx)
        })
        .await;
    cx.run_until_parked();

    eprintln!(
        "status-watch without changes: {} notifications",
        notifications.get()
    );
    assert_eq!(notifications.get(), 0);
    assert!(app.events().contains(&AppEvent::MutationRecorded {
        repo_id: repo_id.clone(),
        operation: Some("status-watch".to_string()),
    }));

    // Con un cambio de verdad, sí (y una sola vez).
    repo.write("new.txt", "hello\n");
    notifications.reset();
    app.model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(StatusSyncReason::FsWatchWorkingTree, cx)
        })
        .await;
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_repository().unwrap().changes_count, 1);
    });
}
