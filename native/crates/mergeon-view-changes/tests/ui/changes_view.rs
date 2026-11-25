//! `ChangesView` (`ChangesView.test.js` y lo que el original repartía entre la
//! vista y su `RepositoryWorkspace`): la vista entera sobre repositorios git
//! de verdad.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use mergeon_model::notify::NotifyType;
use mergeon_model::workspace_tabs::{FileTabDescriptor, HOME_TAB, TabSource};
use mergeon_view_changes::{BusyAction, ChangesMode, ChangesView, ChangesViewEvent};

use crate::support::{TestApp, TestRepo};

/// Un repositorio con `src/a.js` cambiado en el índice y, otra vez, en el
/// working tree (`MM`).
fn repo_with_both_sides() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "uno\n", "Add a");
    repo.write("src/a.js", "staged\n");
    repo.git(&["add", "src/a.js"]);
    repo.write("src/a.js", "working\n");
    repo
}

fn tabs(app: &TestApp, cx: &mut TestAppContext, repo_id: &str) -> Vec<(String, TabSource)> {
    let path = app.repo_path(cx, repo_id);
    app.model.read_with(cx, |model, _| {
        model
            .repository_workspace(&path)
            .tabs
            .into_iter()
            .map(|tab| (tab.path, tab.source))
            .collect()
    })
}

fn opened(app: &TestApp) -> Vec<(String, TabSource, bool)> {
    app.events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            ChangesViewEvent::OpenFile {
                descriptor,
                replace,
            } => Some((descriptor.path.clone(), descriptor.source, *replace)),
            _ => None,
        })
        .collect()
}

fn diff(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> String {
    view.read_with(cx, |view, _| view.file_diff().to_string())
}

fn selected(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> Option<(String, String)> {
    view.read_with(cx, |view, _| {
        view.selected_file().map(|file| {
            (
                file.path.clone(),
                format!("{}{}", file.index, file.working_dir),
            )
        })
    })
}

fn wait_for_diff(app: &TestApp, view: &Entity<ChangesView>, cx: &mut TestAppContext, text: &str) {
    let view = view.clone();
    let text = text.to_string();
    app.wait_until(cx, &format!("a diff with {text:?}"), move |cx| {
        view.read_with(cx, |view, _| {
            !view.is_loading_diff() && view.file_diff().contains(text.as_str())
        })
    });
}

/// Lo que hace la app cuando el watcher ve cambiar el working tree.
async fn watcher_reread(app: &TestApp, repo_id: &str, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id,
                mergeon_app::SyncRequest::new()
                    .status(true)
                    .operation("status-watch"),
                cx,
            )
        })
        .await;
}

fn type_message(app: &TestApp, cx: &mut TestAppContext, message: &str) {
    app.click(cx, "commit-title");
    let message = message.to_string();
    app.in_window(cx, move |window, cx| window.input(&message, cx));
}

// ---------------------------------------------------------------------------
// file workspace integration
// ---------------------------------------------------------------------------

/// "Local Changes" enseña el diff del archivo elegido a la derecha de la
/// lista: nada elegido al entrar, cada lado de un archivo a medio preparar
/// con su diff, y el doble clic abre la pestaña del archivo.
#[gpui_kit::test]
async fn local_changes_shows_the_diff_of_each_side_beside_the_list(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_both_sides();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    // Al entrar no hay nada elegido: la columna del diff lo dice.
    assert_eq!(selected(&view, cx), None);
    assert_eq!(diff(&view, cx), "");
    assert_eq!(
        app.label(cx, "changes-no-selection").as_deref(),
        Some("Select a file to see its changes")
    );
    assert!(!app.exists(cx, "toggle-diff-view"));

    app.click(cx, "unstaged:file:src/a.js");
    wait_for_diff(&app, &view, cx, "+working");
    assert!(diff(&view, cx).contains("-staged"));
    assert!(!app.exists(cx, "changes-no-selection"));
    assert!(app.exists(cx, "toggle-diff-view"));

    app.click(cx, "staged:file:src/a.js");
    wait_for_diff(&app, &view, cx, "+staged");
    assert!(diff(&view, cx).contains("-uno"));

    // El clic no abre pestañas; el doble clic sí.
    assert!(opened(&app).is_empty());
    assert!(tabs(&app, cx, &repo_id).is_empty());
    app.double_click(cx, "unstaged:file:src/a.js");
    app.settle(cx, &view);
    assert_eq!(
        opened(&app),
        [("src/a.js".to_string(), TabSource::Working, false)]
    );
    assert_eq!(
        tabs(&app, cx, &repo_id),
        [("src/a.js".to_string(), TabSource::Working)]
    );
}

/// El lado sin preparar de un archivo a medio preparar sigue elegido tras una
/// relectura del watcher (antes saltaba al lado preparado).
#[gpui_kit::test]
async fn the_unstaged_side_stays_selected_after_a_reread(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_both_sides();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:src/a.js");
    wait_for_diff(&app, &view, cx, "+working");
    // El watcher ve cambiar el archivo: el estado (`MM`) queda igual.
    repo.write("src/a.js", "working again\n");
    watcher_reread(&app, &repo_id, cx).await;
    wait_for_diff(&app, &view, cx, "+working again");
    assert_eq!(
        selected(&view, cx),
        Some(("src/a.js".to_string(), " M".to_string()))
    );
}

#[gpui_kit::test]
async fn reuses_the_tab_on_click_and_asks_for_a_new_one_on_double_click_or_from_the_menu(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.commit_file("src/b.js", "b\n", "Add b");
    repo.write("src/a.js", "A\n");
    repo.write("src/b.js", "B\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::working("src/a.js"));
    wait_for_diff(&app, &view, cx, "+A");

    app.click(cx, "unstaged:file:src/b.js");
    app.settle(cx, &view);
    // El clic reutilizó la pestaña, y la vista siguió a su pestaña.
    assert_eq!(
        tabs(&app, cx, &repo_id),
        [("src/b.js".to_string(), TabSource::Working)]
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.tab_id().map(str::to_string)),
        Some(FileTabDescriptor::working("src/b.js").id())
    );
    wait_for_diff(&app, &view, cx, "+B");

    app.double_click(cx, "unstaged:file:src/b.js");
    app.right_click(cx, "unstaged:file:src/b.js");
    // "Open here", "Open in new tab", separador, …
    app.click_menu_item(cx, 1);

    let events = opened(&app);
    assert_eq!(
        events.first(),
        Some(&("src/b.js".to_string(), TabSource::Working, true))
    );
    let new_tabs: Vec<_> = events.iter().filter(|(_, _, replace)| !replace).collect();
    assert_eq!(
        new_tabs,
        [
            &("src/b.js".to_string(), TabSource::Working, false),
            &("src/b.js".to_string(), TabSource::Working, false),
        ]
    );
}

#[gpui_kit::test]
async fn loads_a_tab_against_its_own_repository_and_index_even_when_another_repository_is_active(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = repo_with_both_sides();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::index("src/a.js"));
    let other = TestRepo::other("other");
    let other_id = app.open(&other, cx).await;
    assert_ne!(other_id, repo_id);

    wait_for_diff(&app, &view, cx, "+staged");
    assert!(!diff(&view, cx).contains("working"));
    // En una pestaña de archivo no hay caja de commit.
    assert!(!app.exists(cx, "commit"));
}

#[gpui_kit::test]
async fn keeps_an_empty_tab_in_its_original_context_instead_of_selecting_another_file(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.commit_file("src/b.js", "b\n", "Add b");
    repo.write("src/b.js", "B\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::working("src/a.js"));
    app.settle(cx, &view);

    assert_eq!(selected(&view, cx), None);
    assert_eq!(diff(&view, cx), "");
    assert_eq!(
        app.label(cx, "file-review-state").as_deref(),
        Some("No changes to display for this file in Local.")
    );
}

#[gpui_kit::test]
async fn loads_inactive_local_file_tabs_only_when_activated(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let path = app.repo_path(cx, &repo_id);
    let descriptor = FileTabDescriptor::working("src/a.js");
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &descriptor, false, cx);
        // La pestaña existe, pero se ve otra.
        model.activate_workspace_tab(&path, HOME_TAB, cx);
    });
    let view = app.mount(
        cx,
        &repo_id,
        ChangesMode::FileTab {
            tab_id: descriptor.id(),
        },
    );
    app.settle(cx, &view);
    assert_eq!(diff(&view, cx), "");

    app.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, &descriptor.id(), cx);
    });
    wait_for_diff(&app, &view, cx, "+A");
}

/// `set_active(false)` (el espacio de trabajo del repositorio no se ve) vale
/// igual que no ser la pestaña activa.
#[gpui_kit::test]
async fn a_view_the_shell_hides_waits_to_load(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::working("src/a.js"));
    wait_for_diff(&app, &view, cx, "+A");

    view.update(cx, |view, cx| view.set_active(false, cx));
    repo.write("src/a.js", "AA\n");
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                mergeon_app::SyncRequest::new()
                    .status(true)
                    .operation("status-watch"),
                cx,
            )
        })
        .await;
    app.settle(cx, &view);
    assert!(!diff(&view, cx).contains("+AA"));

    view.update(cx, |view, cx| view.set_active(true, cx));
    wait_for_diff(&app, &view, cx, "+AA");
}

// ---------------------------------------------------------------------------
// ChangesView
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn auto_selects_the_first_conflict_and_blocks_commit_outside_idle_lifecycle(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    app.wait_until(cx, "the resolver", |cx| {
        app.in_window(cx, |_, _| {});
        view.read_with(cx, |view, cx| {
            view.file_content().contains("<<<<<<<")
                && !view.conflict_resolver().read(cx).is_loading()
        })
    });

    assert_eq!(
        selected(&view, cx).map(|(path, _)| path).as_deref(),
        Some("notes.txt")
    );
    assert_eq!(
        app.label(cx, "resolver-file-name").as_deref(),
        Some("Resolve Conflicts: notes.txt")
    );
    assert_eq!(
        app.label(cx, "resolver-status").as_deref(),
        Some("0 of 1 resolved")
    );
    type_message(&app, cx, "Merge it");
    let head = repo.head();
    app.click(cx, "commit");
    app.in_window(cx, |window, cx| window.press("ctrl-enter", cx));
    app.settle(cx, &view);
    assert_eq!(repo.head(), head);
    assert!(app.events.borrow().is_empty());
}

#[gpui_kit::test]
async fn loads_the_file_diff_for_non_conflict_files(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);

    wait_for_diff(&app, &view, cx, "+A");
    assert_eq!(
        selected(&view, cx),
        Some(("src/a.js".to_string(), " M".to_string()))
    );
    assert!(app.exists(cx, "toggle-diff-view"));
}

#[gpui_kit::test]
async fn commits_pushes_when_requested_and_refreshes_refs_after_the_push(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    app.settle(cx, &view);

    type_message(&app, cx, "Ship it");
    app.click(cx, "push-after-commit");
    assert_eq!(
        app.label(cx, "commit-action").as_deref(),
        Some("Commit & Push 1 file")
    );
    assert_eq!(
        app.label(cx, "commit-target").as_deref(),
        Some("Committing to main")
    );
    app.click(cx, "commit");
    app.wait_until(cx, "the commit event", |_| {
        app.events
            .borrow()
            .iter()
            .any(|event| matches!(event, ChangesViewEvent::CommitCreated { .. }))
    });
    app.settle(cx, &view);

    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Ship it");
    assert_eq!(repo.origin_ref("main"), Some(repo.head()));
    assert!(app.host.has_message(
        NotifyType::Success,
        "Commit and push completed successfully."
    ));
    assert!(app.host.messages(NotifyType::Error).is_empty());
    assert!(app.host.messages(NotifyType::Info).is_empty());
    let created: Vec<bool> = app
        .events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            ChangesViewEvent::CommitCreated {
                sync_already_applied,
                ..
            } => Some(*sync_already_applied),
            _ => None,
        })
        .collect();
    assert_eq!(created, [true]);
    // Las ramas se releyeron tras el push: `main` ya no va por delante.
    let ahead = app.model.read_with(cx, |model, _| {
        model.active_branch_entry().map(|branch| branch.ahead)
    });
    assert_eq!(ahead, Some(0));
    assert_eq!(
        view.read_with(cx, |view, cx| view.commit_message(cx).to_string()),
        ""
    );
}

/// Sin push, las ramas también se releen (lo hacía `handleCommitCreated` de
/// `CommitView.vue`): el `ahead` de la rama sube.
#[gpui_kit::test]
async fn a_commit_rereads_the_branches(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let ahead = |app: &TestApp, cx: &mut TestAppContext| {
        app.model.read_with(cx, |model, _| {
            model.active_branch_entry().map(|branch| branch.ahead)
        })
    };
    assert_eq!(ahead(&app, cx), Some(0));
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    type_message(&app, cx, "One more");
    // El atajo, como en la app: ni salto de línea ni commit doble.
    app.in_window(cx, |window, cx| window.press("ctrl-enter", cx));
    app.wait_until(cx, "the branches to be reread", |cx| {
        ahead(&app, cx) == Some(1)
    });
    app.settle(cx, &view);

    assert_eq!(repo.git(&["log", "-1", "--format=%B"]), "One more");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
    assert!(app.host.messages(NotifyType::Error).is_empty());
}

#[gpui_kit::test]
async fn stages_the_selected_file_and_follows_it_to_the_staged_side(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+A");
    assert_eq!(
        selected(&view, cx),
        Some(("src/a.js".to_string(), " M".to_string()))
    );

    app.click(cx, "check:unstaged:file:src/a.js");
    app.wait_until(cx, "the file to be staged", |cx| {
        selected(&view, cx).is_some_and(|(_, status)| status == "M ")
    });
    app.settle(cx, &view);

    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "src/a.js");
    assert!(!app.exists(cx, "check:unstaged:file:src/a.js"));
    assert!(app.exists(cx, "check:staged:file:src/a.js"));
    // El diff es ahora el del índice.
    wait_for_diff(&app, &view, cx, "+A");
}

#[gpui_kit::test]
async fn keeps_the_created_commit_when_push_after_commit_fails(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    // El remoto deja de existir: el push fallará.
    std::fs::remove_dir_all(repo.origin_path()).expect("remove origin");
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    app.settle(cx, &view);

    type_message(&app, cx, "Ship it");
    app.click(cx, "push-after-commit");
    app.click(cx, "commit");
    app.wait_until(cx, "the push failure", |_| {
        app.host
            .has_message(NotifyType::Error, "Commit created, but push failed.")
    });
    app.settle(cx, &view);

    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Ship it");
    assert_eq!(app.host.messages(NotifyType::Error).len(), 1);
    assert!(app.host.messages(NotifyType::Success).is_empty());
    let created: Vec<bool> = app
        .events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            ChangesViewEvent::CommitCreated {
                sync_already_applied,
                ..
            } => Some(*sync_already_applied),
            _ => None,
        })
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(
        view.read_with(cx, |view, cx| view.commit_message(cx).to_string()),
        ""
    );
}

#[gpui_kit::test]
async fn disables_the_commit_form_while_a_commit_is_running(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    // Un hook que tarda: el commit queda en curso un rato.
    let hook = repo.path.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nsleep 2\n").expect("hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    app.settle(cx, &view);

    type_message(&app, cx, "Ship it");
    app.click_now(cx, "commit");

    assert_eq!(
        view.read_with(cx, |view, _| view.busy_action()),
        Some(BusyAction::Commit)
    );
    // Mientras dura, ni otro clic, ni el atajo, ni la casilla hacen nada.
    app.click_now(cx, "commit");
    app.press_now(cx, "ctrl-enter");
    app.click_now(cx, "push-after-commit");
    assert!(!view.read_with(cx, |view, _| view.is_push_after_commit()));

    app.wait_until(cx, "the commit to finish", |cx| {
        view.read_with(cx, |view, _| view.busy_action().is_none())
    });
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
}

#[gpui_kit::test]
async fn an_amend_starts_from_the_head_commit_message(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Reword the startup message");
    // Un stash más reciente que HEAD: no es lo que se enmienda (el original lo
    // tomaba por el último commit).
    repo.write("src/a.js", "stashed\n");
    repo.git(&["stash", "push", "-q", "-m", "WIP"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.wait_until(cx, "the amend checkbox", |cx| {
        app.exists(cx, "amend-last-commit")
    });

    app.click(cx, "amend-last-commit");

    assert!(view.read_with(cx, |view, _| view.is_amend()));
    assert_eq!(
        view.read_with(cx, |view, cx| view.commit_message(cx).to_string()),
        "Reword the startup message"
    );
    assert_eq!(app.label(cx, "commit-action").as_deref(), Some("Amend"));
    assert_eq!(
        app.label(cx, "commit-target").as_deref(),
        Some("Amending the last commit on main")
    );
    // Enmendar sin nada preparado vale: solo cambia el mensaje.
    assert!(view.read_with(cx, |view, cx| view.is_committable(cx)));
    app.click(cx, "commit");
    app.wait_until(cx, "the amend", |cx| {
        view.read_with(cx, |view, _| view.busy_action().is_none())
            && repo.git(&["log", "-1", "--format=%s"]) == "Reword the startup message"
            && app.events.borrow().len() == 1
    });
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
}

// ---------------------------------------------------------------------------
// Asunto, cuerpo, borrador y firma
// ---------------------------------------------------------------------------

fn commit_parts(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> (String, String) {
    view.read_with(cx, |view, cx| {
        (
            view.commit_title(cx).to_string(),
            view.commit_body(cx).to_string(),
        )
    })
}

#[gpui_kit::test]
async fn joins_subject_and_body_with_a_blank_line_when_committing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    type_message(&app, cx, "Add a");
    app.click(cx, "commit-body");
    app.in_window(cx, |window, cx| {
        window.input("Because it was missing.", cx);
        window.press("enter", cx);
        window.input("Second line.", cx);
    });
    app.click(cx, "commit");
    app.wait_until(cx, "the commit", |_| {
        app.events
            .borrow()
            .iter()
            .any(|event| matches!(event, ChangesViewEvent::CommitCreated { .. }))
    });
    app.settle(cx, &view);

    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]),
        "Add a\n\nBecause it was missing.\nSecond line."
    );
    assert_eq!(commit_parts(&view, cx), (String::new(), String::new()));
}

// El borrador se sigue guardando como una sola cadena: uno viejo de una sola
// caja se reparte (asunto hasta el primer salto, el resto al cuerpo) y lo que
// se escribe vuelve a guardarse unido.
#[gpui_kit::test]
async fn splits_an_old_single_box_draft_and_saves_it_joined(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("src/a.js", "a\n");
    let repo_id = app.open(&repo, cx).await;
    let path = app.repo_path(cx, &repo_id);
    app.model.update(cx, |model, cx| {
        model.set_commit_draft(&path, "Half a subject\nand a body\nin two lines", cx);
    });

    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    assert_eq!(
        commit_parts(&view, cx),
        (
            "Half a subject".to_string(),
            "and a body\nin two lines".to_string()
        )
    );

    app.click(cx, "commit-title");
    app.in_window(cx, |window, cx| {
        window.press("end", cx);
        window.input("!", cx);
    });
    let draft = app
        .model
        .read_with(cx, |model, _| model.commit_draft(&path));
    assert_eq!(draft, "Half a subject!\n\nand a body\nin two lines");

    // Al volver, el borrador nuevo sale igual.
    let again = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &again);
    assert_eq!(
        commit_parts(&again, cx),
        (
            "Half a subject!".to_string(),
            "and a body\nin two lines".to_string()
        )
    );
}

#[gpui_kit::test]
async fn an_amend_splits_the_head_message_into_subject_and_body(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    repo.git(&["commit", "-q", "-m", "Add a", "-m", "With a body."]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.wait_until(cx, "the amend checkbox", |cx| {
        app.exists(cx, "amend-last-commit")
    });

    app.click(cx, "amend-last-commit");

    assert_eq!(
        commit_parts(&view, cx),
        ("Add a".to_string(), "With a body.".to_string())
    );
}

// Sin clave con que firmar, el commit no se crea y el aviso dice qué falta.
#[gpui_kit::test]
async fn a_signed_commit_without_a_key_says_what_is_missing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let missing = repo.path.join("no-such-gpg");
    repo.git(&["config", "gpg.format", "openpgp"]);
    repo.git(&[
        "config",
        "gpg.program",
        missing.to_str().expect("utf-8 path"),
    ]);
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    let head = repo.head();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    type_message(&app, cx, "Signed");
    app.click(cx, "sign-commit");
    assert!(view.read_with(cx, |view, _| view.is_sign()));
    app.click(cx, "commit");
    app.wait_until(cx, "the signing error", |_| {
        !app.host.messages(NotifyType::Error).is_empty()
    });
    app.settle(cx, &view);

    let errors = app.host.messages(NotifyType::Error);
    // Lo que hay que hacer va primero: el aviso corta los textos largos.
    assert!(
        errors[0].starts_with(
            "Could not sign the commit: set a signing key (user.signingkey; gpg.format ssh for SSH keys) or turn off Sign."
        ),
        "{errors:?}"
    );
    assert_eq!(repo.head(), head);
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "src/a.js");
    // El mensaje sigue ahí para intentarlo de nuevo.
    assert_eq!(
        commit_parts(&view, cx),
        ("Signed".to_string(), String::new())
    );
}

// "Sign" se recuerda por repositorio.
#[gpui_kit::test]
async fn remembers_sign_for_each_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let other = TestRepo::other("other");
    let repo_id = app.open(&repo, cx).await;
    let other_id = app.open(&other, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "sign-commit");
    let path = app.repo_path(cx, &repo_id);
    assert!(app.model.read_with(cx, |model, _| model.commit_sign(&path)));

    let again = app.mount(cx, &repo_id, ChangesMode::List);
    assert!(again.read_with(cx, |view, _| view.is_sign()));
    let elsewhere = app.mount(cx, &other_id, ChangesMode::List);
    assert!(!elsewhere.read_with(cx, |view, _| view.is_sign()));
}

// ---------------------------------------------------------------------------
// Teclado de la lista
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn the_list_keys_stage_discard_and_move_the_diff_without_opening_tabs(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.commit_file("src/b.js", "b\n", "Add b");
    repo.write("src/a.js", "A\n");
    repo.write("src/b.js", "B\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    // Sin nada enfocado, la lista toma el foco: las teclas funcionan nada
    // más entrar.
    assert!(app.in_window(cx, |window, cx| {
        view.read(cx).list_focus_handle().is_focused(window)
    }));
    // Nada elegido al entrar: la primera flecha elige el primer archivo.
    let selected_path = |cx: &mut TestAppContext| selected(&view, cx).map(|(path, _)| path);
    assert_eq!(selected_path(cx), None);
    app.in_window(cx, |window, cx| window.press("down", cx));
    assert_eq!(selected_path(cx).as_deref(), Some("src/a.js"));
    app.in_window(cx, |window, cx| window.press("down", cx));
    assert_eq!(selected_path(cx).as_deref(), Some("src/b.js"));
    wait_for_diff(&app, &view, cx, "+B");
    app.in_window(cx, |window, cx| window.press("up", cx));
    assert_eq!(selected_path(cx).as_deref(), Some("src/a.js"));
    wait_for_diff(&app, &view, cx, "+A");
    assert!(opened(&app).is_empty());

    // El clic enseña el diff, no abre pestañas, y da el foco a la lista.
    app.click(cx, "unstaged:file:src/a.js");
    app.settle(cx, &view);
    assert!(opened(&app).is_empty());
    assert!(app.in_window(cx, |window, cx| {
        view.read(cx).list_focus_handle().is_focused(window)
    }));

    // Las flechas mueven la selección (y el diff) sin abrir pestañas.
    app.in_window(cx, |window, cx| window.press("down", cx));
    assert_eq!(
        selected(&view, cx).map(|(path, _)| path).as_deref(),
        Some("src/b.js")
    );
    wait_for_diff(&app, &view, cx, "+B");
    assert!(opened(&app).is_empty());

    // Espacio prepara el elegido.
    app.in_window(cx, |window, cx| window.press("space", cx));
    app.wait_until(cx, "src/b.js to be staged", |_| {
        repo.git(&["diff", "--cached", "--name-only"]) == "src/b.js"
    });
    app.settle(cx, &view);

    // D pregunta antes de descartar.
    app.click(cx, "unstaged:file:src/a.js");
    app.settle(cx, &view);
    app.host.answer_confirm(true);
    app.in_window(cx, |window, cx| window.press("d", cx));
    app.wait_until(cx, "the discard", |_| repo.read("src/a.js") == "a\n");
    app.settle(cx, &view);
    let confirms = app.host.confirms();
    assert_eq!(
        confirms
            .last()
            .and_then(|confirm| confirm.description.clone())
            .as_deref(),
        Some("Discard local changes in src/a.js?")
    );
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "src/b.js");
}

// ---------------------------------------------------------------------------
// repaints
// ---------------------------------------------------------------------------

fn renders(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> usize {
    view.read_with(cx, |view, _| view.render_count())
}

/// Deja terminar lo que quede en vuelo (cifras de la lista, el último commit,
/// "Fixup into…"): hasta que la vista lleve un rato sin pintarse.
fn quiet(view: &Entity<ChangesView>, cx: &mut TestAppContext) {
    let mut last = renders(view, cx);
    let mut still = 0;
    while still < 10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        let now = renders(view, cx);
        still = if now == last { still + 1 } else { 0 };
        last = now;
    }
}

/// Cada aviso del modelo repintaba la vista (y con ella la ventana): un
/// toast, el progreso de una operación, otro repositorio. Ahora solo lo hace
/// lo que cambia algo de lo que enseña.
#[gpui_kit::test]
async fn unrelated_model_notifications_do_not_repaint_the_view(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    // Otro repositorio abierto (y activo) junto al de la vista.
    let other = TestRepo::other("other");
    let other_id = app.open(&other, cx).await;
    app.settle(cx, &view);
    quiet(&view, cx);

    let before = renders(&view, cx);
    for index in 0..3 {
        app.model.update(cx, |model, cx| {
            model.notify_info(&format!("Something else happened {index}"), cx);
            cx.notify();
        });
        cx.run_until_parked();
    }
    // Cambios en el otro repositorio.
    other.write("b.txt", "b\n");
    watcher_reread(&app, &other_id, cx).await;
    cx.run_until_parked();
    assert!(
        app.model.read_with(cx, |model, _| model
            .repository(other_id.as_str())
            .and_then(|repository| repository.status.as_ref())
            .is_some_and(|status| !status.files.is_empty())),
        "the other repository saw its change"
    );
    assert_eq!(renders(&view, cx), before);

    // Un cambio de su repositorio sí la repinta.
    repo.write("src/c.js", "c\n");
    watcher_reread(&app, &repo_id, cx).await;
    let view_for_wait = view.clone();
    app.wait_until(cx, "the new file in the list", move |cx| {
        view_for_wait.read_with(cx, |view, _| view.files().len() == 2)
    });
    let after = renders(&view, cx);
    assert!(after > before, "{before} -> {after}");
}
