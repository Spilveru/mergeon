//! `CommitView.test.js`: la vista entera sobre repositorios git reales. Donde
//! el original sustituía `useGit` por dobles, aquí se prepara el repositorio
//! para que git responda lo mismo y se comprueba el efecto (rama actual,
//! commits, selección, ajustes guardados).

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, SharedString, TestAppContext, point, px};

use mergeon_backend::CommitOptions;
use mergeon_model::notify::NotifyType;
use mergeon_model::workspace_tabs::{CHANGES_TAB, HOME_TAB};
use mergeon_view_commits::{
    CommitCreated, CommitLoadOptions, CommitsView, WORKING_CHANGES_ROW_ID, file_row_id,
};
use mergeon_view_diff::DiffViewMode;

use crate::support::{TestApp, TestRepo, commit_menu_index};

/// Abre el repositorio y monta la vista; espera a que el historial y el
/// commit seleccionado estén cargados.
async fn open_view(
    app: &TestApp,
    repo: &TestRepo,
    workspace: bool,
    cx: &mut TestAppContext,
) -> (String, Entity<CommitsView>) {
    let repo_id = app.open(repo, cx).await;
    let id = repo_id.clone();
    let view = app.mount(cx, move |window, cx| {
        if workspace {
            CommitsView::new(id, window, cx)
        } else {
            CommitsView::for_active_repository(window, cx)
        }
    });
    wait_loaded(app, &view, cx);
    (repo_id, view)
}

fn wait_loaded(app: &TestApp, view: &Entity<CommitsView>, cx: &mut TestAppContext) {
    app.wait_for(cx, view, "the history to load", |view, _| {
        !view.commits().is_empty()
            && view.selected_commit().is_some()
            && !view.is_loading_history()
            && !view.is_loading_details()
    });
}

fn hash_of(view: &Entity<CommitsView>, message: &str, cx: &mut TestAppContext) -> String {
    view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .find(|commit| commit.message == message)
            .map(|commit| commit.full_hash.clone())
            .unwrap_or_else(|| panic!("no commit `{message}` in the history"))
    })
}

fn row_id(hash: &str) -> SharedString {
    SharedString::from(format!("commit:{hash}"))
}

fn selected_message(view: &Entity<CommitsView>, cx: &mut TestAppContext) -> Option<String> {
    view.read_with(cx, |view, _| {
        view.selected_commit().map(|commit| commit.message.clone())
    })
}

fn last_operation(app: &TestApp, cx: &mut TestAppContext) -> Option<String> {
    app.model.read_with(cx, |model, _| {
        model
            .active_repository()
            .and_then(|repo| repo.last_mutation_operation.clone())
    })
}

fn open_row_menu(
    app: &TestApp,
    view: &Entity<CommitsView>,
    message: &str,
    cx: &mut TestAppContext,
) {
    let hash = hash_of(view, message, cx);
    app.in_window(cx, |window, cx| window.right_click(row_id(&hash), cx));
    assert!(app.menu_is_open(cx), "the commit menu opened");
}

fn double_click_row(
    app: &TestApp,
    view: &Entity<CommitsView>,
    message: &str,
    cx: &mut TestAppContext,
) {
    let hash = hash_of(view, message, cx);
    app.in_window(cx, |window, cx| window.double_click(row_id(&hash), cx));
}

/// Dos commits en `main`: uno viejo sin refs y la punta.
fn two_commit_repo() -> TestRepo {
    let repo = TestRepo::empty();
    repo.commit_file("older.txt", "old\n", "older commit");
    repo.commit_file("current.txt", "current\n", "current branch commit");
    repo
}

// En el espacio de trabajo la lista de archivos no tiene un diff al lado (un
// clic abre la pestaña del archivo): antes se marcaba el primero igual.
#[gpui_kit::test]
async fn in_the_workspace_no_changed_file_is_selected_until_one_is_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_, view) = open_view(&app, &two_commit_repo(), true, cx).await;
    assert_eq!(
        view.read_with(cx, |view, _| view
            .selected_file()
            .map(|file| file.path.clone())),
        None
    );

    // Fuera del espacio de trabajo sí: es el diff que se ve a la derecha.
    let app = TestApp::new(cx);
    let (_, view) = open_view(&app, &two_commit_repo(), false, cx).await;
    assert_eq!(
        view.read_with(cx, |view, _| view
            .selected_file()
            .map(|file| file.path.clone())),
        Some("current.txt".to_string())
    );
}

/// La primera apertura selecciona HEAD aunque arriba haya un commit más nuevo
/// de otra rama. (Electron elegía la primera fila porque el repo recién
/// agregado aún no sabía su rama al cargar el historial.)
#[gpui_kit::test]
async fn first_open_selects_head_even_when_another_branch_is_newer(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    repo.git(&["checkout", "-q", "-b", "newer"]);
    repo.commit_file("newer.txt", "newer\n", "newer branch commit");
    repo.git(&["checkout", "-q", "main"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    assert_eq!(
        selected_message(&view, cx).as_deref(),
        Some("current branch commit")
    );
}

/// Un stash queda encima de HEAD en el historial: abrir el repo no lo elige.
#[gpui_kit::test]
async fn first_open_does_not_select_a_stash_above_head(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    repo.write("current.txt", "work in progress\n");
    repo.git(&["stash", "push", "-q", "-m", "wip"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    assert_eq!(
        selected_message(&view, cx).as_deref(),
        Some("current branch commit")
    );
}

#[gpui_kit::test]
async fn loads_history_selects_the_current_branch_commit_and_checks_out_a_detached_commit(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let older = hash_of(&view, "older commit", cx);

    assert_eq!(
        selected_message(&view, cx).as_deref(),
        Some("current branch commit")
    );
    let baseline = view.read_with(cx, |view, _| view.history_load_count());

    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Checkout"));

    // Un commit sin refs solo admite HEAD desanclado: el diálogo lo dice y el
    // botón confirma exactamente eso.
    app.wait_for_dialog(cx, "Checkout commit");
    let request = view
        .read_with(cx, |view, _| view.checkout_dialog().cloned())
        .expect("a pending checkout");
    assert!(request.is_detached_only());
    assert_eq!(request.title(), "Checkout commit");
    assert_eq!(request.targets[0].confirm_label, "Checkout commit");
    app.click(cx, "checkout-submit");

    app.wait_for(cx, &view, "the detached checkout", |view, _| {
        view.current_branch() == "HEAD" && !view.is_loading_history()
    });
    assert_eq!(repo.head(), older);
    assert!(view.read_with(cx, |view, _| view.history_load_count()) > baseline);
    assert_eq!(last_operation(&app, cx).as_deref(), Some("checkout"));
    // La selección se queda en el commit al que se hizo checkout.
    assert_eq!(selected_message(&view, cx).as_deref(), Some("older commit"));
}

/// Doble clic sobre la punta de una rama local con el working tree limpio:
/// no hay nada que preguntar, se cambia de rama y punto.
#[gpui_kit::test]
async fn switches_straight_to_the_branch_when_the_commit_is_the_tip_of_a_clean_local_branch(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    repo.commit_file("a.txt", "a\n", "feature tip");
    repo.git(&["branch", "feature"]);
    repo.commit_file("b.txt", "b\n", "current branch commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    double_click_row(&app, &view, "feature tip", cx);

    app.wait_until(cx, "the switch to feature", |_, _| {
        repo.current_branch() == "feature"
    });
    // Sin diálogo de por medio.
    assert!(app.dialogs(cx).is_empty());
}

/// La misma rama pero con cambios sin commitear: ahí sí hay algo que decidir.
#[gpui_kit::test]
async fn asks_what_to_do_with_local_changes_before_switching_to_the_branch(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    repo.commit_file("shared.txt", "shared\n", "feature tip");
    repo.git(&["branch", "feature"]);
    repo.commit_file("b.txt", "b\n", "current branch commit");
    repo.write("shared.txt", "edited\n");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    double_click_row(&app, &view, "feature tip", cx);

    // El diálogo va solo de los cambios locales: el destino es la rama, y el
    // HEAD desanclado ni se ofrece.
    app.wait_for_dialog(cx, "Switch Branch");
    assert_eq!(repo.current_branch(), "main");
    let request = view
        .read_with(cx, |view, _| view.checkout_dialog().cloned())
        .expect("a pending checkout");
    assert!(request.has_local_changes);
    assert_eq!(request.targets.len(), 1);
    assert_eq!(request.targets[0].value, "branch:feature");
    assert_eq!(request.targets[0].confirm_label, "Switch branch");

    app.click(cx, "checkout-submit");

    app.wait_until(cx, "the switch to feature", |_, _| {
        repo.current_branch() == "feature"
    });
    // Se mantuvieron los cambios.
    assert_eq!(
        std::fs::read_to_string(repo.path.join("shared.txt")).expect("file"),
        "edited\n"
    );
}

/// Solo con rama remota encima: se ofrece crear la local que la trackea.
#[gpui_kit::test]
async fn offers_creating_a_tracking_branch_when_the_commit_only_carries_a_remote_ref(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("f.txt", "f\n", "upstream tip");
    repo.git(&["push", "-q", "origin", "feature"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["branch", "-q", "-D", "feature"]);
    repo.commit_file("m.txt", "m\n", "current branch commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    double_click_row(&app, &view, "upstream tip", cx);

    // El mismo diálogo de tracking que sale desde el sidebar.
    app.wait_for_dialog(cx, "Track Remote Branch");
    let request = view
        .read_with(cx, |view, _| view.checkout_dialog().cloned())
        .expect("a pending checkout");
    assert_eq!(request.remote_branch(), "origin/feature");
    assert!(!request.is_detached_only());
    assert_eq!(request.targets[0].confirm_label, "Create and Checkout");

    app.click(cx, "checkout-submit");

    app.wait_until(cx, "the tracking branch", |_, _| {
        repo.current_branch() == "feature"
    });
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"]),
        "origin/feature"
    );
}

/// Tu rama local existe pero está atrasada: en el commit de origin/x lo que
/// quieres es ir a tu rama.
#[gpui_kit::test]
async fn offers_the_existing_local_branch_when_it_is_behind_its_remote_counterpart(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.git(&["branch", "feature"]);
    repo.git(&["checkout", "-q", "-b", "ahead"]);
    repo.commit_file("f.txt", "f\n", "upstream tip");
    repo.git(&["push", "-q", "origin", "ahead:feature"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["branch", "-q", "-D", "ahead"]);
    repo.commit_file("m.txt", "m\n", "current branch commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    double_click_row(&app, &view, "upstream tip", cx);

    // Va a la rama local, sin ofrecer tracking ni HEAD desanclado.
    app.wait_until(cx, "the switch to feature", |_, _| {
        repo.current_branch() == "feature"
    });
    assert!(app.dialogs(cx).is_empty());
}

/// "Load more": la página siguiente se acumula sin duplicar y sin tocar la
/// selección.
#[gpui_kit::test]
async fn appends_the_next_page_of_history_without_duplicates_when_load_more_fires(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.many_commits(204, "commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    let (count, has_more) = view.read_with(cx, |view, _| {
        (view.commits().len(), view.has_more_commits())
    });
    assert_eq!(count, 200);
    assert!(has_more);
    let selected = selected_message(&view, cx);

    // Hasta el final de la lista: el centinela pide la página siguiente.
    let list = view.read_with(cx, |view, _| view.history_list().clone());
    list.update(cx, |list, _| {
        list.set_scroll_top(200. * mergeon_view_commits::ROW_HEIGHT)
    });
    app.in_window(cx, |_, _| {});

    app.wait_for(cx, &view, "the second page", |view, _| {
        view.commits().len() == 205 && !view.is_loading_more_commits()
    });
    let mut hashes: Vec<String> = view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .map(|commit| commit.full_hash.clone())
            .collect()
    });
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), 205);
    assert!(!view.read_with(cx, |view, _| view.has_more_commits()));
    // La selección no se movió.
    assert_eq!(selected_message(&view, cx), selected);
}

/// Un salto pedido desde el sidebar puede apuntar a un commit más viejo que
/// la ventana normal de 200: se reintenta una vez con 1000.
#[gpui_kit::test]
async fn retries_with_a_bigger_window_when_the_requested_commit_is_outside_the_loaded_history(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.many_commits(600, "commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;
    assert_eq!(view.read_with(cx, |view, _| view.commits().len()), 200);

    let old = repo.rev("HEAD~450");
    view.update(cx, |view, cx| view.request_commit_selection(&old, cx));

    app.wait_for(cx, &view, "the old commit to be selected", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.full_hash == old)
    });
    assert_eq!(view.read_with(cx, |view, _| view.commits().len()), 601);
}

fn split_repo() -> TestRepo {
    let repo = TestRepo::empty();
    repo.commit_file("src/main.js", "const a = 1\n", "base");
    repo.commit_file("src/main.js", "const a = 2\n", "current branch commit");
    repo
}

/// Unified | Split ya no está en el detalle del commit (no cambiaba nada a la
/// vista: el diff se abre en su pestaña, que tiene su propio Split). La
/// preferencia sigue mandando sobre el visor.
#[gpui_kit::test]
async fn the_diff_follows_the_split_preference_without_a_toggle_in_the_inspector(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = split_repo();
    let (_, view) = open_view(&app, &repo, false, cx).await;
    app.wait_for(cx, &view, "the diff", |view, _| {
        !view.file_diff().is_empty() && !view.is_loading_diff()
    });
    assert!(!app.exists(cx, "tab-split"));
    assert!(!app.exists(cx, "tab-unified"));

    let viewer = view.read_with(cx, |view, cx| view.changes_tab().read(cx).viewer().clone());
    app.model
        .update(cx, |model, cx| model.toggle_diff_view(cx))
        .detach();
    app.wait_for(cx, &viewer, "the split view", |viewer, _| {
        viewer.view_mode() == DiffViewMode::Split
    });
}

/// La app pide `git log --all`: el historial trae commits de otras ramas y de
/// lo fetcheado. Sin marcarlos, todos parecen igual de tuyos.
#[gpui_kit::test]
async fn dims_what_is_not_in_your_branch_and_flags_what_you_have_yet_to_pull(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("f.txt", "f\n", "from another branch");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("m.txt", "m\n", "current branch commit");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.colleague_pushes("waiting on the remote");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    let list = view.read_with(cx, |view, _| view.history_list().clone());
    let flags = |message: &str, cx: &mut TestAppContext| {
        list.read_with(cx, |list, _| {
            let commit = list
                .commits()
                .iter()
                .find(|commit| commit.message == message)
                .expect("commit")
                .clone();
            list.row_flags(&commit)
        })
    };

    assert!(!flags("current branch commit", cx).not_in_branch);
    assert!(!flags("Initial commit", cx).not_in_branch);
    assert!(flags("from another branch", cx).not_in_branch);
    assert!(!flags("from another branch", cx).to_pull);
    // Lo que falta por pullear también se apaga, pero además se marca.
    let pending = flags("waiting on the remote", cx);
    assert!(pending.not_in_branch && pending.to_pull);
    assert!(app.exists(cx, "pull-marker"));
}

#[gpui_kit::test]
async fn asks_git_to_ignore_whitespace_when_that_is_switched_on(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    repo.commit_file("src/main.js", "if (a) {\ncall()\n}\n", "base");
    repo.commit_file(
        "src/main.js",
        "if (a) {\n    call()\n}\n",
        "current branch commit",
    );
    let (_, view) = open_view(&app, &repo, false, cx).await;
    app.wait_for(cx, &view, "the diff", |view, _| {
        !view.file_diff().is_empty()
    });
    assert!(view.read_with(cx, |view, _| view.file_diff().contains("+    call()")));

    app.click(cx, "toggle-ignore-whitespace");

    // Con `-w` el reindentado deja de ser un cambio.
    app.wait_for(cx, &view, "the diff without whitespace", |view, _| {
        !view.is_loading_diff() && !view.file_diff().contains("+    call()")
    });
    assert!(
        app.model
            .read_with(cx, |model, _| model.diff_preferences().ignore_whitespace)
    );
    // La preferencia se guardó en los ajustes.
    let settings = app
        .model
        .update(cx, |model, cx| model.load_settings(cx))
        .await;
    assert!(settings.commit_view.ignore_whitespace);
}

#[gpui_kit::test]
async fn uses_the_rich_create_branch_dialog_for_commit_context_actions(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let older = hash_of(&view, "older commit", cx);

    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Create Branch"));

    app.wait_for_dialog(cx, "Create Branch");
    let request = view
        .read_with(cx, |view, _| view.create_branch_dialog().cloned())
        .expect("a pending create branch");
    assert_eq!(request.base_label, repo.short(&older));
    assert_eq!(request.base_reference, older);

    app.in_window(cx, |window, cx| window.input("feature-from-commit", cx));
    app.click(cx, "create-branch-submit");

    app.wait_until(cx, "the new branch", |_, _| {
        repo.current_branch() == "feature-from-commit"
    });
    assert_eq!(repo.rev("feature-from-commit"), older);
    app.wait_until(cx, "the mutation to be recorded", |app, cx| {
        last_operation(app, cx).as_deref() == Some("create-branch")
    });
}

#[gpui_kit::test]
async fn reloads_history_when_the_active_repository_records_an_external_head_change(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let (repo_id, view) = open_view(&app, &repo, true, cx).await;
    let baseline = view.read_with(cx, |view, _| view.history_load_count());

    repo.commit_file("x.txt", "x\n", "made in a terminal");
    app.model.update(cx, |model, cx| {
        model.record_mutation(repo_id.as_str(), Some("external-head-change"), cx)
    });

    app.wait_for(cx, &view, "the reload", |view, _| {
        view.history_load_count() == baseline + 1
            && view
                .commits()
                .iter()
                .any(|commit| commit.message == "made in a terminal")
    });
}

fn store_with_history_width(width: f64) -> mergeon_backend::Store {
    let store = mergeon_app::testing::TestBackend::memory_store();
    store
        .update_settings_json(&serde_json::json!({ "commitView": { "historyWidth": width } }))
        .expect("settings");
    store
}

#[gpui_kit::test]
async fn applies_the_stored_history_width_on_mount(cx: &mut TestAppContext) {
    let app = TestApp::with_store(1000., store_with_history_width(560.), cx);
    let repo = TestRepo::new();
    let (_, view) = open_view(&app, &repo, true, cx).await;

    app.wait_for(cx, &view, "the stored width", |view, _| {
        view.history_width() == 560.
    });
}

#[gpui_kit::test]
async fn re_clamps_an_oversized_stored_history_width_for_smaller_containers(
    cx: &mut TestAppContext,
) {
    let app = TestApp::with_store(700., store_with_history_width(900.), cx);
    let repo = TestRepo::new();
    let (_, view) = open_view(&app, &repo, true, cx).await;

    // 700 - 320 del inspector - 8 del divisor.
    app.wait_for(cx, &view, "the clamped width", |view, _| {
        view.history_width() == 372.
    });
}

#[gpui_kit::test]
async fn persists_the_resized_history_width_when_dragging_stops(cx: &mut TestAppContext) {
    let app = TestApp::with_width(1000., cx);
    let repo = TestRepo::new();
    let (_, view) = open_view(&app, &repo, true, cx).await;

    let from = app.in_window(cx, |window, _| {
        window.find("commit-resizer").bounds().center()
    });
    app.in_window(cx, |window, cx| {
        window.drag(from, point(px(560.), from.y), cx);
    });

    app.wait_for(cx, &view, "the dragged width", |view, _| {
        view.history_width() == 560. && !view.is_resizing_history()
    });
    app.wait_until(cx, "the width to be saved", |app, cx| {
        app.model.read_with(cx, |model, _| model.history_width()) == Some(560.)
    });
    let settings = app
        .model
        .update(cx, |model, cx| model.load_settings(cx))
        .await;
    assert_eq!(settings.commit_view.history_width, Some(560.));
}

#[gpui_kit::test]
async fn handles_commit_created_payloads_without_a_global_refresh(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let (repo_id, view) = open_view(&app, &repo, true, cx).await;

    repo.write("new.txt", "new\n");
    repo.git(&["add", "new.txt"]);
    let path = repo.path_string();
    let result = app
        .model
        .update(cx, |model, cx| {
            model.commit(
                &path,
                "Created from Local Changes",
                CommitOptions::default(),
                true,
                cx,
            )
        })
        .await;
    assert!(result.success());

    view.update(cx, |view, cx| {
        view.handle_commit_created(Some(CommitCreated::new(result)), cx)
    })
    .await;

    assert_eq!(last_operation(&app, cx).as_deref(), Some("commit"));
    assert!(view.read_with(cx, |view, _| {
        view.commits()
            .first()
            .is_some_and(|commit| commit.message == "Created from Local Changes")
    }));
    let _ = repo_id;
}

/// Un commit hecho en la pestaña Local Changes mientras la de Commits está
/// oculta: al volver a ella el historial ya lo trae.
#[gpui_kit::test]
async fn catches_up_with_a_commit_made_while_the_history_was_hidden(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let (repo_id, view) = open_view(&app, &repo, true, cx).await;
    let path = repo.path_string();
    app.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, CHANGES_TAB, cx)
    });
    assert!(!view.read_with(cx, |view, cx| view.is_active(cx)));

    // Lo que hace la vista de cambios: commit, aplicar el resultado y releer.
    repo.write("new.txt", "new\n");
    repo.git(&["add", "new.txt"]);
    let result = app
        .model
        .update(cx, |model, cx| {
            model.commit(
                &path,
                "Committed from Local Changes",
                CommitOptions::default(),
                true,
                cx,
            )
        })
        .await;
    app.model.update(cx, |model, cx| {
        model.apply_repository_operation_result(repo_id.as_str(), &result, cx)
    });
    app.settle(cx, 100);
    assert!(view.read_with(cx, |view, _| {
        !view
            .commits()
            .iter()
            .any(|commit| commit.message == "Committed from Local Changes")
    }));

    app.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, HOME_TAB, cx)
    });

    app.wait_for(cx, &view, "the new commit in the history", |view, _| {
        view.commits()
            .first()
            .is_some_and(|commit| commit.message == "Committed from Local Changes")
    });
    // La badge de la rama se mueve con él.
    let head_refs = view.read_with(cx, |view, _| {
        view.commits()[0]
            .refs
            .iter()
            .map(|commit_ref| commit_ref.name.clone())
            .collect::<Vec<_>>()
    });
    assert!(head_refs.contains(&"main".to_string()));
}

#[gpui_kit::test]
async fn coalesces_repeated_history_reload_requests_while_commits_are_still_loading(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let baseline = view.read_with(cx, |view, _| view.history_load_count());

    let load = view.update(cx, |view, cx| {
        let first = view.load_commits(CommitLoadOptions::new().preferred_branch("main"), cx);
        // Mientras la primera sigue en vuelo, las demás se funden en una.
        drop(view.load_commits(CommitLoadOptions::new().force_details_reload(true), cx));
        drop(view.load_commits(CommitLoadOptions::new().force_details_reload(true), cx));
        first
    });
    load.await;

    assert_eq!(
        view.read_with(cx, |view, _| view.history_load_count()),
        baseline + 2
    );
}

/// Un fetch no debe rehacer la lista: las filas siguen en pantalla mientras
/// llega la recarga, sin pasar por "Loading commits...".
#[gpui_kit::test]
async fn keeps_the_rendered_history_on_screen_while_a_fetch_refresh_is_in_flight(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let (repo_id, view) = open_view(&app, &repo, true, cx).await;
    let list = view.read_with(cx, |view, _| view.history_list().clone());

    app.model.update(cx, |model, cx| {
        model.record_mutation(repo_id.as_str(), Some("fetch"), cx)
    });

    assert!(view.read_with(cx, |view, _| view.is_loading_history()));
    assert!(!view.read_with(cx, |view, _| view.is_loading_initial_history()));
    assert_eq!(list.read_with(cx, |list, _| list.status_text()), None);
    assert_eq!(list.read_with(cx, |list, _| list.commits().len()), 1);
    wait_loaded(&app, &view, cx);
    assert_eq!(list.read_with(cx, |list, _| list.commits().len()), 1);
}

/// Con varias páginas cargadas, refrescar no encoge la lista.
#[gpui_kit::test]
async fn refreshes_the_whole_loaded_window_instead_of_collapsing_back_to_one_page(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.many_commits(204, "commit");
    let (repo_id, view) = open_view(&app, &repo, true, cx).await;
    view.update(cx, |view, cx| view.handle_load_more_commits(cx));
    app.wait_for(cx, &view, "the second page", |view, _| {
        view.commits().len() == 205
    });

    app.model.update(cx, |model, cx| {
        model.record_mutation(repo_id.as_str(), Some("pull"), cx)
    });
    app.wait_for(cx, &view, "the refresh", |view, _| {
        !view.is_loading_history()
    });

    assert_eq!(view.read_with(cx, |view, _| view.commits().len()), 205);
}

#[gpui_kit::test]
async fn reloads_history_when_merge_or_rebase_mutations_arrive_from_outside_the_commit_view(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let (repo_id, view) = open_view(&app, &repo, true, cx).await;
    let baseline = view.read_with(cx, |view, _| view.history_load_count());

    app.model.update(cx, |model, cx| {
        model.record_mutation(repo_id.as_str(), Some("merge"), cx)
    });
    app.wait_for(cx, &view, "the merge reload", |view, _| {
        view.history_load_count() == baseline + 1 && !view.is_loading_history()
    });

    app.model.update(cx, |model, cx| {
        model.record_mutation(repo_id.as_str(), Some("rebase"), cx)
    });
    app.wait_for(cx, &view, "the rebase reload", |view, _| {
        view.history_load_count() == baseline + 2 && !view.is_loading_history()
    });
}

#[gpui_kit::test]
async fn reconciles_selection_after_reset_so_details_do_not_point_to_a_removed_commit(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    repo.commit_file("src/reset.js", "a\n", "reset target");
    repo.commit_file("src/new.js", "b\n", "newest commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let target = hash_of(&view, "reset target", cx);
    assert_eq!(
        selected_message(&view, cx).as_deref(),
        Some("newest commit")
    );

    app.host.answer_confirm(true);
    open_row_menu(&app, &view, "reset target", cx);
    app.click_menu_item(cx, commit_menu_index("Reset (Hard)"));

    app.wait_until(cx, "the reset", |_, _| repo.head() == target);
    app.wait_for(cx, &view, "the reconciled selection", |view, _| {
        !view.is_loading_history()
            && view.commits().len() == 1
            && view
                .selected_commit()
                .is_some_and(|commit| commit.message == "reset target")
    });
    let confirms = app.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Confirm reset"));
    assert!(
        confirms[0]
            .description
            .as_deref()
            .is_some_and(|text| text.ends_with("using hard mode?"))
    );
    assert_eq!(last_operation(&app, cx).as_deref(), Some("reset"));
}

#[gpui_kit::test]
async fn refreshes_branches_when_cherry_picking_from_the_commit_context_menu(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("feature.txt", "f\n", "older commit");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("main.txt", "m\n", "main moved on");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Cherry-pick"));

    app.wait_until(cx, "the cherry-pick", |_, _| {
        repo.git(&["log", "-1", "--format=%s", "main"]) == "older commit"
    });
    app.wait_for(cx, &view, "the reloaded history", |view, _| {
        view.commits()
            .iter()
            .filter(|commit| commit.message == "older commit")
            .count()
            == 2
    });
    app.wait_until(cx, "the mutation", |app, cx| {
        last_operation(app, cx).as_deref() == Some("cherry-pick")
    });
}

/// Con el menú abierto, la letra de una entrada la ejecuta (C = cherry-pick).
#[gpui_kit::test]
async fn cherry_picks_with_its_letter_while_the_commit_menu_is_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("feature.txt", "f\n", "older commit");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("main.txt", "m\n", "main moved on");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    open_row_menu(&app, &view, "older commit", cx);
    app.in_window(cx, |window, cx| window.within("popup-menu").press("c", cx));

    assert!(!app.menu_is_open(cx), "the letter closes the menu");
    app.wait_until(cx, "the cherry-pick", |_, _| {
        repo.git(&["log", "-1", "--format=%s", "main"]) == "older commit"
    });
}

#[gpui_kit::test]
async fn keeps_the_view_usable_when_cherry_pick_stops_on_conflicts(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("file.txt", "base\n", "base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("file.txt", "feature\n", "older commit");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("file.txt", "main\n", "main change");
    let (_, view) = open_view(&app, &repo, true, cx).await;

    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Cherry-pick"));

    app.wait_until(cx, "the conflicted cherry-pick", |app, cx| {
        last_operation(app, cx).as_deref() == Some("cherry-pick")
    });
    assert!(repo.path.join(".git/CHERRY_PICK_HEAD").exists());
    wait_loaded(&app, &view, cx);
    assert!(view.read_with(cx, |view, _| view.selected_commit().is_some()));
}

#[gpui_kit::test]
async fn reloads_history_and_focuses_a_requested_commit_from_sidebar_search(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    repo.commit_file("src/app.js", "a\n", "search result commit");
    repo.commit_file("src/other.js", "b\n", "current branch commit");
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let wanted = hash_of(&view, "search result commit", cx);
    let baseline = view.read_with(cx, |view, _| view.history_load_count());

    view.update(cx, |view, cx| view.request_commit_selection(&wanted, cx));

    app.wait_for(
        cx,
        &view,
        "the requested commit and its files",
        |view, _| {
            view.selected_commit()
                .is_some_and(|commit| commit.full_hash == wanted)
                && view
                    .commit_files()
                    .iter()
                    .any(|file| file.path == "src/app.js")
        },
    );
    assert!(view.read_with(cx, |view, _| view.history_load_count()) > baseline);
}

#[gpui_kit::test]
async fn discards_an_in_flight_history_load_when_the_active_repository_switches_mid_load(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let first = TestRepo::named("demo");
    first.commit_file("a.txt", "a\n", "repo one commit");
    let second = TestRepo::named("other");
    second.commit_file("b.txt", "b\n", "repo two commit");
    app.open(&second, cx).await;
    app.open(&first, cx).await;

    let view = app.mount(cx, |window, cx| {
        CommitsView::for_active_repository(window, cx)
    });
    // Cambia el repo activo mientras la carga del primero sigue en vuelo.
    let second_path = second.path_string();
    app.model.update(cx, |model, cx| {
        let index = model.find_repository_index(second_path.as_str());
        model.set_active_repository(index, cx);
    });

    wait_loaded(&app, &view, cx);
    app.wait_for(cx, &view, "the second repository", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "repo two commit")
    });
    // Los detalles nunca se pidieron con el hash del repo viejo.
    assert!(view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .all(|commit| commit.message != "repo one commit")
            && view.commit_files().iter().all(|file| file.path == "b.txt")
    }));
}

#[gpui_kit::test]
async fn recovers_once_from_a_stale_commit_hash_and_does_not_loop_on_repeated_failures(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "-b", "doomed"]);
    repo.commit_file("gone.txt", "gone\n", "rewritten elsewhere");
    repo.git(&["checkout", "-q", "main"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let doomed = view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .find(|commit| commit.message == "rewritten elsewhere")
            .cloned()
            .expect("the doomed commit is listed")
    });
    // Al abrir queda elegida la primera fila, que es la condenada: se parte
    // de otro commit para que elegirla cargue su detalle.
    let initial = view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .find(|commit| commit.message == "Initial commit")
            .cloned()
            .expect("the initial commit is listed")
    });
    view.update(cx, |view, cx| view.handle_history_select(initial, cx));
    app.wait_for(cx, &view, "the initial commit", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "Initial commit")
    });

    // La historia se reescribe en una terminal: el objeto deja de existir.
    repo.git(&["branch", "-q", "-D", "doomed"]);
    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "-q", "--prune=now"]);
    let baseline = view.read_with(cx, |view, _| view.history_load_count());

    view.update(cx, |view, cx| view.handle_history_select(doomed, cx));

    // Exactamente UNA recarga de recuperación, y la selección vuelve a un
    // commit vivo.
    app.wait_for(cx, &view, "the recovery reload", |view, _| {
        view.history_load_count() == baseline + 1
            && !view.is_loading_history()
            && view
                .selected_commit()
                .is_some_and(|commit| commit.message == "Initial commit")
    });
    app.settle(cx, 300);
    assert_eq!(
        view.read_with(cx, |view, _| view.history_load_count()),
        baseline + 1
    );
}

/// Las flechas mueven la selección al commit vecino.
#[gpui_kit::test]
async fn moves_the_selection_with_the_arrow_keys(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    assert_eq!(
        selected_message(&view, cx).as_deref(),
        Some("current branch commit")
    );

    app.in_window(cx, |window, cx| window.press("down", cx));
    app.wait_for(cx, &view, "the next commit", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "older commit")
    });
    // En el último no hay a dónde bajar.
    app.in_window(cx, |window, cx| window.press("down", cx));
    assert_eq!(selected_message(&view, cx).as_deref(), Some("older commit"));
    app.in_window(cx, |window, cx| window.press("up", cx));
    app.wait_for(cx, &view, "the previous commit", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "current branch commit")
    });
}

/// El mensaje desplegado es el del commit en el que se desplegó: al pasar a
/// otro vuelve a verse recortado.
#[gpui_kit::test]
async fn forgets_the_opened_message_when_another_commit_is_selected(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_, view) = open_view(&app, &two_commit_repo(), true, cx).await;
    view.update(cx, |view, cx| view.toggle_summary(cx));
    assert!(view.read_with(cx, |view, _| view.is_summary_expanded()));

    app.in_window(cx, |window, cx| window.press("down", cx));
    app.wait_for(cx, &view, "the next commit", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "older commit")
    });

    assert!(!view.read_with(cx, |view, _| view.is_summary_expanded()));
}

/// Con un stash guardado, `git log --all` pone su merge encima de HEAD: el
/// rango del rebase sigue los padres desde HEAD y no lo cuenta.
#[gpui_kit::test]
async fn interactive_rebase_ignores_a_saved_stash_and_rewrites_the_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "Bump version");
    repo.commit_file("b.txt", "b\n", "Add users route");
    repo.write("a.txt", "wip\n");
    repo.git(&["stash", "push", "-q", "-m", "experiment"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;
    assert!(view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .any(|commit| commit.message.contains("experiment"))
    }));

    open_row_menu(&app, &view, "Bump version", cx);
    app.click_menu_item(cx, commit_menu_index("Interactive rebase from here…"));

    app.wait_for_dialog(cx, "Interactive rebase");
    assert!(
        app.host
            .messages(mergeon_model::notify::NotifyType::Warning)
            .is_empty()
    );
    let dialog = view
        .read_with(cx, |view, _| view.interactive_rebase_dialog().cloned())
        .expect("the rebase dialog");
    let rows: Vec<String> = dialog.read_with(cx, |dialog, _| {
        dialog
            .rows()
            .iter()
            .map(|row| row.original_message.clone())
            .collect()
    });
    // Del más viejo al más nuevo, sin el stash.
    assert_eq!(rows, ["Bump version", "Add users route"]);

    // Reword del segundo y a correr.
    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.set_action(1, mergeon_view_commits::RebaseAction::Reword, window, cx);
            dialog.set_message(1, "Add the users route", window, cx);
        });
    });
    app.click(cx, "rebase-submit");

    app.wait_until(cx, "the rewritten branch", |_, _| {
        repo.git(&["log", "-1", "--format=%s"]) == "Add the users route"
    });
    app.wait_until(cx, "the dialog to close", |app, cx| {
        app.dialogs(cx).is_empty()
    });
    app.wait_for(cx, &view, "the reloaded history", |view, _| {
        view.interactive_rebase_dialog().is_none()
            && view
                .commits()
                .iter()
                .any(|commit| commit.message == "Add the users route")
    });
}

#[gpui_kit::test]
async fn interactive_rebase_refuses_a_range_with_a_merge(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("f.txt", "f\n", "feature work");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("m.txt", "m\n", "main work");
    repo.git(&["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    open_row_menu(&app, &view, "main work", cx);
    app.click_menu_item(cx, commit_menu_index("Interactive rebase from here…"));

    app.wait_until(cx, "the warning", |app, _| {
        app.host.has_message(
            mergeon_model::notify::NotifyType::Warning,
            "The range contains a merge commit; interactive rebase supports linear history only.",
        )
    });
    assert!(app.dialogs(cx).is_empty());
}

/// Con cambios sin commitear, una fila encima del historial los resume y lleva
/// a Local Changes; las flechas siguen moviéndose solo entre commits.
#[gpui_kit::test]
async fn shows_the_uncommitted_changes_on_top_and_opens_local_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    repo.write("older.txt", "edited\n");
    repo.write("staged.txt", "new\n");
    repo.git(&["add", "staged.txt"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    app.wait_until(cx, "the uncommitted changes row", |app, cx| {
        app.exists(cx, WORKING_CHANGES_ROW_ID)
    });
    assert_eq!(
        app.label(cx, WORKING_CHANGES_ROW_ID).as_deref(),
        Some("2 uncommitted changes \u{b7} 1 staged")
    );

    // La fila no es un commit: las flechas la saltan.
    app.in_window(cx, |window, cx| window.press("up", cx));
    assert_eq!(
        selected_message(&view, cx).as_deref(),
        Some("current branch commit")
    );
    app.in_window(cx, |window, cx| window.press("down", cx));
    app.wait_for(cx, &view, "the next commit", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "older commit")
    });

    app.click(cx, WORKING_CHANGES_ROW_ID);

    let path = repo.path_string();
    app.wait_until(cx, "Local Changes", |app, cx| {
        app.model.read_with(cx, |model, _| {
            model.repository_workspace(&path).active_tab == CHANGES_TAB
        })
    });
}

/// Sin nada que commitear no hay fila de cambios.
#[gpui_kit::test]
async fn leaves_out_the_uncommitted_changes_row_on_a_clean_tree(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    open_view(&app, &repo, true, cx).await;
    app.settle(cx, 100);

    assert!(!app.exists(cx, WORKING_CHANGES_ROW_ID));
}

/// "Reset (Soft)" pide confirmación y deja lo deshecho preparado en el índice.
#[gpui_kit::test]
async fn soft_reset_from_the_commit_menu_keeps_the_changes_staged(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let target = hash_of(&view, "older commit", cx);

    app.host.answer_confirm(true);
    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Reset (Soft)"));

    app.wait_until(cx, "the reset", |_, _| repo.head() == target);
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]),
        "current.txt"
    );
    let confirms = app.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Confirm reset"));
    assert!(
        confirms[0]
            .description
            .as_deref()
            .is_some_and(|text| text.ends_with("using soft mode?"))
    );
}

/// Los tres resets van seguidos en el menú: Soft, Mixed y Hard.
#[gpui_kit::test]
async fn offers_soft_mixed_and_hard_reset_in_that_order(cx: &mut TestAppContext) {
    let soft = commit_menu_index("Reset (Soft)");
    assert_eq!(commit_menu_index("Reset (Mixed)"), soft + 1);
    assert_eq!(commit_menu_index("Reset (Hard)"), soft + 2);

    // Cancelar no toca nada.
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let head = repo.head();
    app.host.answer_confirm(false);
    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, soft);
    app.settle(cx, 200);
    assert_eq!(repo.head(), head);
    assert_eq!(app.host.confirms().len(), 1);
}

#[gpui_kit::test]
async fn copies_the_github_link_of_a_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    repo.git(&["remote", "add", "origin", "git@github.com:octo/demo.git"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let hash = hash_of(&view, "older commit", cx);

    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Copy GitHub link"));

    assert_eq!(
        app.clipboard(cx),
        Some(format!("https://github.com/octo/demo/commit/{hash}"))
    );
    assert!(
        app.host
            .has_message(NotifyType::Success, "GitHub link copied to clipboard")
    );
}

/// Sin remoto de GitHub el menú acaba en "Copy SHA".
#[gpui_kit::test]
async fn a_repository_off_github_has_no_github_link(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://gitlab.com/octo/demo.git",
    ]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    open_row_menu(&app, &view, "older commit", cx);
    assert_eq!(app.menu_len(cx), commit_menu_index("Copy SHA") + 1);
}

/// "Changed Files": abrir en el editor, copiar la ruta y, en GitHub, el enlace
/// al archivo en el commit elegido.
#[gpui_kit::test]
async fn the_changed_files_menu_copies_the_github_link_of_the_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/octo/demo.git",
    ]);
    let (_, view) = open_view(&app, &repo, false, cx).await;
    let hash = hash_of(&view, "current branch commit", cx);

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("current.txt"), cx)
    });
    // Open in editor · separador · File history · separador · Copy Path ·
    // Copy GitHub link
    assert_eq!(app.menu_len(cx), 6);
    app.click_menu_item(cx, 5);
    assert_eq!(
        app.clipboard(cx),
        Some(format!(
            "https://github.com/octo/demo/blob/{hash}/current.txt"
        ))
    );

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("current.txt"), cx)
    });
    app.click_menu_item(cx, 4);
    assert_eq!(app.clipboard(cx).as_deref(), Some("current.txt"));
}

#[gpui_kit::test]
async fn the_changed_files_menu_has_no_github_link_off_github(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, _view) = open_view(&app, &repo, false, cx).await;

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("current.txt"), cx)
    });
    // Open in editor · separador · File history · separador · Copy Path
    assert_eq!(app.menu_len(cx), 5);
}

#[gpui_kit::test]
async fn the_files_of_a_commit_open_in_the_editor_from_their_menu(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, _view) = open_view(&app, &repo, true, cx).await;
    app.model
        .update(cx, |model, cx| {
            model.save_external_editor(
                mergeon_backend::ExternalEditorSettings::custom("/opt/ed", "--file {path}"),
                cx,
            )
        })
        .await;
    app.wait_until(cx, "the files of the commit", |app, cx| {
        app.exists(cx, mergeon_view_commits::file_row_id("current.txt"))
    });

    app.in_window(cx, |window, cx| {
        window.right_click(mergeon_view_commits::file_row_id("current.txt"), cx)
    });
    assert!(app.menu_is_open(cx));
    // Open in editor, lo primero.
    app.click_menu_item(cx, 0);
    app.wait_until(cx, "the editor", |app, _| {
        !app.launcher.launched().is_empty()
    });
    let launched = app.launcher.launched();
    assert_eq!(launched[0].command, "/opt/ed");
    assert_eq!(
        launched[0].args,
        [
            "--file".to_string(),
            format!("{}/current.txt", repo.path_string())
        ]
    );
}

// ---------------------------------------------------------------------------
// Fixups dirigidos
// ---------------------------------------------------------------------------

fn subjects(repo: &TestRepo) -> Vec<String> {
    repo.git(&["log", "--format=%s"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// "Create fixup for this commit" hace `commit --fixup` con lo preparado y
/// el historial se relee con el commit nuevo.
#[gpui_kit::test]
async fn creates_a_fixup_for_a_commit_from_its_menu(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "Add a");
    repo.commit_file("b.txt", "b\n", "Add b");
    repo.write("a.txt", "a fixed\n");
    repo.git(&["add", "a.txt"]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    open_row_menu(&app, &view, "Add a", cx);
    app.click_menu_item(cx, commit_menu_index("Create fixup for this commit"));

    app.wait_until(cx, "the fixup commit", |_, _| {
        repo.git(&["log", "-1", "--format=%s"]) == "fixup! Add a"
    });
    assert!(app.host.confirms().is_empty());
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    app.wait_for(cx, &view, "the reloaded history", |view, _| {
        view.commits()
            .iter()
            .any(|commit| commit.message == "fixup! Add a")
    });
}

/// Sin nada preparado la entrada está apagada: el clic no crea nada.
#[gpui_kit::test]
async fn create_fixup_needs_staged_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "Add a");
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let before = repo.head();

    open_row_menu(&app, &view, "Add a", cx);
    app.click_menu_item(cx, commit_menu_index("Create fixup for this commit"));
    app.wait_until(cx, "the view to settle", |app, cx| {
        !app.model.read_with(cx, |model, _| model.git_is_loading())
    });

    assert_eq!(repo.head(), before);
}

/// "Autosquash fixups" pregunta con lo que va a pasar y deja un solo commit
/// con el cambio del fixup.
#[gpui_kit::test]
async fn autosquash_folds_the_fixups_after_confirming(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "Add a");
    let target = repo.head();
    repo.commit_file("b.txt", "b\n", "Add b");
    repo.write("a.txt", "a fixed\n");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-q", &format!("--fixup={target}")]);
    let (_, view) = open_view(&app, &repo, true, cx).await;

    app.host.answer_confirm(true);
    open_row_menu(&app, &view, "Add a", cx);
    app.click_menu_item(cx, commit_menu_index("Autosquash fixups"));

    app.wait_until(cx, "the autosquash", |_, _| {
        subjects(&repo) == ["Add b", "Add a", "Initial commit"]
    });
    assert_eq!(repo.git(&["show", "HEAD~1:a.txt"]), "a fixed");
    let confirms = app.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Autosquash fixups"));
    assert!(
        confirms[0]
            .description
            .as_deref()
            .is_some_and(|text| text.starts_with("Fold 1 fixup commit into its target?")),
        "{:?}",
        confirms[0].description
    );
    app.wait_for(cx, &view, "the reloaded history", |view, _| {
        !view
            .commits()
            .iter()
            .any(|commit| commit.message.starts_with("fixup! "))
    });
}

#[gpui_kit::test]
async fn autosquash_without_fixups_only_warns(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    let before = repo.head();

    open_row_menu(&app, &view, "older commit", cx);
    app.click_menu_item(cx, commit_menu_index("Autosquash fixups"));

    app.wait_until(cx, "the warning", |app, _| {
        app.host
            .messages(NotifyType::Warning)
            .iter()
            .any(|message| message.starts_with("There are no fixup commits from"))
    });
    assert!(app.host.confirms().is_empty());
    assert_eq!(repo.head(), before);
}

/// Un fixup que no encaja deja el rebase parado como cualquier otro: el
/// mismo Abort de la barra lo deshace.
#[gpui_kit::test]
async fn a_conflicting_autosquash_stops_in_a_rebase_that_can_be_aborted(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("file.txt", "first\n", "first edit");
    let first = repo.head();
    repo.commit_file("file.txt", "second\n", "second edit");
    repo.write("file.txt", "fixed\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-q", &format!("--fixup={first}")]);
    let before = repo.head();
    let (_, view) = open_view(&app, &repo, true, cx).await;

    app.host.answer_confirm(true);
    open_row_menu(&app, &view, "first edit", cx);
    app.click_menu_item(cx, commit_menu_index("Autosquash fixups"));

    app.wait_until(cx, "the stopped rebase", |app, cx| {
        app.model.read_with(cx, |model, _| {
            model
                .active_repository()
                .and_then(|repository| repository.operation_state.as_ref())
                .and_then(|state| state.kind)
                .is_some_and(|kind| kind.as_str() == "rebase")
        })
    });

    app.host.answer_confirm(true);
    app.model
        .update(cx, |model, cx| model.handle_sequence_action("abort", cx))
        .await;
    app.wait_until(cx, "the aborted rebase", |_, _| repo.head() == before);
    assert!(!repo.path.join(".git/rebase-merge").exists());
}

fn renders(view: &Entity<CommitsView>, cx: &mut TestAppContext) -> usize {
    view.read_with(cx, |view, _| view.render_count())
}

/// La vista pinta desde su propio estado (si está a la vista lo recoge en
/// `model_changed`), sin leer el modelo: un aviso que no le toca (un toast, un
/// `notify` cualquiera) no la repinta; dejar de estar a la vista sí.
#[gpui_kit::test]
async fn unrelated_model_notifications_do_not_repaint_the_commits_view(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = two_commit_repo();
    let (_, view) = open_view(&app, &repo, true, cx).await;
    app.settle(cx, 100);
    assert!(view.read_with(cx, |view, cx| view.is_active(cx)));

    let before = renders(&view, cx);
    for index in 0..3 {
        app.model.update(cx, |model, cx| {
            model.notify_info(&format!("Something else happened {index}"), cx);
            cx.notify();
        });
        cx.run_until_parked();
    }
    assert_eq!(
        renders(&view, cx),
        before,
        "an unrelated notification repainted"
    );

    // Los ajustes tapan la vista: deja de estar activa y eso sí se pinta.
    app.model
        .update(cx, |model, cx| model.open_settings("general", cx));
    cx.run_until_parked();
    assert!(!view.read_with(cx, |view, cx| view.is_active(cx)));
    let hidden = renders(&view, cx);
    assert!(hidden > before, "{hidden} vs {before}");

    app.model.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(renders(&view, cx), hidden);
}
