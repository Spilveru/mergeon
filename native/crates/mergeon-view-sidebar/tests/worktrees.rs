//! La sección "Worktrees" con git real: listar (principal, con rama,
//! desanclado, bloqueado y sin carpeta), crear con rama nueva y existente,
//! quitar (limpio y con cambios), abrir como pestaña, bloquear, podar, y el
//! checkout de una rama que ya está en otro worktree.

mod support;

use std::path::{Path, PathBuf};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Focusable as _, TestAppContext};
use mergeon_view_sidebar::SidebarCommand;
use mergeon_view_sidebar::dialogs::NewWorktreeMode;

use support::{Harness, TestRepo};

fn worktree_sections() -> serde_json::Value {
    serde_json::json!({
        "starred": true,
        "branches": true,
        "remotes": false,
        "tags": false,
        "stashes": false,
        "submodules": false,
        "worktrees": true,
    })
}

/// La ruta como la dan git y el modelo: con `/` también en Windows.
fn text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn row(path: &Path) -> String {
    format!("worktree:{}", text(path))
}

async fn open(repo: &TestRepo, cx: &mut TestAppContext) -> Harness {
    let h = Harness::open_with(repo, worktree_sections(), cx).await;
    h.wait_app(cx, "the worktree list", |cx| {
        !h.sidebar.read(cx).worktrees().is_empty()
    });
    h
}

fn run(h: &Harness, cx: &mut TestAppContext, command: SidebarCommand) {
    h.in_window(cx, |window, cx| {
        h.sidebar
            .update(cx, |sidebar, cx| sidebar.run_command(command, window, cx));
    });
}

fn worktree_count(h: &Harness, cx: &mut TestAppContext) -> usize {
    h.sidebar
        .read_with(cx, |sidebar, _| sidebar.worktrees().len())
}

fn is_open_as_tab(h: &Harness, cx: &mut TestAppContext, folder: &Path) -> bool {
    let name = folder.file_name().unwrap().to_string_lossy().into_owned();
    h.model.read_with(cx, |model, _| {
        model
            .open_repositories()
            .iter()
            .any(|repo| repo.path.trim_end_matches('/').ends_with(&name))
    })
}

fn add(repo: &TestRepo, args: &[&str]) {
    let mut full = vec!["worktree", "add"];
    full.extend(args);
    repo.git(&full);
}

#[gpui_kit::test]
async fn lists_every_kind_of_worktree_with_its_state(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let root = repo.root();
    let feature = root.join("demo feature ñ");
    let detached = root.join("demo-detached");
    let gone = root.join("demo-gone");
    add(&repo, &[&text(&feature), "feature"]);
    add(&repo, &["--detach", &text(&detached), "HEAD"]);
    repo.git(&["worktree", "lock", "--reason", "usb", &text(&detached)]);
    add(&repo, &["-b", "old", &text(&gone)]);
    std::fs::remove_dir_all(&gone).unwrap();
    std::fs::write(feature.join("wip.txt"), "wip\n").unwrap();

    let h = open(&repo, cx).await;
    h.wait_app(cx, "four worktrees", |cx| {
        h.sidebar.read(cx).worktrees().len() == 4
    });

    for path in [&repo.path, &feature, &detached, &gone] {
        assert!(h.exists(cx, &row(path)), "row for {}", path.display());
    }
    h.sidebar.read_with(cx, |sidebar, _| {
        let list = sidebar.worktrees();
        assert!(list[0].is_main && list[0].is_current);
        let by = |path: &PathBuf| list.iter().find(|w| w.path == text(path)).unwrap();
        assert_eq!(by(&feature).branch.as_deref(), Some("feature"));
        assert!(by(&detached).detached && by(&detached).locked);
        assert!(by(&gone).prunable);
        // La rama de otro worktree se reconoce; la de este no.
        assert_eq!(
            sidebar.branch_worktree("feature"),
            Some(text(&feature).as_str())
        );
        assert_eq!(sidebar.branch_worktree("main"), None);
    });
    // El estado se lee en segundo plano: el de `feature` tiene cambios.
    h.wait_app(cx, "the worktree states", |cx| {
        h.sidebar.read(cx).worktree_has_changes(&text(&feature)) == Some(true)
    });
    assert_eq!(
        h.sidebar.read_with(cx, |sidebar, _| sidebar
            .worktree_has_changes(&text(&repo.path))),
        Some(false)
    );
    assert!(h.exists(cx, &format!("worktree-dirty:{}", text(&feature))));
    // La rama ocupada se marca en Branches.
    assert!(h.exists(cx, "branch-worktree:feature"));
    assert!(h.exists(cx, "prune-worktrees"));
}

#[gpui_kit::test]
async fn creates_a_worktree_with_a_new_branch_and_opens_it(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = open(&repo, cx).await;

    h.hover(cx, "section-header-worktrees");
    h.click(cx, "new-worktree");
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.new_worktree_dialog().cloned())
        .expect("the dialog is open");
    assert_eq!(h.dialog_titles(cx), ["New Worktree"]);
    // Se escribe en el campo de la rama, que tiene el foco al abrir.
    h.in_window(cx, |window, cx| window.input("feature/login", cx));
    // La carpeta sugerida sigue a la rama.
    let expected = repo.root().join("demo-feature-login");
    assert_eq!(
        dialog.read_with(cx, |dialog, cx| dialog
            .folder_input()
            .read(cx)
            .value()
            .to_string()),
        text(&expected)
    );
    h.click(cx, "new-worktree-submit");

    h.wait_app(cx, "the new worktree", |cx| {
        h.sidebar.read(cx).worktrees().len() == 2 || h.model.read(cx).open_repositories().len() == 2
    });
    h.wait_app(cx, "the worktree tab", |_| {
        expected.join("README.md").exists()
    });
    h.wait(cx, "the new tab", |model| {
        model.open_repositories().len() == 2
    });
    assert!(is_open_as_tab(&h, cx, &expected));
    assert!(
        repo.git(&["branch", "--list", "feature/login"])
            .contains("feature/login")
    );
}

#[gpui_kit::test]
async fn checks_out_an_existing_branch_in_a_new_worktree_from_its_menu(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = open(&repo, cx).await;

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(
        cx,
        "branch:feature",
        &entries,
        "Check out in new worktree...",
    );
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.new_worktree_dialog().cloned())
        .expect("the dialog is open");
    assert_eq!(
        dialog.read_with(cx, |dialog, _| dialog.mode()),
        NewWorktreeMode::ExistingBranch
    );
    h.in_window(cx, |_, cx| {
        dialog.update(cx, |dialog, cx| dialog.set_open_after(false, cx));
    });
    h.click(cx, "new-worktree-submit");

    let folder = repo.root().join("demo-feature");
    h.wait_app(cx, "the worktree", |cx| {
        h.sidebar.read(cx).branch_worktree("feature") == Some(text(&folder).as_str())
    });
    assert!(folder.join("feature.txt").exists());
    assert_eq!(
        h.model
            .read_with(cx, |model, _| model.open_repositories().len()),
        1
    );
    // Ya ocupada: su menú ofrece abrir ese worktree, no otro.
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    assert!(
        entries
            .iter()
            .any(|entry| entry.label() == "Open its worktree (demo-feature)")
    );
    assert!(
        entries
            .iter()
            .find(|entry| entry.label() == "Check out in new worktree...")
            .is_some_and(|entry| entry.is_disabled())
    );
}

#[gpui_kit::test]
async fn removes_a_clean_worktree_and_asks_before_losing_changes(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let clean = repo.root().join("demo-clean");
    let dirty = repo.root().join("demo-dirty");
    add(&repo, &["-b", "clean", &text(&clean)]);
    add(&repo, &["-b", "dirty", &text(&dirty)]);
    std::fs::write(dirty.join("wip.txt"), "wip\n").unwrap();
    let h = open(&repo, cx).await;
    h.wait_app(cx, "three worktrees", |cx| {
        h.sidebar.read(cx).worktrees().len() == 3
    });

    h.host.answer_confirm_with_option(true, false);
    run(&h, cx, SidebarCommand::RemoveWorktree(text(&clean)));
    h.wait_app(cx, "the removal", |_| !clean.exists());
    h.wait_app(cx, "the shorter list", |cx| {
        h.sidebar.read(cx).worktrees().len() == 2
    });
    assert!(
        repo.git(&["branch", "--list", "clean"]).contains("clean"),
        "the branch stays"
    );

    // Con cambios y sin forzar, git se niega y se dice por qué.
    h.wait_app(cx, "the dirty state", |cx| {
        h.sidebar.read(cx).worktree_has_changes(&text(&dirty)) == Some(true)
    });
    h.host.answer_confirm_with_option(true, false);
    run(&h, cx, SidebarCommand::RemoveWorktree(text(&dirty)));
    h.wait_app(cx, "the refusal", |_| {
        h.host
            .toasts()
            .iter()
            .any(|toast| toast.full_message.contains("has uncommitted changes"))
    });
    assert!(dirty.join("wip.txt").exists());
    let confirm = h.host.option_confirms().pop().expect("the confirmation");
    assert!(
        confirm
            .description
            .as_deref()
            .is_some_and(|text| text.contains("It has uncommitted changes")),
        "{confirm:?}"
    );

    h.host.answer_confirm_with_option(true, true);
    run(&h, cx, SidebarCommand::RemoveWorktree(text(&dirty)));
    h.wait_app(cx, "the forced removal", |_| !dirty.exists());
}

#[gpui_kit::test]
async fn opens_a_worktree_as_a_tab_with_a_double_click(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let feature = repo.root().join("demo-feature");
    add(&repo, &[&text(&feature), "feature"]);
    let h = open(&repo, cx).await;
    h.wait_app(cx, "two worktrees", |cx| {
        h.sidebar.read(cx).worktrees().len() == 2
    });

    h.double_click(cx, &row(&feature));

    h.wait(cx, "the new tab", |model| {
        model.open_repositories().len() == 2
    });
    assert!(is_open_as_tab(&h, cx, &feature));
    h.wait_repo(cx, "the worktree as the active repository", |repository| {
        repository.current_branch.as_deref() == Some("feature")
    });
}

#[gpui_kit::test]
async fn a_branch_checked_out_elsewhere_is_not_checked_out_here(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let feature = repo.root().join("demo-feature");
    add(&repo, &[&text(&feature), "feature"]);
    let h = open(&repo, cx).await;
    h.wait_app(cx, "the busy branch", |cx| {
        h.sidebar.read(cx).branch_worktree("feature").is_some()
    });

    h.host.answer_confirm(false);
    run(&h, cx, SidebarCommand::CheckoutBranch("feature".into()));
    h.wait_app(cx, "the warning", |_| !h.host.confirms().is_empty());
    let confirm = h.host.confirms().pop().unwrap();
    assert_eq!(
        confirm.title.as_deref(),
        Some("'feature' is open in another worktree")
    );
    assert!(confirm.description.unwrap().contains(&text(&feature)));
    h.settle(cx);
    assert_eq!(repo.current_branch(), "main", "nothing was checked out");

    // Aceptar abre ese worktree.
    h.host.answer_confirm(true);
    run(&h, cx, SidebarCommand::CheckoutBranch("feature".into()));
    h.wait(cx, "the worktree tab", |model| {
        model.open_repositories().len() == 2
    });
}

#[gpui_kit::test]
async fn locks_unlocks_and_prunes(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let locked = repo.root().join("demo-locked");
    let gone = repo.root().join("demo-gone");
    add(&repo, &["-b", "locked", &text(&locked)]);
    add(&repo, &["-b", "gone", &text(&gone)]);
    let h = open(&repo, cx).await;
    h.wait_app(cx, "three worktrees", |cx| {
        h.sidebar.read(cx).worktrees().len() == 3
    });

    run(&h, cx, SidebarCommand::LockWorktree(text(&locked)));
    h.wait_app(cx, "the lock", |cx| {
        h.sidebar
            .read(cx)
            .worktrees()
            .iter()
            .any(|worktree| worktree.locked && worktree.path == text(&locked))
    });
    let menu = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.worktree_menu(&text(&locked), cx));
    assert!(menu.iter().any(|entry| entry.label() == "Unlock"));

    std::fs::remove_dir_all(&gone).unwrap();
    std::fs::remove_dir_all(&locked).unwrap();
    h.click(cx, "sidebar-refresh");
    h.wait_app(cx, "the stale worktree", |cx| {
        h.sidebar
            .read(cx)
            .worktrees()
            .iter()
            .any(|worktree| worktree.prunable)
    });
    h.hover(cx, "section-header-worktrees");
    h.click(cx, "prune-worktrees");
    h.wait_app(cx, "the prune", |cx| {
        h.sidebar.read(cx).worktrees().len() == 2
    });
    // El bloqueado no se poda hasta desbloquearlo.
    run(&h, cx, SidebarCommand::UnlockWorktree(text(&locked)));
    h.wait_app(cx, "the unlock", |cx| {
        h.sidebar
            .read(cx)
            .worktrees()
            .iter()
            .all(|worktree| !worktree.locked)
    });
    run(&h, cx, SidebarCommand::PruneWorktrees);
    h.wait_app(cx, "the second prune", |cx| {
        h.sidebar.read(cx).worktrees().len() == 1
    });
    assert_eq!(worktree_count(&h, cx), 1);
}

#[gpui_kit::test]
async fn existing_branch_mode_picks_from_the_local_branches(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.git(&["branch", "another"]);
    let h = open(&repo, cx).await;

    run(&h, cx, SidebarCommand::NewWorktree);
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.new_worktree_dialog().cloned())
        .expect("the dialog is open");
    h.in_window(cx, |_, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.set_mode(NewWorktreeMode::ExistingBranch, cx)
        });
    });
    assert!(h.exists(cx, "new-worktree-branch"));
    let choices = dialog.read_with(cx, |dialog, _| dialog.branch_choices());
    assert_eq!(
        choices,
        [
            ("another".to_string(), "another".to_string()),
            ("feature".to_string(), "feature".to_string()),
            ("main".to_string(), "main (in a worktree)".to_string()),
        ]
    );

    h.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| dialog.choose_branch("feature", window, cx));
    });
    dialog.read_with(cx, |dialog, cx| {
        assert_eq!(
            dialog.branch_input().read(cx).value().to_string(),
            "feature"
        );
        assert!(
            dialog
                .folder_input()
                .read(cx)
                .value()
                .ends_with("demo-feature")
        );
        assert!(dialog.is_submittable(cx));
    });

    // Abrir otro no deja el anterior huérfano.
    run(&h, cx, SidebarCommand::NewWorktree);
    assert_eq!(h.dialog_titles(cx).len(), 1);
}

#[gpui_kit::test]
async fn copying_a_worktree_path_says_so(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = open(&repo, cx).await;
    run(&h, cx, SidebarCommand::CopyWorktreePath(text(&repo.path)));
    assert!(h.host.has_message(
        mergeon_model::notify::NotifyType::Success,
        "Worktree path copied to clipboard"
    ));
}

#[gpui_kit::test]
async fn a_mutation_rereads_only_this_worktree_state(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let root = repo.root();
    let feature = root.join("demo-feature-wt");
    let other = root.join("demo-other-wt");
    add(&repo, &[&text(&feature), "feature"]);
    add(&repo, &["-b", "other", &text(&other)]);
    let h = open(&repo, cx).await;
    h.wait_app(cx, "every worktree state", |cx| {
        let sidebar = h.sidebar.read(cx);
        sidebar.worktrees().len() == 3
            && [&repo.path, &feature, &other]
                .iter()
                .all(|path| sidebar.worktree_has_changes(&text(path)).is_some())
    });
    h.settle(cx);
    let (before, list_reads) = h.sidebar.read_with(cx, |sidebar, _| {
        (sidebar.worktree_status_reads(), sidebar.worktree_reads())
    });

    // Una mutación de este repositorio (crear un tag desde el sidebar) relee
    // la lista, que no cambia.
    h.host.answer_prompt("v1");
    h.host.cancel_prompt();
    run(&h, cx, SidebarCommand::CreateTag);
    h.wait_app(cx, "the list to be read again", |cx| {
        h.sidebar.read(cx).worktree_reads() > list_reads
    });
    h.settle(cx);

    let after = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.worktree_status_reads());
    assert!(
        after - before <= 2,
        "a mutation re-read {} worktree states",
        after - before
    );
    // El refresco manual sí relee todos.
    h.click(cx, "sidebar-refresh");
    h.settle(cx);
    h.wait_app(cx, "every state again", |cx| {
        h.sidebar.read(cx).worktree_status_reads() >= after + 3
    });
}

#[gpui_kit::test]
async fn existing_branch_mode_focuses_the_folder_input_on_open(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = open(&repo, cx).await;

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(
        cx,
        "branch:feature",
        &entries,
        "Check out in new worktree...",
    );
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.new_worktree_dialog().cloned())
        .expect("the dialog is open");
    assert_eq!(
        dialog.read_with(cx, |dialog, _| dialog.mode()),
        NewWorktreeMode::ExistingBranch
    );

    let (folder_focused, branch_focused) = h.in_window(cx, |window, cx| {
        let state = dialog.read(cx);
        (
            state
                .folder_input()
                .read(cx)
                .focus_handle(cx)
                .is_focused(window),
            state
                .branch_input()
                .read(cx)
                .focus_handle(cx)
                .is_focused(window),
        )
    });
    assert!(folder_focused, "folder input should be focused");
    assert!(
        !branch_focused,
        "unrendered branch input should not be focused"
    );
}
