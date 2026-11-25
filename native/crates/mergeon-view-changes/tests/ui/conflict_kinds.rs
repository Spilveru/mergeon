//! Todas las clases de conflicto que deja un merge (`UU`, `AA` de texto y
//! binario, `UD`, `DU`, `DD`, `AU`, `UA`), en la vista de cambios sobre un
//! repositorio real: cada archivo sale una sola vez, en "Conflicted files", y
//! se resuelve desde el resolvedor.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use mergeon_git_ops::ConflictSide;
use mergeon_view_changes::{ChangesMode, ChangesView, WholeChoice};

use crate::support::{TestApp, TestRepo};

/// Los archivos en conflicto de [`repo_with_every_conflict`], con su código.
const CONFLICTS: [(&str, &str); 9] = [
    ("a.txt", "DD"),
    ("b.txt", "AU"),
    ("both.txt", "AA"),
    ("c.txt", "UA"),
    ("gone.txt", "DU"),
    ("icon.bin", "AA"),
    ("kept.txt", "UD"),
    ("logo.bin", "UU"),
    ("notes.txt", "UU"),
];

fn write_bytes(repo: &TestRepo, path: &str, bytes: &[u8]) {
    std::fs::write(repo.path.join(path), bytes).expect("write file");
}

/// Un merge de `feature` en `main` parado con un conflicto de cada clase:
///
/// - `a.txt` lo renombran los dos a nombres distintos: `DD a.txt`,
///   `AU b.txt` (el de `main`) y `UA c.txt` (el de `feature`);
/// - `both.txt` (texto) e `icon.bin` (binario) los añaden los dos: `AA`;
/// - `gone.txt` lo borra `main` y lo cambia `feature`: `DU`;
/// - `kept.txt` lo cambia `main` y lo borra `feature`: `UD`;
/// - `logo.bin` (binario) y `notes.txt` los cambian los dos: `UU`.
fn repo_with_every_conflict() -> TestRepo {
    let repo = TestRepo::new();
    let lines: String = (1..=20).map(|line| format!("line {line}\n")).collect();
    repo.write("a.txt", &lines);
    repo.write("kept.txt", "base\n");
    repo.write("gone.txt", "base\n");
    repo.write("notes.txt", "base\n");
    write_bytes(&repo, "logo.bin", b"\x89PNG\0base");
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "base"]);

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.git(&["mv", "a.txt", "c.txt"]);
    repo.git(&["rm", "-q", "kept.txt"]);
    repo.write("gone.txt", "feature\n");
    repo.write("notes.txt", "feature\n");
    repo.write("both.txt", "same\ntheirs\nend\n");
    write_bytes(&repo, "logo.bin", b"\x89PNG\0feature!!");
    write_bytes(&repo, "icon.bin", b"\x89PNG\0feat");
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "feature"]);

    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["mv", "a.txt", "b.txt"]);
    repo.write("kept.txt", "main\n");
    repo.git(&["rm", "-q", "gone.txt"]);
    repo.write("notes.txt", "main\n");
    repo.write("both.txt", "same\nmine\nend\n");
    write_bytes(&repo, "logo.bin", b"\x89PNG\0main");
    write_bytes(&repo, "icon.bin", b"\x89PNG\0mainicon");
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "main"]);
    assert!(
        repo.try_git(&["merge", "feature"]).is_err(),
        "the merge should stop on conflicts"
    );

    let mut status: Vec<String> = repo
        .git(&["status", "--porcelain"])
        .lines()
        .map(str::to_string)
        .collect();
    status.sort();
    let mut expected: Vec<String> = CONFLICTS
        .iter()
        .map(|(path, code)| format!("{code} {path}"))
        .collect();
    expected.sort();
    assert_eq!(
        status, expected,
        "the fixture should hold every conflict kind"
    );
    repo
}

fn pending(app: &TestApp, cx: &mut TestAppContext, repo_id: &str) -> Vec<String> {
    app.model.read_with(cx, |model, _| {
        model
            .repository(repo_id)
            .map(|repository| repository.conflict_state.pending_files.clone())
            .unwrap_or_default()
    })
}

/// Elige `path` en la lista y espera a que el resolvedor lo tenga analizado.
fn open_conflict(app: &TestApp, view: &Entity<ChangesView>, cx: &mut TestAppContext, path: &str) {
    app.click(cx, format!("conflict:file:{path}"));
    let resolver = view.read_with(cx, |view, _| view.conflict_resolver().clone());
    let path = path.to_string();
    app.wait_until(cx, &format!("the analysis of {path}"), move |cx| {
        resolver.read_with(cx, |resolver, _| {
            !resolver.is_loading()
                && resolver
                    .document()
                    .is_some_and(|document| document.path == path)
        })
    });
    app.in_window(cx, |_, _| {});
}

/// "Mark as resolved" y espera a que `path` deje de estar pendiente.
fn mark_resolved(
    app: &TestApp,
    view: &Entity<ChangesView>,
    cx: &mut TestAppContext,
    repo_id: &str,
    path: &str,
) {
    app.click(cx, "mark-resolved");
    app.wait_until(cx, &format!("{path} to be resolved"), |cx| {
        !pending(app, cx, repo_id).iter().any(|file| file == path)
    });
    app.settle(cx, view);
}

fn choose(view: &Entity<ChangesView>, cx: &mut TestAppContext, choice: WholeChoice) {
    let resolver = view.read_with(cx, |view, _| view.conflict_resolver().clone());
    resolver.update(cx, |resolver, cx| resolver.choose_whole(choice, cx));
}

#[gpui_kit::test]
async fn every_kind_of_conflict_is_listed_once_and_resolved(cx: &mut TestAppContext) {
    let app = TestApp::with_size(1400., 1000., cx);
    let repo = repo_with_every_conflict();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    app.settle(cx, &view);

    // Una sola vez, en "Conflicted files": ni en Unstaged ni en Staged.
    for (path, code) in CONFLICTS {
        assert!(
            app.exists(cx, format!("conflict:file:{path}")),
            "{code} {path} should be a conflict"
        );
        assert!(!app.exists(cx, format!("unstaged:file:{path}")), "{path}");
        assert!(!app.exists(cx, format!("staged:file:{path}")), "{path}");
    }
    assert_eq!(pending(&app, cx, &repo_id).len(), CONFLICTS.len());
    assert_eq!(app.label(cx, "conflict-progress").as_deref(), Some("0 / 9"));

    // UU de texto: el resolvedor de tres vías.
    open_conflict(&app, &view, cx, "notes.txt");
    app.click(cx, "take-incoming");
    mark_resolved(&app, &view, cx, &repo_id, "notes.txt");
    assert_eq!(repo.read("notes.txt"), "feature\n");
    assert_eq!(app.label(cx, "conflict-progress").as_deref(), Some("1 / 9"));

    // AA de texto: el mismo resolvedor, con la base vacía (las líneas
    // comunes ya vienen fusionadas).
    open_conflict(&app, &view, cx, "both.txt");
    assert_eq!(
        app.label(cx, "resolver-kind").as_deref(),
        Some("Added on both sides")
    );
    app.click(cx, "take-current");
    mark_resolved(&app, &view, cx, &repo_id, "both.txt");
    assert_eq!(repo.read("both.txt"), "same\nmine\nend\n");

    // UU binario: una versión entera, con su tamaño.
    open_conflict(&app, &view, cx, "logo.bin");
    let options = view.read_with(cx, |view, cx| {
        view.conflict_resolver().read(cx).whole_options()
    });
    let summary: Vec<(&str, &str)> = options
        .iter()
        .map(|option| (option.label.as_str(), option.detail.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("Keep current (ours)", "From main · 9 B"),
            ("Keep incoming (theirs)", "From feature · 14 B"),
            ("Keep base (common ancestor)", "9 B"),
        ]
    );
    choose(&view, cx, WholeChoice::Keep(ConflictSide::Theirs));
    mark_resolved(&app, &view, cx, &repo_id, "logo.bin");
    assert_eq!(
        std::fs::read(repo.path.join("logo.bin")).unwrap(),
        b"\x89PNG\0feature!!"
    );

    // AA binario: sin base que ofrecer.
    open_conflict(&app, &view, cx, "icon.bin");
    let options = view.read_with(cx, |view, cx| {
        view.conflict_resolver().read(cx).whole_options()
    });
    assert_eq!(options.len(), 2);
    choose(&view, cx, WholeChoice::Keep(ConflictSide::Ours));
    mark_resolved(&app, &view, cx, &repo_id, "icon.bin");
    assert_eq!(
        std::fs::read(repo.path.join("icon.bin")).unwrap(),
        b"\x89PNG\0mainicon"
    );

    // UD: lo cambió `main` y lo borró `feature`.
    open_conflict(&app, &view, cx, "kept.txt");
    let (title, description, options) = view.read_with(cx, |view, cx| {
        let resolver = view.conflict_resolver().read(cx);
        let (title, description) = resolver.whole_text().unwrap();
        (title, description, resolver.whole_options())
    });
    assert_eq!(title, "Modified in main, deleted in feature");
    assert_eq!(
        description,
        "feature (incoming) deleted this file and main (current) changed it. Keep the modified file, or delete it."
    );
    let labels: Vec<&str> = options.iter().map(|option| option.label.as_str()).collect();
    assert_eq!(labels, ["Keep the modified file", "Delete the file"]);
    assert_eq!(options[0].choice, WholeChoice::Keep(ConflictSide::Ours));
    assert_eq!(options[0].detail, "From main · 5 B");
    assert_eq!(options[1].detail, "As feature did");
    choose(&view, cx, WholeChoice::Keep(ConflictSide::Ours));
    mark_resolved(&app, &view, cx, &repo_id, "kept.txt");
    assert_eq!(repo.read("kept.txt"), "main\n");

    // DU: lo borró `main` y lo cambió `feature`; se borra, con confirmación.
    open_conflict(&app, &view, cx, "gone.txt");
    let (title, options) = view.read_with(cx, |view, cx| {
        let resolver = view.conflict_resolver().read(cx);
        (resolver.whole_text().unwrap().0, resolver.whole_options())
    });
    assert_eq!(title, "Deleted in main, modified in feature");
    assert_eq!(options[0].choice, WholeChoice::Keep(ConflictSide::Theirs));
    choose(&view, cx, WholeChoice::Delete);
    app.host.answer_confirm(true);
    mark_resolved(&app, &view, cx, &repo_id, "gone.txt");
    assert!(!repo.path.join("gone.txt").exists());
    assert_eq!(app.host.confirms().len(), 1);

    // DD: ya viene elegido "Delete the file" y no hay nada que confirmar.
    open_conflict(&app, &view, cx, "a.txt");
    let (choice, status) = view.read_with(cx, |view, cx| {
        let resolver = view.conflict_resolver().read(cx);
        (resolver.whole_choice(), resolver.status_text())
    });
    assert_eq!(choice, Some(WholeChoice::Delete));
    assert_eq!(status.as_ref(), "Ready to mark as resolved");
    mark_resolved(&app, &view, cx, &repo_id, "a.txt");
    assert_eq!(
        app.host.confirms().len(),
        1,
        "deleting a DD file asks nothing"
    );

    // AU / UA (los dos nombres nuevos): quedarse con uno y borrar el otro.
    open_conflict(&app, &view, cx, "b.txt");
    choose(&view, cx, WholeChoice::Keep(ConflictSide::Ours));
    mark_resolved(&app, &view, cx, &repo_id, "b.txt");
    open_conflict(&app, &view, cx, "c.txt");
    choose(&view, cx, WholeChoice::Delete);
    app.host.answer_confirm(true);
    mark_resolved(&app, &view, cx, &repo_id, "c.txt");

    assert!(pending(&app, cx, &repo_id).is_empty());
    assert!(
        repo.git(&["diff", "--name-only", "--diff-filter=U"])
            .is_empty()
    );
    assert!(repo.path.join("b.txt").exists());
    assert!(!repo.path.join("a.txt").exists());
    assert!(!repo.path.join("c.txt").exists());
    let ready = app.model.read_with(cx, |model, _| {
        model
            .repository(repo_id.as_str())
            .is_some_and(|repository| repository.conflict_state.ready_to_continue)
    });
    assert!(ready, "the merge can be continued");
    // Y lo resuelto queda preparado para el commit del merge.
    assert!(app.exists(cx, "staged:file:notes.txt"));
    // Un merge todavía en curso: lo que se resolvió no vuelve a la lista de
    // conflictos.
    for (path, _) in CONFLICTS {
        assert!(!app.exists(cx, format!("conflict:file:{path}")), "{path}");
    }
}

/// `UD` en la lista: Espacio no lo "prepara" a ciegas (se resuelve en el
/// resolvedor) y su fila no está repetida en las otras secciones.
#[gpui_kit::test]
async fn space_on_a_conflict_does_not_stage_it(cx: &mut TestAppContext) {
    let app = TestApp::with_size(1400., 1000., cx);
    let repo = repo_with_every_conflict();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "conflict:file:kept.txt");
    app.settle(cx, &view);
    app.in_window(cx, |window, cx| window.press("space", cx));
    app.settle(cx, &view);

    assert!(
        pending(&app, cx, &repo_id)
            .iter()
            .any(|file| file == "kept.txt")
    );
    assert!(app.exists(cx, "conflict:file:kept.txt"));
}
