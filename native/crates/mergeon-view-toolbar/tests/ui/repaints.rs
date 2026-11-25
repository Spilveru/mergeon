//! Cuándo repintan la barra y la barra de estado. Cada aviso del modelo hace
//! que la ventana entera se vuelva a pintar, así que lo que no cambia nada a
//! la vista no debe avisar.

use std::time::Duration;

use gpui_kit::TestAppContext;
use mergeon_app::StatusSyncReason;

use crate::support::{TestApp, TestRepo};

fn renders(app: &TestApp, cx: &mut TestAppContext) -> (usize, usize) {
    app.shell.read_with(cx, |shell, cx| {
        (
            shell.toolbar.read(cx).render_count(),
            shell.status_bar.read(cx).render_count(),
        )
    })
}

/// El watcher relee el estado con cualquier cambio en el disco. Antes, una
/// relectura que traía lo mismo repintaba las dos barras 3 o 4 veces (un
/// aviso del modelo por cada paso: lectura en vuelo, su fin, el estado, la
/// mutación); ahora ninguna.
#[gpui_kit::test]
async fn a_status_watch_that_changes_nothing_repaints_neither_bar(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    assert!(app.exists(cx, "status-bar"));

    let before = renders(&app, cx);
    app.model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(StatusSyncReason::FsWatchWorkingTree, cx)
        })
        .await;
    cx.run_until_parked();
    let after = renders(&app, cx);
    eprintln!(
        "status-watch without changes: renders (toolbar, status bar) {before:?} -> {after:?}"
    );
    assert_eq!(after, before);

    // Un cambio de lo que enseñan sí llega a la ventana: un commit hecho
    // fuera mueve el HEAD de la barra. (Un archivo nuevo no: ninguna de las
    // dos lo enseña, y ya no leen el modelo al pintar.)
    repo.commit_file("new.txt", "hello\n", "Add new");
    app.model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(StatusSyncReason::FsWatchWorkingTree, cx)
        })
        .await;
    cx.run_until_parked();
    let changed = renders(&app, cx);
    assert!(changed.0 > after.0, "{changed:?}");
    assert!(changed.1 > after.1, "{changed:?}");
}

/// "fetched N min ago" se revisa cada 30 s, pero solo repinta si el texto
/// cambia (antes repintaba la ventana entera en cada vuelta del reloj).
#[gpui_kit::test]
async fn the_fetched_ago_clock_repaints_only_when_its_text_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;
    app.click(cx, "toolbar-fetch");
    app.click(cx, "fetch-submit");
    app.wait_for_label(cx, "status-fetched", "fetched just now");

    let before = renders(&app, cx);
    for _ in 0..3 {
        cx.executor().advance_clock(Duration::from_secs(31));
        cx.run_until_parked();
    }
    // El reloj de la prueba avanza, el de la pared no: el texto sigue igual.
    assert_eq!(renders(&app, cx), before);
    assert_eq!(
        app.label(cx, "status-fetched").as_deref(),
        Some("fetched just now")
    );
}

/// La barra de estado recuerda la hora del último fetch por repositorio
/// abierto; al cerrarlo la olvida (antes la guardaba para siempre).
#[gpui_kit::test]
async fn the_status_bar_forgets_the_last_fetch_of_a_closed_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    let repo_id = app.open(&repo, cx).await;
    app.click(cx, "toolbar-fetch");
    app.click(cx, "fetch-submit");
    app.wait_for_label(cx, "status-fetched", "fetched just now");
    let remembered = |cx: &mut TestAppContext| {
        app.shell.read_with(cx, |shell, cx| {
            shell.status_bar.read(cx).remembered_remote_syncs()
        })
    };
    assert_eq!(remembered(cx), 1);

    app.model.update(cx, |model, cx| {
        model.remove_repository(repo_id.as_str(), cx)
    });
    cx.run_until_parked();
    assert_eq!(remembered(cx), 0);
}

fn slot_renders(app: &TestApp, cx: &mut TestAppContext) -> usize {
    app.shell
        .read_with(cx, |shell, cx| shell.status.read(cx).render_count())
}

/// Un aviso cualquiera del modelo (un toast, un cambio que no es suyo).
fn unrelated_notifications(app: &TestApp, cx: &mut TestAppContext) {
    for index in 0..3 {
        app.model.update(cx, |model, cx| {
            model.notify_info(&format!("Something else happened {index}"), cx);
            cx.notify();
        });
        cx.run_until_parked();
    }
}

async fn sync_status(app: &TestApp, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(StatusSyncReason::FsWatchWorkingTree, cx)
        })
        .await;
    cx.run_until_parked();
}

/// La franja de operación solo se repinta cuando cambia lo que enseña: sin
/// operación de secuencia en curso, ningún aviso del modelo la repinta;
/// empezar, avanzar y terminar un merge sí. (Cada vista de la ventana se
/// pinta en cada frame: que no se repinte quiere decir que ninguna pidió
/// frame, porque ninguna lee el modelo al pintar ni avisa sin cambios.)
#[gpui_kit::test]
async fn the_operation_slot_repaints_only_when_its_banner_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("notes.txt", "base\n", "Add notes");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("notes.txt", "from feature\n", "Notes on feature");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("notes.txt", "from main\n", "Notes on main");
    app.open(&repo, cx).await;
    assert!(!app.exists(cx, "operation-status-slot"));

    // En reposo: ni avisos ni relecturas del estado la repintan.
    let idle = slot_renders(&app, cx);
    unrelated_notifications(&app, cx);
    sync_status(&app, cx).await;
    assert_eq!(slot_renders(&app, cx), idle);

    // Empieza un merge (desde fuera, como en un terminal): aparece la franja.
    // (Se cuenta antes de mirar la ventana: cada consulta pinta un frame.)
    assert!(repo.try_git(&["merge", "feature"]).is_err());
    sync_status(&app, cx).await;
    let started = slot_renders(&app, cx);
    assert!(started > idle, "{idle} -> {started}");
    assert_eq!(
        app.label(cx, "operation-banner").as_deref(),
        Some("Merge in progress")
    );

    // Con la franja a la vista, lo ajeno tampoco la repinta.
    let shown = slot_renders(&app, cx);
    unrelated_notifications(&app, cx);
    assert_eq!(slot_renders(&app, cx), shown);

    // Avanza: el conflicto queda resuelto.
    repo.write("notes.txt", "from main and feature\n");
    repo.git(&["add", "notes.txt"]);
    sync_status(&app, cx).await;
    let resolved = slot_renders(&app, cx);
    assert!(resolved > shown, "{shown} -> {resolved}");
    assert_eq!(
        app.label(cx, "operation-banner").as_deref(),
        Some("All conflicts resolved. Ready to continue merge.")
    );

    // Termina (lo que hace su botón Continue): la franja desaparece.
    let before_continue = slot_renders(&app, cx);
    app.model
        .update(cx, |model, cx| model.handle_sequence_action("continue", cx))
        .detach();
    app.wait_for(cx, "the merge to finish", |model| {
        model.active_repository().is_some_and(|repository| {
            !repository
                .operation_state
                .as_ref()
                .is_some_and(|state| state.in_progress)
        })
    });
    let finished = slot_renders(&app, cx);
    assert!(
        finished > before_continue,
        "{before_continue} -> {finished}"
    );
    assert!(!app.exists(cx, "operation-status-slot"));

    // Y de vuelta en reposo, nada ajeno la repinta.
    let idle = slot_renders(&app, cx);
    unrelated_notifications(&app, cx);
    assert_eq!(slot_renders(&app, cx), idle);
}
