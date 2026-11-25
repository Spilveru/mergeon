//! `ConflictResolver`: el resolvedor de tres vías suelto, con las versiones
//! leídas de merges reales (o el contenido con marcadores puesto a mano) y lo
//! que emite registrado.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::Position;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, ElementId, Entity, IntoElement, SharedString, TestAppContext};
use mergeon_git_ops::{ConflictResolution, ConflictSide};
use mergeon_git_query::{ConflictVersions, read_conflict_versions};
use mergeon_view_changes::{ConflictResolver, ConflictResolverEvent, WholeChoice};

use crate::support::{TestApp, TestRepo};

const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
/// Cambia `two`, `six` y `ten`; además, `four` solo de este lado.
const OURS: &str = "one\ntwo main\nthree\nFOUR\nfive\nsix main\nseven\neight\nnine\nten main\n";
/// Cambia `two`, `six` y `ten` de otra forma; además, `eight` solo de este lado.
const THEIRS: &str =
    "one\ntwo feature\nthree\nfour\nfive\nsix feature\nseven\nEIGHT\nnine\nten feature\n";

struct Resolver {
    app: TestApp,
    resolver: Entity<ConflictResolver>,
    events: Rc<RefCell<Vec<ConflictResolverEvent>>>,
}

/// `main` y `feature` cambian `name` desde `base` (o lo borran con `None`) y
/// el merge se para en el conflicto.
fn conflicted_repo(
    name: &str,
    base: Option<&str>,
    ours: Option<&str>,
    theirs: Option<&str>,
) -> TestRepo {
    let repo = TestRepo::empty();
    repo.commit_file("README", "readme\n", "init");
    if let Some(base) = base {
        repo.commit_file(name, base, "base");
    }
    repo.git(&["checkout", "-q", "-b", "feature"]);
    match theirs {
        Some(theirs) => repo.commit_file(name, theirs, "feature"),
        None => {
            repo.git(&["rm", "-q", "--", name]);
            repo.git(&["commit", "-q", "-m", "feature removes"]);
        }
    }
    repo.git(&["checkout", "-q", "main"]);
    match ours {
        Some(ours) => repo.commit_file(name, ours, "main"),
        None => {
            repo.git(&["rm", "-q", "--", name]);
            repo.git(&["commit", "-q", "-m", "main removes"]);
        }
    }
    assert!(
        repo.try_git(&["merge", "feature"]).is_err(),
        "the merge should stop on a conflict"
    );
    repo
}

fn versions_of(repo: &TestRepo, path: &str) -> Arc<ConflictVersions> {
    Arc::new(read_conflict_versions(&repo.path, path).expect("conflict versions"))
}

impl Resolver {
    fn mount(cx: &mut TestAppContext, files: &[&str]) -> Self {
        let app = TestApp::new(cx);
        let events: Rc<RefCell<Vec<ConflictResolverEvent>>> = Rc::default();
        let files: Vec<String> = files.iter().map(|file| file.to_string()).collect();
        let resolver = cx
            .update_window(app.window, {
                let events = events.clone();
                move |_, window, cx| {
                    let resolver = cx.new(|cx| {
                        let mut resolver = ConflictResolver::new(window, cx);
                        resolver.set_files(files.clone(), cx);
                        resolver.set_counts(files.len(), 0, files.len(), cx);
                        resolver
                    });
                    cx.subscribe(&resolver, move |_, event: &ConflictResolverEvent, _| {
                        events.borrow_mut().push(event.clone());
                    })
                    .detach();
                    resolver
                }
            })
            .expect("la ventana de prueba se cerró");
        let shown = resolver.clone();
        app.show(cx, move |_, _| shown.clone().into_any_element());
        Self {
            app,
            resolver,
            events,
        }
    }

    /// Montado con las versiones de `path` en `repo`, ya analizadas.
    fn with_repo(cx: &mut TestAppContext, repo: &TestRepo, path: &str) -> Self {
        let resolver = Self::mount(cx, &[path]);
        resolver.load(cx, path, versions_of(repo, path));
        resolver
    }

    /// Montado con un contenido con marcadores y sin etapas.
    fn with_markers(cx: &mut TestAppContext, content: &str) -> Self {
        let resolver = Self::mount(cx, &["src/a.js"]);
        resolver.resolver.update(cx, |resolver, cx| {
            resolver.set_file("src/a.js", content.to_string(), cx);
        });
        resolver.wait_loaded(cx);
        resolver
    }

    fn load(&self, cx: &mut TestAppContext, path: &str, versions: Arc<ConflictVersions>) {
        self.resolver.update(cx, |resolver, cx| {
            resolver.set_versions(path.to_string(), Some(versions), cx);
        });
        self.wait_loaded(cx);
    }

    fn wait_loaded(&self, cx: &mut TestAppContext) {
        let resolver = self.resolver.clone();
        self.app.wait_until(cx, "the analysis", move |cx| {
            resolver.read_with(cx, |resolver, _| !resolver.is_loading())
        });
        // Un frame para que el editor tome el texto.
        self.app.in_window(cx, |_, _| {});
    }

    fn result(&self, cx: &mut TestAppContext) -> String {
        self.app.in_window(cx, |_, _| {});
        self.resolver
            .read_with(cx, |resolver, cx| resolver.result_text(cx))
    }

    fn status(&self, cx: &mut TestAppContext) -> Option<String> {
        self.app.label(cx, "resolver-status")
    }

    fn events(&self) -> Vec<ConflictResolverEvent> {
        self.events.borrow().clone()
    }

    /// Lo que guardaría "Mark as resolved".
    fn save(&self, cx: &mut TestAppContext) -> (ConflictResolution, Option<String>) {
        self.events.borrow_mut().clear();
        self.app.click(cx, "mark-resolved");
        let events = self.events();
        let [
            ConflictResolverEvent::Save {
                resolution,
                warning,
            },
        ] = events.as_slice()
        else {
            panic!("expected one save, got {events:?}");
        };
        (resolution.clone(), warning.clone())
    }

    /// Escribe `text` en el resultado al principio de la línea `line` (desde
    /// 0) y deja pasar la espera de la sincronización.
    fn type_at(&self, cx: &mut TestAppContext, line: u32, text: &str) {
        let editor = self
            .resolver
            .read_with(cx, |resolver, _| resolver.editor().clone());
        let text = text.to_string();
        self.app.in_window(cx, move |window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_cursor_position(Position::new(line, 0), window, cx);
                editor.insert(text, window, cx);
            });
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
    }

    fn resolved(&self, cx: &mut TestAppContext) -> usize {
        self.resolver
            .read_with(cx, |resolver, _| resolver.resolved_conflicts())
    }
}

#[gpui_kit::test]
async fn reads_the_stages_and_counts_the_conflicts(cx: &mut TestAppContext) {
    let repo = conflicted_repo("notes.txt", Some(BASE), Some(OURS), Some(THEIRS));
    let resolver = Resolver::with_repo(cx, &repo, "notes.txt");
    let app = &resolver.app;

    assert_eq!(
        app.label(cx, "resolver-file-name").as_deref(),
        Some("Resolve Conflicts: notes.txt")
    );
    assert_eq!(
        app.label(cx, "resolver-kind").as_deref(),
        Some("Modified on both sides")
    );
    assert_eq!(resolver.status(cx).as_deref(), Some("0 of 3 resolved"));
    assert_eq!(
        app.label(cx, "current-conflict").as_deref(),
        Some("Conflict 1 of 3")
    );
    assert!(app.exists(cx, "ours-header-0"));
    assert!(app.exists(cx, "theirs-header-0"));
    // Los cambios de un solo lado ya están en el resultado; los conflictos,
    // con marcadores.
    let result = resolver.result(cx);
    assert!(result.contains("FOUR\n"), "{result}");
    assert!(result.contains("EIGHT\n"), "{result}");
    assert_eq!(result.matches("<<<<<<< Current (ours)").count(), 3);
}

#[gpui_kit::test]
async fn block_buttons_compose_the_result_and_resolve_the_file(cx: &mut TestAppContext) {
    let repo = conflicted_repo("notes.txt", Some(BASE), Some(OURS), Some(THEIRS));
    let resolver = Resolver::with_repo(cx, &repo, "notes.txt");
    let app = &resolver.app;

    app.click(cx, "take-current");
    assert_eq!(resolver.status(cx).as_deref(), Some("1 of 3 resolved"));
    app.click(cx, "conflict-next");
    assert_eq!(
        app.label(cx, "current-conflict").as_deref(),
        Some("Conflict 2 of 3")
    );
    app.click(cx, "take-both-incoming-first");
    app.click(cx, "conflict-next");
    app.click(cx, "take-base");
    assert_eq!(resolver.status(cx).as_deref(), Some("3 of 3 resolved"));

    let expected =
        "one\ntwo main\nthree\nFOUR\nfive\nsix feature\nsix main\nseven\nEIGHT\nnine\nten\n";
    assert_eq!(resolver.result(cx), expected);
    let (resolution, warning) = resolver.save(cx);
    assert_eq!(
        resolution,
        ConflictResolution::Content(expected.to_string())
    );
    assert_eq!(warning, None);

    // Git lo acepta como resolución.
    repo.write("notes.txt", expected);
    repo.git(&["add", "notes.txt"]);
    repo.git(&["commit", "-q", "--no-edit"]);
    assert_eq!(repo.read("notes.txt"), expected);
}

#[gpui_kit::test]
async fn navigation_wraps_around_and_reset_brings_the_markers_back(cx: &mut TestAppContext) {
    let repo = conflicted_repo("notes.txt", Some(BASE), Some(OURS), Some(THEIRS));
    let resolver = Resolver::with_repo(cx, &repo, "notes.txt");
    let app = &resolver.app;

    app.click(cx, "conflict-prev");
    assert_eq!(
        app.label(cx, "current-conflict").as_deref(),
        Some("Conflict 3 of 3")
    );
    app.click(cx, "conflict-next");
    assert_eq!(
        app.label(cx, "current-conflict").as_deref(),
        Some("Conflict 1 of 3")
    );
    // El encabezado de un conflicto en un panel también lo hace actual.
    app.click(cx, "theirs-goto-1");
    assert_eq!(
        app.label(cx, "current-conflict").as_deref(),
        Some("Conflict 2 of 3")
    );

    app.click(cx, "take-incoming");
    assert!(resolver.result(cx).contains("\nsix feature\n"));
    app.click(cx, "reset-conflict");
    assert_eq!(resolver.status(cx).as_deref(), Some("0 of 3 resolved"));
    assert_eq!(
        resolver
            .result(cx)
            .matches("<<<<<<< Current (ours)")
            .count(),
        3
    );
}

#[gpui_kit::test]
async fn line_and_block_checkboxes_pick_lines_from_each_side(cx: &mut TestAppContext) {
    let repo = conflicted_repo(
        "list.txt",
        Some("start\nold\nend\n"),
        Some("start\nours 1\nours 2\nend\n"),
        Some("start\ntheirs 1\ntheirs 2\nend\n"),
    );
    let resolver = Resolver::with_repo(cx, &repo, "list.txt");
    let app = &resolver.app;

    // Líneas sueltas: la segunda de ours y la primera de theirs (los ids
    // llevan el número de línea de cada versión, desde 0).
    app.click(cx, "ours-line-2");
    app.click(cx, "theirs-line-1");
    assert_eq!(resolver.result(cx), "start\nours 2\ntheirs 1\nend\n");
    assert_eq!(resolver.status(cx).as_deref(), Some("1 of 1 resolved"));

    // La casilla de bloque completa un lado; otra vez, lo vacía.
    app.click(cx, "ours-block-0");
    assert_eq!(
        resolver.result(cx),
        "start\nours 1\nours 2\ntheirs 1\nend\n"
    );
    app.click(cx, "ours-block-0");
    assert_eq!(resolver.result(cx), "start\ntheirs 1\nend\n");

    // Desde "ambos con lo entrante primero", quitar una línea mantiene el orden.
    app.click(cx, "take-both-incoming-first");
    app.click(cx, "ours-line-1");
    assert_eq!(
        resolver.result(cx),
        "start\ntheirs 1\ntheirs 2\nours 2\nend\n"
    );
}

#[gpui_kit::test]
async fn edits_in_the_result_are_kept_and_tracked(cx: &mut TestAppContext) {
    let repo = conflicted_repo(
        "list.txt",
        Some("start\nold\nmid\nold2\nend\n"),
        Some("start\nours\nmid\nours2\nend\n"),
        Some("start\ntheirs\nmid\ntheirs2\nend\n"),
    );
    let resolver = Resolver::with_repo(cx, &repo, "list.txt");
    let app = &resolver.app;
    assert_eq!(resolver.status(cx).as_deref(), Some("0 of 2 resolved"));

    // Una línea nueva al principio: no toca los conflictos, pero los
    // desplaza; el botón sigue reemplazando el tramo correcto.
    resolver.type_at(cx, 0, "header\n");
    assert_eq!(resolver.resolved(cx), 0);
    app.click(cx, "take-current");
    assert_eq!(
        resolver.result(cx),
        "header\nstart\nours\nmid\n<<<<<<< Current (ours)\nours2\n=======\ntheirs2\n>>>>>>> Incoming (theirs)\nend\n"
    );

    // Escribir dentro del segundo conflicto sin quitar marcadores no lo
    // resuelve…
    resolver.type_at(cx, 6, "typed ");
    assert_eq!(resolver.resolved(cx), 1);
    // …y "Mark as resolved" avisa de lo que queda.
    let (_, warning) = resolver.save(cx);
    assert!(
        warning
            .as_deref()
            .is_some_and(|warning| warning.contains("1 conflict(s)")),
        "{warning:?}"
    );

    // Elegir un lado de ese conflicto reemplaza lo escrito.
    app.click(cx, "conflict-next");
    app.click(cx, "take-incoming");
    assert_eq!(
        resolver.result(cx),
        "header\nstart\nours\nmid\ntheirs2\nend\n"
    );
    let (resolution, warning) = resolver.save(cx);
    assert_eq!(warning, None);
    assert_eq!(
        resolution,
        ConflictResolution::Content("header\nstart\nours\nmid\ntheirs2\nend\n".to_string())
    );
}

#[gpui_kit::test]
async fn resolving_by_hand_counts_once_the_markers_are_gone(cx: &mut TestAppContext) {
    let resolver = Resolver::with_markers(
        cx,
        "before\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> feature\nafter\n",
    );
    let app = &resolver.app;
    assert_eq!(
        app.label(cx, "resolver-kind").as_deref(),
        Some("From conflict markers")
    );
    assert_eq!(
        resolver.result(cx),
        "before\n<<<<<<< Current (ours)\nmine\n=======\ntheirs\n>>>>>>> Incoming (theirs)\nafter\n"
    );

    // Se borran a mano las cinco líneas del conflicto y se escribe otra.
    let editor = resolver
        .resolver
        .read_with(cx, |resolver, _| resolver.editor().clone());
    resolver.app.in_window(cx, move |window, cx| {
        editor.update(cx, |editor, cx| {
            let start = "before\n".len();
            let end = start
                + "<<<<<<< Current (ours)\nmine\n=======\ntheirs\n>>>>>>> Incoming (theirs)\n"
                    .len();
            editor.set_selected_range(start..end, cx);
            editor.replace("merged by hand\n", window, cx);
        });
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();

    assert_eq!(resolver.status(cx).as_deref(), Some("1 of 1 resolved"));
    let (resolution, warning) = resolver.save(cx);
    assert_eq!(
        resolution,
        ConflictResolution::Content("before\nmerged by hand\nafter\n".to_string())
    );
    assert_eq!(warning, None);
}

#[gpui_kit::test]
async fn saving_with_markers_left_asks_first(cx: &mut TestAppContext) {
    let repo = conflicted_repo("notes.txt", Some(BASE), Some(OURS), Some(THEIRS));
    let resolver = Resolver::with_repo(cx, &repo, "notes.txt");

    let (resolution, warning) = resolver.save(cx);
    let ConflictResolution::Content(content) = resolution else {
        panic!("a text resolution");
    };
    assert_eq!(content.matches("<<<<<<<").count(), 3);
    assert_eq!(
        warning.as_deref(),
        Some(
            "3 conflict(s) in notes.txt are still unresolved and keep their conflict markers. Mark it as resolved anyway?"
        )
    );
}

#[gpui_kit::test]
async fn keeps_crlf_and_a_missing_final_newline_when_saving(cx: &mut TestAppContext) {
    let repo = conflicted_repo(
        "win.txt",
        Some("alpha\r\nbase\r\nomega"),
        Some("alpha\r\nmain\r\nomega"),
        Some("alpha\r\nfeature\r\nomega"),
    );
    let resolver = Resolver::with_repo(cx, &repo, "win.txt");
    let app = &resolver.app;
    // El editor trabaja en LF.
    app.click(cx, "take-both-current-first");
    assert_eq!(resolver.result(cx), "alpha\nmain\nfeature\nomega\n");

    let (resolution, warning) = resolver.save(cx);
    assert_eq!(warning, None);
    assert_eq!(
        resolution,
        ConflictResolution::Content("alpha\r\nmain\r\nfeature\r\nomega".to_string())
    );
}

#[gpui_kit::test]
async fn an_add_add_conflict_has_no_base_to_take(cx: &mut TestAppContext) {
    let repo = conflicted_repo(
        "new.txt",
        None,
        Some("same\nmine\nend\n"),
        Some("same\ntheirs\nend\n"),
    );
    let resolver = Resolver::with_repo(cx, &repo, "new.txt");
    let app = &resolver.app;
    assert_eq!(
        app.label(cx, "resolver-kind").as_deref(),
        Some("Added on both sides")
    );
    assert_eq!(resolver.status(cx).as_deref(), Some("0 of 1 resolved"));

    // Sin base, "Take base" no hace nada.
    app.click(cx, "take-base");
    assert_eq!(resolver.resolved(cx), 0);
    app.click(cx, "take-incoming");
    assert_eq!(resolver.result(cx), "same\ntheirs\nend\n");
}

#[gpui_kit::test]
async fn the_base_pane_folds_and_unfolds(cx: &mut TestAppContext) {
    let repo = conflicted_repo("notes.txt", Some(BASE), Some(OURS), Some(THEIRS));
    let resolver = Resolver::with_repo(cx, &repo, "notes.txt");
    let app = &resolver.app;

    assert!(!app.exists(cx, "pane-base-frame"));
    app.click(cx, "toggle-base");
    assert!(app.exists(cx, "pane-base-frame"));
    assert!(app.exists(cx, "base-header-0"));
    app.click(cx, "base-take-0");
    assert!(resolver.result(cx).starts_with("one\ntwo\nthree\n"));
    app.click(cx, "toggle-base");
    assert!(!app.exists(cx, "pane-base-frame"));
}

#[gpui_kit::test]
async fn a_delete_modify_conflict_keeps_or_deletes_the_file(cx: &mut TestAppContext) {
    let repo = conflicted_repo("gone.txt", Some("base\n"), None, Some("changed\n"));
    let resolver = Resolver::with_repo(cx, &repo, "gone.txt");
    let app = &resolver.app;

    assert_eq!(
        app.label(cx, "resolver-kind").as_deref(),
        Some("Deleted in current, modified in incoming")
    );
    // Sin editor: se elige entero, y hasta elegir no se puede guardar.
    assert!(!app.exists(cx, "conflict-result"));
    assert!(app.exists(cx, "pane-theirs-frame"));
    app.click(cx, "mark-resolved");
    assert!(resolver.events().is_empty());

    resolver.resolver.update(cx, |resolver, cx| {
        resolver.choose_whole(WholeChoice::Delete, cx)
    });
    let (resolution, warning) = resolver.save(cx);
    assert_eq!(resolution, ConflictResolution::Delete);
    assert_eq!(
        warning.as_deref(),
        Some("gone.txt will be deleted and the conflict marked as resolved.")
    );

    resolver.resolver.update(cx, |resolver, cx| {
        resolver.choose_whole(WholeChoice::Keep(ConflictSide::Theirs), cx)
    });
    assert_eq!(
        resolver.save(cx),
        (ConflictResolution::Keep(ConflictSide::Theirs), None)
    );
}

#[gpui_kit::test]
async fn a_binary_conflict_chooses_a_whole_version(cx: &mut TestAppContext) {
    let repo = TestRepo::empty();
    let write = |bytes: &[u8]| std::fs::write(repo.path.join("logo.bin"), bytes).unwrap();
    write(b"\x89PNG\0base");
    repo.git(&["add", "logo.bin"]);
    repo.git(&["commit", "-q", "-m", "logo"]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    write(b"\x89PNG\0feature");
    repo.git(&["commit", "-q", "-am", "feature logo"]);
    repo.git(&["checkout", "-q", "main"]);
    write(b"\x89PNG\0main");
    repo.git(&["commit", "-q", "-am", "main logo"]);
    assert!(repo.try_git(&["merge", "feature"]).is_err());

    let resolver = Resolver::with_repo(cx, &repo, "logo.bin");
    let app = &resolver.app;
    assert_eq!(
        app.label(cx, "resolver-kind").as_deref(),
        Some("Binary · Modified on both sides")
    );
    assert!(!app.exists(cx, "conflict-result"));
    assert!(!app.exists(cx, "pane-ours-frame"));
    assert_eq!(
        resolver.status(cx).as_deref(),
        Some("Choose a version to keep")
    );
    resolver.resolver.update(cx, |resolver, cx| {
        resolver.choose_whole(WholeChoice::Keep(ConflictSide::Ours), cx)
    });
    assert_eq!(
        resolver.save(cx),
        (ConflictResolution::Keep(ConflictSide::Ours), None)
    );
}

#[gpui_kit::test]
async fn keeps_what_was_decided_in_each_file_while_switching(cx: &mut TestAppContext) {
    let repo = TestRepo::empty();
    repo.commit_file("a.txt", "a\nbase\n", "base a");
    repo.commit_file("b.txt", "b\nbase\n", "base b");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("a.txt", "a\nfeature\n", "feature a");
    repo.commit_file("b.txt", "b\nfeature\n", "feature b");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("a.txt", "a\nmain\n", "main a");
    repo.commit_file("b.txt", "b\nmain\n", "main b");
    assert!(repo.try_git(&["merge", "feature"]).is_err());

    let resolver = Resolver::mount(cx, &["a.txt", "b.txt"]);
    let app = &resolver.app;
    resolver.load(cx, "a.txt", versions_of(&repo, "a.txt"));
    app.click(cx, "take-incoming");
    assert_eq!(resolver.result(cx), "a\nfeature\n");

    resolver.load(cx, "b.txt", versions_of(&repo, "b.txt"));
    assert_eq!(resolver.status(cx).as_deref(), Some("0 of 1 resolved"));
    assert!(resolver.result(cx).starts_with("b\n<<<<<<<"));

    // De vuelta en a.txt, lo decidido sigue ahí aunque lleguen las mismas
    // versiones otra vez.
    resolver.load(cx, "a.txt", versions_of(&repo, "a.txt"));
    assert_eq!(resolver.status(cx).as_deref(), Some("1 of 1 resolved"));
    assert_eq!(resolver.result(cx), "a\nfeature\n");
}

#[gpui_kit::test]
async fn emits_select_file_when_the_user_chooses_another_conflict_file(cx: &mut TestAppContext) {
    let resolver = Resolver::mount(cx, &["src/a.js", "src/b.js"]);

    assert_eq!(
        resolver.app.label(cx, "conflict-file:src/b.js").as_deref(),
        Some("src/b.js")
    );
    resolver.app.click(cx, "conflict-file:src/b.js");
    assert_eq!(
        resolver.events(),
        [ConflictResolverEvent::SelectFile("src/b.js".to_string())]
    );
}

/// Un merge grande deja cientos de archivos en conflicto: la lista es
/// virtual, solo monta las filas a la vista, y elegir uno lejano lo trae a la
/// vista sin perder el clic.
#[gpui_kit::test]
async fn lists_hundreds_of_conflict_files_mounting_only_the_visible_rows(cx: &mut TestAppContext) {
    let paths: Vec<String> = (0..250).map(|ix| format!("src/file-{ix:03}.js")).collect();
    let files: Vec<&str> = paths.iter().map(String::as_str).collect();
    let resolver = Resolver::mount(cx, &files);
    let app = &resolver.app;
    let mounted = |cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| {
            paths
                .iter()
                .filter(|path| {
                    window
                        .try_find(ElementId::from(SharedString::from(format!(
                            "conflict-file:{path}"
                        ))))
                        .is_some()
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    };

    let visible = mounted(cx);
    assert!(
        visible.first().map(String::as_str) == Some("src/file-000.js"),
        "{visible:?}"
    );
    assert!(
        !visible.is_empty() && visible.len() < 60,
        "only the rows in view are mounted, got {}",
        visible.len()
    );
    assert!(!visible.iter().any(|path| path == "src/file-249.js"));

    // Clic en una fila a la vista.
    app.click(cx, "conflict-file:src/file-003.js");
    assert_eq!(
        resolver.events(),
        [ConflictResolverEvent::SelectFile(
            "src/file-003.js".to_string()
        )]
    );

    // Elegido desde fuera (la lista de cambios), uno del final: la lista lo
    // trae a la vista.
    resolver.resolver.update(cx, |resolver, cx| {
        resolver.set_versions("src/file-240.js", None, cx);
    });
    let visible = mounted(cx);
    assert!(
        visible.iter().any(|path| path == "src/file-240.js"),
        "{visible:?}"
    );
    assert!(!visible.iter().any(|path| path == "src/file-000.js"));
    assert!(visible.len() < 60, "{}", visible.len());

    app.click(cx, "conflict-file:src/file-240.js");
    assert_eq!(
        resolver.events().last(),
        Some(&ConflictResolverEvent::SelectFile(
            "src/file-240.js".to_string()
        ))
    );
}

#[gpui_kit::test]
async fn reads_diff3_markers_with_their_base_when_there_are_no_stages(cx: &mut TestAppContext) {
    let resolver = Resolver::with_markers(
        cx,
        "before\n<<<<<<< HEAD\nmine\n||||||| base\nancestor\n=======\ntheirs\n>>>>>>> feature\nafter",
    );
    resolver.app.click(cx, "take-base");
    let (resolution, warning) = resolver.save(cx);
    assert_eq!(warning, None);
    // Sin salto final, como el archivo.
    assert_eq!(
        resolution,
        ConflictResolution::Content("before\nancestor\nafter".to_string())
    );
}

/// Un archivo largo: las filas de los paneles son virtuales y la navegación
/// lleva a un conflicto que está a miles de líneas.
#[gpui_kit::test]
async fn a_long_file_scrolls_to_a_conflict_far_below(cx: &mut TestAppContext) {
    let head: String = (0..20_000).map(|line| format!("line {line}\n")).collect();
    let base = format!("{head}middle\nend\n");
    let ours = format!("{head}middle ours\nend\n");
    let theirs = format!("{head}middle theirs\nend\n");
    let repo = conflicted_repo("big.txt", Some(&base), Some(&ours), Some(&theirs));
    let resolver = Resolver::with_repo(cx, &repo, "big.txt");
    let app = &resolver.app;

    // Los paneles ya están en el conflicto, a 20 000 líneas.
    assert!(app.exists(cx, "ours-header-0"));
    assert!(!app.exists(cx, ("ours", 0usize)));
    app.click(cx, "ours-line-20000");
    assert_eq!(resolver.status(cx).as_deref(), Some("1 of 1 resolved"));
    assert!(
        resolver
            .result(cx)
            .ends_with("line 19999\nmiddle ours\nend\n")
    );
}

/// Medida a mano (`cargo test --release -- --ignored large_conflict_timings
/// --nocapture`): un conflicto de ~5 MiB con cientos de bloques, del
/// análisis a elegir, escribir y guardar.
#[gpui_kit::test]
#[ignore]
async fn large_conflict_timings(cx: &mut TestAppContext) {
    use std::time::Instant;
    let mut base = String::new();
    let mut ours = String::new();
    let mut theirs = String::new();
    for line in 0..85_000 {
        let text = format!("    let value_{line} = compute({line}, \"some payload text\");\n");
        base.push_str(&text);
        if line % 400 == 0 {
            ours.push_str(&format!("    let value_{line} = ours({line});\n"));
            theirs.push_str(&format!("    let value_{line} = theirs({line});\n"));
        } else {
            ours.push_str(&text);
            theirs.push_str(&text);
        }
    }
    eprintln!("size: {} bytes", ours.len());
    let repo = conflicted_repo("big.rs", Some(&base), Some(&ours), Some(&theirs));
    let resolver = Resolver::mount(cx, &["big.rs"]);

    let started = Instant::now();
    let versions = versions_of(&repo, "big.rs");
    eprintln!("read stages: {:?}", started.elapsed());
    let started = Instant::now();
    resolver.load(cx, "big.rs", versions);
    eprintln!("analysis + editor: {:?}", started.elapsed());
    assert_eq!(resolver.status(cx).as_deref(), Some("0 of 213 resolved"));

    let started = Instant::now();
    for _ in 0..10 {
        resolver.app.in_window(cx, |_, _| {});
    }
    eprintln!("10 frames: {:?}", started.elapsed());
    let started = Instant::now();
    resolver.app.click(cx, "take-current");
    eprintln!("take current: {:?}", started.elapsed());
    let started = Instant::now();
    resolver.app.click(cx, "conflict-next");
    eprintln!("next conflict: {:?}", started.elapsed());
    let started = Instant::now();
    resolver.type_at(cx, 40_000, "// typed\n");
    eprintln!("type + resync: {:?}", started.elapsed());
    let started = Instant::now();
    let (_, warning) = resolver.save(cx);
    eprintln!("save: {:?}", started.elapsed());
    assert!(warning.is_some());
}
