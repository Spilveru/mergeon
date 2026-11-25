//! `CommitTreeTab.test.js`, sobre un repositorio real.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Entity, TestAppContext};
use mergeon_backend::FileChange;
use mergeon_model::format::format_size;
use mergeon_model::workspace_tabs::FileTabDescriptor;
use mergeon_view_commits::{CommitTreeTab, CommitTreeTabEvent};

use crate::support::{TestApp, TestRepo};

/// `package.json` (2 KB), `src/App.vue` y `src/lib/diff.js`.
fn tree_repo() -> TestRepo {
    let repo = TestRepo::empty();
    repo.write(
        "package.json",
        &format!("{{\n  \"name\": \"mergeon\"\n}}\n{}", " ".repeat(2048 - 25)),
    );
    repo.write("src/App.vue", "<template></template>\n");
    repo.write("src/lib/diff.js", "export const diff = 1\n");
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "tree"]);
    repo
}

type Events = Rc<RefCell<Vec<CommitTreeTabEvent>>>;

async fn mount_tree(
    app: &TestApp,
    repo: &TestRepo,
    external: bool,
    cx: &mut TestAppContext,
) -> (Entity<CommitTreeTab>, Events) {
    app.open(repo, cx).await;
    let tree = app.mount(cx, CommitTreeTab::new);
    let (path, head) = (repo.path_string(), repo.head());
    tree.update(cx, |tree, cx| {
        tree.set_external_files(external, cx);
        tree.set_commit_files(
            &[FileChange {
                path: "src/lib/diff.js".to_string(),
                status: "A".to_string(),
                additions: 1,
                deletions: 0,
            }],
            cx,
        );
        tree.set_source(&path, &head, cx);
    });
    let events: Events = Rc::default();
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&tree, move |_, event: &CommitTreeTabEvent, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    wait_idle(app, &tree, cx);
    (tree, events)
}

fn wait_idle(app: &TestApp, tree: &Entity<CommitTreeTab>, cx: &mut TestAppContext) {
    app.wait_for(cx, tree, "the tree", |tree, _| {
        !tree.is_loading_root()
            && tree
                .rows()
                .iter()
                .all(|row| !tree.is_loading(&row.entry.path))
    });
}

fn names(tree: &Entity<CommitTreeTab>, cx: &mut TestAppContext) -> Vec<String> {
    tree.read_with(cx, |tree, _| {
        tree.rows()
            .iter()
            .map(|row| row.entry.name.clone())
            .collect()
    })
}

#[gpui_kit::test]
async fn discards_a_late_directory_response_after_changing_commits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let old = repo.head();
    repo.git(&["rm", "-q", "-r", "src", "package.json"]);
    repo.commit_file("new-version.js", "1\n", "new version");
    let (tree, events) = mount_tree(&app, &repo, true, cx).await;
    let (path, new) = (repo.path_string(), repo.head());

    // El árbol del commit viejo se pide y, sin esperarlo, se cambia de commit.
    tree.update(cx, |tree, cx| {
        tree.set_source(&path, &old, cx);
        tree.set_source(&path, &new, cx);
    });
    wait_idle(&app, &tree, cx);
    app.settle(cx, 200);

    assert_eq!(names(&tree, cx), ["new-version.js"]);
    tree.update(cx, |tree, cx| tree.activate("new-version.js", cx));
    assert_eq!(
        *events.borrow(),
        [CommitTreeTabEvent::OpenFile(FileTabDescriptor::tree(
            "new-version.js",
            &new
        ))]
    );
    // Con la lista fuera de su pestaña, el archivo no se lee aquí.
    assert!(tree.read_with(cx, |tree, cx| tree.viewer().read(cx).file().is_none()));
}

// Lo que se veía antes eran los archivos del commit, no el repositorio.
#[gpui_kit::test]
async fn shows_the_repository_as_it_was_not_just_what_the_commit_touched(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;

    assert_eq!(names(&tree, cx), ["src", "package.json"]);
}

// Un repositorio grande no cabe de una vez: se pide lo que se abre.
#[gpui_kit::test]
async fn asks_only_for_the_folder_being_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;

    tree.update(cx, |tree, cx| tree.activate("src", cx));
    assert!(tree.read_with(cx, |tree, _| tree.is_loading("src")));
    wait_idle(&app, &tree, cx);

    assert_eq!(names(&tree, cx), ["src", "lib", "App.vue", "package.json"]);
}

#[gpui_kit::test]
async fn folds_a_folder_back_without_asking_again(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;
    tree.update(cx, |tree, cx| tree.activate("src", cx));
    wait_idle(&app, &tree, cx);

    tree.update(cx, |tree, cx| tree.activate("src", cx));
    assert_eq!(names(&tree, cx), ["src", "package.json"]);

    // Ya estaba leída: se despliega en el acto.
    tree.update(cx, |tree, cx| tree.activate("src", cx));
    assert!(!tree.read_with(cx, |tree, _| tree.is_loading("src")));
    assert_eq!(names(&tree, cx), ["src", "lib", "App.vue", "package.json"]);
}

// Es lo que el árbol aporta y la lista de cambios no: el cambio en contexto.
#[gpui_kit::test]
async fn marks_the_files_this_commit_touched(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;
    tree.update(cx, |tree, cx| tree.activate("src", cx));
    wait_idle(&app, &tree, cx);
    tree.update(cx, |tree, cx| tree.activate("src/lib", cx));
    wait_idle(&app, &tree, cx);

    let status = tree.read_with(cx, |tree, _| {
        let row = tree
            .rows()
            .iter()
            .find(|row| row.entry.path == "src/lib/diff.js")
            .expect("diff.js");
        tree.changed_status(row).map(str::to_string)
    });
    assert_eq!(status.as_deref(), Some("A"));
}

#[gpui_kit::test]
async fn shows_how_big_each_file_is(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;

    let size = tree.read_with(cx, |tree, _| {
        tree.rows()
            .iter()
            .find(|row| row.entry.name == "package.json")
            .and_then(|row| row.entry.size)
    });
    assert_eq!(
        size.map(|size| format_size(size as f64)).as_deref(),
        Some("2.0 KB")
    );
}

#[gpui_kit::test]
async fn opens_a_file_as_it_was_at_that_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;

    tree.update(cx, |tree, cx| tree.activate("package.json", cx));

    let viewer = tree.read_with(cx, |tree, _| tree.viewer().clone());
    app.wait_for(cx, &viewer, "the file content", |viewer, _| {
        viewer
            .file()
            .is_some_and(|file| file.content.contains("\"mergeon\""))
    });
    assert_eq!(
        tree.read_with(cx, |tree, _| tree.selected_path().to_string()),
        "package.json"
    );
}

// Una carpeta ilegible no debe tumbar la navegación por el resto.
#[gpui_kit::test]
async fn marks_a_folder_it_could_not_read_and_keeps_the_rest_usable(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    // El objeto del árbol de `src` desaparece: leerlo falla.
    let src = repo.rev("HEAD:src");
    let object = repo
        .path
        .join(".git/objects")
        .join(&src[..2])
        .join(&src[2..]);
    std::fs::remove_file(object).expect("remove the src tree object");
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;

    tree.update(cx, |tree, cx| tree.activate("src", cx));
    wait_idle(&app, &tree, cx);

    let failed = tree.read_with(cx, |tree, _| {
        tree.rows()
            .iter()
            .find(|row| row.entry.path == "src")
            .and_then(|row| row.error.clone())
    });
    assert!(failed.is_some());
    assert_eq!(names(&tree, cx), ["src", "package.json"]);
    assert!(app.exists(cx, "tree-error"));
}

#[gpui_kit::test]
async fn says_how_many_entries_it_left_out(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    // 2040 archivos en la raíz: 40 más que el límite de un nivel.
    let blob = repo.git(&["hash-object", "-w", "--stdin"]);
    let index: String = (0..2040)
        .map(|index| format!("100644 {blob}\tfile-{index:04}.txt\n"))
        .collect();
    let mut child = std::process::Command::new("git")
        .args(["update-index", "--index-info"])
        .current_dir(&repo.path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("git update-index");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(index.as_bytes())
        .expect("index");
    assert!(child.wait().expect("update-index").success());
    repo.git(&["commit", "-q", "-m", "many files"]);
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;

    assert_eq!(tree.read_with(cx, |tree, _| tree.omitted_total()), 40);
    assert_eq!(tree.read_with(cx, |tree, _| tree.rows().len()), 2000);
}

#[gpui_kit::test]
async fn starts_over_when_another_commit_is_selected(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = tree_repo();
    let first = repo.head();
    repo.commit_file("CHANGELOG.md", "1\n", "second");
    let (tree, _) = mount_tree(&app, &repo, false, cx).await;
    tree.update(cx, |tree, cx| tree.activate("src", cx));
    wait_idle(&app, &tree, cx);

    let path = repo.path_string();
    tree.update(cx, |tree, cx| tree.set_source(&path, &first, cx));
    wait_idle(&app, &tree, cx);

    assert!(!tree.read_with(cx, |tree, _| tree.is_expanded("src")));
    assert_eq!(names(&tree, cx), ["src", "package.json"]);
}
