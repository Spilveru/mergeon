//! "File history": la pestaña con el historial de un archivo siguiendo sus
//! renombres, el diff de cada revisión, las flechas, la paginación y la
//! navegación al commit.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, point, px};
use mergeon_app::AppEvent;
use mergeon_model::workspace_tabs::{FileTabDescriptor, HOME_TAB};
use mergeon_view_commits::{HistoricalFileTabView, file_history_row_id};

use crate::support::{TestApp, TestRepo};

/// `notes.txt` → `docs/notes.md` → `docs/guide.md`, con cambios entre medias.
fn renamed_twice() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit_file("notes.txt", "one\n", "Add notes");
    repo.commit_file("notes.txt", "one\ntwo\n", "Edit notes");
    std::fs::create_dir_all(repo.path.join("docs")).unwrap();
    repo.git(&["mv", "notes.txt", "docs/notes.md"]);
    repo.git(&["commit", "-q", "-m", "Move notes to docs"]);
    repo.commit_file("docs/notes.md", "one\ntwo\nthree\n", "Edit docs notes");
    repo.git(&["mv", "docs/notes.md", "docs/guide.md"]);
    repo.git(&["commit", "-q", "-m", "Rename notes to guide"]);
    repo.commit_file("other.txt", "noise\n", "Unrelated");
    repo
}

async fn open_history(
    app: &TestApp,
    repo: &TestRepo,
    descriptor: FileTabDescriptor,
    cx: &mut TestAppContext,
) -> (String, Entity<HistoricalFileTabView>) {
    let repo_id = app.open(repo, cx).await;
    let path = repo.path_string();
    let tab_id = descriptor.id();
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &descriptor, false, cx);
    });
    let id = repo_id.clone();
    let view = app.mount(cx, move |window, cx| {
        HistoricalFileTabView::new(id, tab_id, window, cx)
    });
    (repo_id, view)
}

/// Clic en la fila de una revisión, sobre el asunto (lejos del hash).
fn click_row(app: &TestApp, cx: &mut TestAppContext, hash: &str) {
    let id = file_history_row_id(hash);
    app.in_window(cx, |window, cx| {
        window.click_at(id, point(px(30.), px(12.)), cx)
    });
}

fn subjects(view: &Entity<HistoricalFileTabView>, cx: &mut TestAppContext) -> Vec<String> {
    view.read_with(cx, |view, cx| {
        view.history()
            .read(cx)
            .rows()
            .iter()
            .map(|row| row.revision.subject.clone())
            .collect()
    })
}

fn selected_subject(view: &Entity<HistoricalFileTabView>, cx: &mut TestAppContext) -> String {
    view.read_with(cx, |view, _| {
        view.selected_revision()
            .map(|revision| revision.subject.clone())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
async fn follows_a_file_renamed_twice_and_shows_each_revision(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = renamed_twice();
    let (_, view) = open_history(
        &app,
        &repo,
        FileTabDescriptor::history("docs/guide.md", None),
        cx,
    )
    .await;

    // La primera revisión queda elegida y su diff es el renombre.
    app.wait_for(cx, &view, "the history and its first diff", |view, cx| {
        view.history().read(cx).rows().len() == 5 && !view.is_loading() && !view.diff().is_empty()
    });
    assert_eq!(
        subjects(&view, cx),
        [
            "Rename notes to guide",
            "Edit docs notes",
            "Move notes to docs",
            "Edit notes",
            "Add notes"
        ]
    );
    assert_eq!(selected_subject(&view, cx), "Rename notes to guide");
    let diff = view.read_with(cx, |view, _| view.diff().to_string());
    assert!(diff.contains("rename from docs/notes.md"), "{diff}");
    assert!(diff.contains("rename to docs/guide.md"), "{diff}");

    // Insignias: de dónde viene cada renombre y dónde nació.
    let hashes: Vec<String> = view.read_with(cx, |view, cx| {
        view.history()
            .read(cx)
            .rows()
            .iter()
            .map(|row| row.revision.hash.clone())
            .collect()
    });
    assert_eq!(
        app.label(cx, format!("revision-badge:{}", hashes[0]))
            .as_deref(),
        Some("renamed from notes.md")
    );
    assert_eq!(
        app.label(cx, format!("revision-badge:{}", hashes[2]))
            .as_deref(),
        Some("renamed from notes.txt")
    );
    assert_eq!(
        app.label(cx, format!("revision-badge:{}", hashes[4]))
            .as_deref(),
        Some("added")
    );
    assert_eq!(
        app.label(cx, "file-history-count").as_deref(),
        Some("5 revisions")
    );

    // Clic en una revisión de antes de los renombres: su diff con la ruta
    // que tenía entonces.
    click_row(&app, cx, &hashes[3]);
    app.wait_for(cx, &view, "the diff of the old name", |view, _| {
        !view.is_loading() && view.diff().contains("+two")
    });
    let diff = view.read_with(cx, |view, _| view.diff().to_string());
    assert!(diff.contains("a/notes.txt"), "{diff}");
    let shown_path = view.read_with(cx, |view, cx| {
        view.diff_viewer().read(cx).file_path().to_string()
    });
    assert_eq!(shown_path, "notes.txt");

    // Las flechas recorren las revisiones (el clic dejó el foco en la lista).
    app.in_window(cx, |window, cx| window.press("down", cx));
    app.wait_for(cx, &view, "the next revision", |view, _| {
        view.selected_revision()
            .is_some_and(|revision| revision.subject == "Add notes")
            && !view.is_loading()
    });
    app.in_window(cx, |window, cx| window.press("up", cx));
    app.in_window(cx, |window, cx| window.press("up", cx));
    app.wait_for(cx, &view, "two revisions up", |view, _| {
        view.selected_revision()
            .is_some_and(|revision| revision.subject == "Move notes to docs")
    });
}

/// Clic en el hash: la pestaña Commits pasa a verse y el modelo pide elegir
/// ese commit.
#[gpui_kit::test]
async fn the_hash_of_a_revision_leads_to_its_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = renamed_twice();
    let (repo_id, view) = open_history(
        &app,
        &repo,
        FileTabDescriptor::history("docs/guide.md", None),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the history", |view, cx| {
        view.history().read(cx).rows().len() == 5
    });
    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscription = cx.update(|cx| {
        let requests = requests.clone();
        cx.subscribe(&app.model, move |_, event: &AppEvent, _| {
            if let AppEvent::CommitSelectionRequested { repo_id, hash } = event {
                requests.borrow_mut().push((repo_id.clone(), hash.clone()));
            }
        })
    });
    let target = repo.rev("HEAD~2");

    app.click(cx, format!("revision-hash:{target}"));

    assert_eq!(requests.borrow().as_slice(), [(repo_id, target)]);
    let active = app.model.read_with(cx, |model, _| {
        model.repository_workspace(&repo.path_string()).active_tab
    });
    assert_eq!(active, HOME_TAB);
    // El clic en el hash no cambia la revisión elegida.
    assert_eq!(selected_subject(&view, cx), "Rename notes to guide");
}

/// Desde un commit concreto: solo la historia que alcanza ese commit, con la
/// ruta que tenía el archivo en él.
#[gpui_kit::test]
async fn a_history_from_a_commit_starts_there(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = renamed_twice();
    let middle = repo.rev("HEAD~2");
    let (_, view) = open_history(
        &app,
        &repo,
        FileTabDescriptor::history("docs/notes.md", Some(&middle)),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the history", |view, cx| {
        view.history().read(cx).rows().len() == 4
    });
    assert_eq!(selected_subject(&view, cx), "Edit docs notes");
}

/// Un repositorio con `count` commits que tocan `old.txt`, el renombre a
/// `new.txt` y `after` commits más sobre `new.txt`, de una vez.
fn long_history(before: usize, after: usize) -> TestRepo {
    let repo = TestRepo::new();
    let parent = repo.head();
    let mut stream = String::new();
    let mut mark = 0;
    let mut commit = |stream: &mut String, message: &str, body: &str| {
        mark += 1;
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{mark}\ncommitter Test User <test@example.com> {} +0000\ndata {}\n{message}\n",
            1_780_000_000 + mark,
            message.len()
        ));
        if mark == 1 {
            stream.push_str(&format!("from {parent}\n"));
        }
        stream.push_str(body);
        stream.push('\n');
    };
    let file = |path: &str, content: &str| {
        format!(
            "M 100644 inline {path}\ndata {}\n{content}\n",
            content.len()
        )
    };
    for index in 0..before {
        commit(
            &mut stream,
            &format!("old {index}"),
            &file("old.txt", &format!("line {index}\n")),
        );
    }
    commit(&mut stream, "rename", "R old.txt new.txt\n");
    for index in 0..after {
        commit(
            &mut stream,
            &format!("new {index}"),
            &file("new.txt", &format!("line {index}\nmore\n")),
        );
    }
    let mut child = std::process::Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(&repo.path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("git fast-import");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stream.as_bytes())
        .expect("write stream");
    assert!(child.wait().expect("fast-import").success());
    repo.git(&["reset", "-q", "--hard", "main"]);
    repo
}

/// Páginas de 100: la segunda llega al llegar al final y sigue por el nombre
/// de antes del renombre.
#[gpui_kit::test]
async fn loads_the_next_page_at_the_end_and_keeps_following_the_rename(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = long_history(10, 95);
    let (_, view) =
        open_history(&app, &repo, FileTabDescriptor::history("new.txt", None), cx).await;
    app.wait_for(cx, &view, "the first page", |view, cx| {
        view.history().read(cx).rows().len() == 100
    });
    let list = view.read_with(cx, |view, _| view.history().clone());
    assert!(list.read_with(cx, |list, _| list.has_more()));
    assert_eq!(
        app.label(cx, "file-history-count").as_deref(),
        Some("100+ revisions")
    );

    // Al final de la lista (Fin con el foco en ella) se pide la siguiente.
    click_row(&app, cx, &repo.head());
    app.in_window(cx, |window, cx| window.press("end", cx));
    app.wait_for(cx, &list, "the second page", |list, _| {
        list.rows().len() == 106 && !list.is_loading_more()
    });
    list.read_with(cx, |list, _| {
        assert!(!list.has_more());
        let last = &list.rows()[105].revision;
        assert_eq!(last.subject, "old 0");
        assert_eq!(last.path, "old.txt");
        assert_eq!(list.rows()[95].revision.subject, "rename");
        assert_eq!(
            list.rows()[95].revision.previous_path.as_deref(),
            Some("old.txt")
        );
    });
    // Una lectura por página: nada se pidió dos veces.
    assert_eq!(list.read_with(cx, |list, _| list.load_count()), 2);
}

#[gpui_kit::test]
async fn a_file_without_commits_has_no_history(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("draft.txt", "x\n");
    let (_, view) = open_history(
        &app,
        &repo,
        FileTabDescriptor::history("draft.txt", None),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the empty history", |view, cx| {
        let list = view.history().read(cx);
        !list.is_loading() && list.source().is_some()
    });
    assert!(view.read_with(cx, |view, _| view.selected_revision().is_none()));
    assert_eq!(
        view.read_with(cx, |view, cx| view.status_text(cx))
            .as_deref(),
        Some("No commit has touched this file.")
    );
}

/// "File history" desde la pestaña del diff de un commit: abre la pestaña de
/// historial desde ese commit.
#[gpui_kit::test]
async fn the_commit_tab_opens_the_file_history(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = renamed_twice();
    let commit = repo.rev("HEAD~1");
    let repo_id = app.open(&repo, cx).await;
    let path = repo.path_string();
    let descriptor = FileTabDescriptor::commit("docs/guide.md", &commit);
    let tab_id = descriptor.id();
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &descriptor, false, cx);
    });
    let view = app.mount(cx, move |window, cx| {
        HistoricalFileTabView::new(repo_id, tab_id, window, cx)
    });
    app.wait_for(cx, &view, "the diff", |view, _| {
        !view.is_loading() && !view.diff().is_empty()
    });

    app.click(cx, "open-file-history");

    let active = app.model.read_with(cx, |model, _| {
        model.repository_workspace(&repo.path_string()).active_tab
    });
    assert_eq!(
        active,
        FileTabDescriptor::history("docs/guide.md", Some(&commit)).id()
    );
}
