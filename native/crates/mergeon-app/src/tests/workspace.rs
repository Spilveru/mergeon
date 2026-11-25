//! Estado de pestañas por repositorio (`RepositoryWorkspace.vue`). La lógica
//! de las pestañas se prueba en `mergeon-model`; aquí, que el modelo la aplica,
//! la guarda en la sesión y avisa.

use gpui_kit::TestAppContext;
use mergeon_model::workspace_tabs::{
    CHANGES_TAB, FileTabDescriptor, HOME_TAB, Scroll, WorkspaceSection,
};

use super::support::{TestApp, TestRepo};
use crate::AppEvent;
use crate::workspace::TabNavigation;

fn stored_workspace(app: &TestApp, path: &str) -> serde_json::Value {
    app.backend
        .storage()
        .get_workspace_session()
        .repository_workspaces
        .get(path)
        .cloned()
        .unwrap_or_default()
}

#[gpui_kit::test]
async fn opens_activates_and_closes_file_tabs_and_keeps_them_in_the_session(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.seed(&repo, cx);

    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        assert_eq!(workspace.active_tab, HOME_TAB);
        assert!(workspace.tabs.is_empty());
    });

    let readme = FileTabDescriptor::working("README.md");
    let notes = FileTabDescriptor::working("notes.txt");
    let (readme_id, notes_id) = (readme.id(), notes.id());

    app.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, CHANGES_TAB, cx);
        assert_eq!(model.open_workspace_file(&path, &readme, false, cx), None);
        assert_eq!(model.open_workspace_file(&path, &notes, false, cx), None);
    });
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        assert_eq!(workspace.section, WorkspaceSection::Changes);
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.active_tab, notes_id);
        // La sección fija visible es también la vista activa de la app.
        assert_eq!(model.active_view(), WorkspaceSection::Changes);
    });
    let stored = stored_workspace(&app, &path);
    assert_eq!(stored["activeTab"], notes_id.as_str());
    assert_eq!(stored["section"], "changes");
    assert_eq!(stored["tabs"].as_array().map(Vec::len), Some(2));

    app.model.update(cx, |model, cx| {
        model.close_workspace_tab(&path, &notes_id, cx);
    });
    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        assert_eq!(workspace.tabs.len(), 1);
        // Queda otro documento: pasa a él, no a la vista fija.
        assert_eq!(workspace.active_tab, readme_id);
    });

    // `mod+w` cierra la pestaña de archivo activa, nunca una fija.
    app.model.update(cx, |model, cx| {
        model.close_active_workspace_tab(&path, cx);
        let workspace = model.repository_workspace(&path);
        assert!(workspace.tabs.is_empty());
        // Era el último: se vuelve a la vista fija desde la que se abrió.
        assert_eq!(workspace.active_tab, CHANGES_TAB);
        model.close_active_workspace_tab(&path, cx);
        assert_eq!(model.repository_workspace(&path).active_tab, CHANGES_TAB);
    });
    cx.run_until_parked();
    app.model.read_with(cx, |model, _| {
        assert!(model.repository_workspace(&path).tabs.is_empty());
    });
    assert_eq!(
        stored_workspace(&app, &path)["tabs"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
}

#[gpui_kit::test]
async fn a_click_inside_a_file_tab_reuses_it_and_announces_the_replacement(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.seed(&repo, cx);
    let first = FileTabDescriptor::working("a.txt");
    let second = FileTabDescriptor::working("b.txt");

    let moved = app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &first, false, cx);
        model.open_workspace_file(&path, &second, true, cx)
    });

    let moved = moved.expect("the tab changed identity");
    assert_eq!(moved.from, first.id());
    assert_eq!(moved.to, second.id());
    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_tab, second.id());
    });
    assert!(app.events().contains(&AppEvent::WorkspaceTabReplaced {
        repo_path: path.clone(),
        from: first.id(),
        to: second.id(),
    }));

    // El doble clic llega después de sus dos clics: la pestaña recupera su
    // archivo y el nuevo se abre aparte.
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &second, false, cx);
    });
    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        let ids: Vec<_> = workspace.tabs.iter().map(|tab| tab.id.clone()).collect();
        assert_eq!(ids, [first.id(), second.id()]);
        assert_eq!(workspace.active_tab, second.id());
    });
}

#[gpui_kit::test]
async fn navigates_tabs_with_the_keyboard_and_remembers_scroll_and_selection(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.seed(&repo, cx);
    let file = FileTabDescriptor::working("a.txt");
    let file_id = file.id();

    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &file, false, cx);

        // commits, changes, a.txt
        assert_eq!(
            model.navigate_workspace_tabs(&path, TabNavigation::Next, cx),
            HOME_TAB
        );
        assert_eq!(
            model.navigate_workspace_tabs(&path, TabNavigation::Previous, cx),
            file_id
        );
        assert_eq!(
            model.navigate_workspace_tabs(&path, TabNavigation::First, cx),
            HOME_TAB
        );
        assert_eq!(
            model.navigate_workspace_tabs(&path, TabNavigation::Next, cx),
            CHANGES_TAB
        );
        assert_eq!(
            model.navigate_workspace_tabs(&path, TabNavigation::Last, cx),
            file_id
        );

        model.remember_workspace_scroll(
            &path,
            &file_id,
            Scroll {
                top: 120.0,
                left: 8.0,
            },
            cx,
        );
        model.set_workspace_selected_commit(&path, Some("abc123"), cx);
        model.show_workspace_home(&path, None, cx);
    });
    app.model
        .update(cx, |model, cx| model.flush_workspaces(cx))
        .await;

    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        assert_eq!(workspace.tabs[0].scroll.top, 120.0);
        assert_eq!(workspace.selected_commit.as_deref(), Some("abc123"));
        // `showHome()` vuelve a la última vista fija que se vio.
        assert_eq!(workspace.active_tab, CHANGES_TAB);
    });
    let stored = stored_workspace(&app, &path);
    assert_eq!(stored["selectedCommit"], "abc123");
    assert_eq!(stored["tabs"][0]["scroll"]["top"], 120.0);
    assert_eq!(stored["activeTab"], CHANGES_TAB);
}

/// "File history" y el blame llevan a un commit: el repositorio pasa a ser
/// el activo, con su pestaña Commits a la vista, y la vista de commits recibe
/// la petición.
#[gpui_kit::test]
async fn showing_a_commit_activates_the_history_of_its_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.seed(&repo, cx);
    let other = TestRepo::new();
    let other_path = app.seed(&other, cx);

    app.model.update(cx, |model, cx| {
        let index = model.find_repository_index(other_path.as_str());
        model.set_active_repository(index, cx);
        model.activate_workspace_tab(&path, CHANGES_TAB, cx);
        let history = FileTabDescriptor::history("README.md", None);
        model.open_workspace_file(&path, &history, false, cx);
        assert!(!model.show_commit_in_history(&path, "  ", cx));
        assert!(!model.show_commit_in_history("/not/open", "abc", cx));
        assert!(model.show_commit_in_history(&path, "abc123", cx));
    });
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().map(|repo| repo.id.clone()),
            Some(path.clone())
        );
        assert_eq!(model.repository_workspace(&path).active_tab, HOME_TAB);
        assert_eq!(model.active_view(), WorkspaceSection::Commits);
    });
    assert!(app.events().contains(&AppEvent::CommitSelectionRequested {
        repo_id: path.clone(),
        hash: "abc123".into(),
    }));
}

/// Desde Commits los archivos comparten una pestaña de vista previa; fijarla,
/// arrastrarla y los cierres del menú contextual; `Alt+←/→` recorre los
/// documentos del repositorio activo.
#[gpui_kit::test]
async fn previews_pins_moves_closes_and_walks_document_tabs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.seed(&repo, cx);
    app.model.update(cx, |model, cx| {
        let index = model.find_repository_index(path.as_str());
        model.set_active_repository(index, cx);
    });
    let hash = "a".repeat(40);
    let one = FileTabDescriptor::commit("src/one.rs", &hash);
    let two = FileTabDescriptor::commit("src/two.rs", &hash);
    let ids = |app: &TestApp, cx: &mut TestAppContext| {
        app.model.read_with(cx, |model, _| {
            model
                .repository_workspace(&path)
                .tabs
                .iter()
                .map(|tab| (tab.path.clone(), tab.preview))
                .collect::<Vec<_>>()
        })
    };

    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &one, false, cx);
        model.activate_workspace_tab(&path, HOME_TAB, cx);
        model.open_workspace_file(&path, &two, false, cx);
    });
    assert_eq!(ids(&app, cx), [("src/two.rs".to_string(), true)]);

    app.model.update(cx, |model, cx| {
        model.pin_workspace_tab(&path, &two.id(), cx);
        model.activate_workspace_tab(&path, HOME_TAB, cx);
        model.open_workspace_file(&path, &one, false, cx);
        // La paleta reutiliza la de vista previa.
        model.open_workspace_file_preview(&path, &FileTabDescriptor::working("README.md"), cx);
    });
    assert_eq!(
        ids(&app, cx),
        [
            ("src/two.rs".to_string(), false),
            ("README.md".to_string(), true)
        ]
    );
    cx.run_until_parked();
    let stored = stored_workspace(&app, &path);
    assert_eq!(stored["tabs"][1]["preview"], true);
    assert!(stored["tabs"][0].get("preview").is_none());

    // Arrastrar la mueve y la fija.
    app.model
        .update(cx, |model, cx| model.move_workspace_tab(&path, 1, 0, cx));
    assert_eq!(
        ids(&app, cx),
        [
            ("README.md".to_string(), false),
            ("src/two.rs".to_string(), false)
        ]
    );

    // Alt+→ / Alt+←, dando la vuelta; desde Commits, el primero.
    let readme = FileTabDescriptor::working("README.md").id();
    app.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, HOME_TAB, cx);
        assert_eq!(model.cycle_document_tab(true, cx), Some(readme.clone()));
        assert_eq!(model.cycle_document_tab(true, cx), Some(two.id()));
        assert_eq!(model.cycle_document_tab(true, cx), Some(readme.clone()));
        assert_eq!(model.cycle_document_tab(false, cx), Some(two.id()));
    });

    // "Close to the right" desde la primera con la segunda activa: queda la
    // primera a la vista.
    app.model.update(cx, |model, cx| {
        model.close_workspace_tabs_to_the_right(&path, &readme, cx);
        assert_eq!(model.repository_workspace(&path).active_tab, readme);
        model.open_workspace_file(&path, &two, false, cx);
        model.open_workspace_file(&path, &one, false, cx);
        // "Close others" deja la elegida a la vista.
        model.close_other_workspace_tabs(&path, &two.id(), cx);
        let workspace = model.repository_workspace(&path);
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_tab, two.id());
        // "Close all" vuelve a la última vista fija.
        model.close_all_workspace_tabs(&path, cx);
        let workspace = model.repository_workspace(&path);
        assert!(workspace.tabs.is_empty());
        assert_eq!(workspace.active_tab, HOME_TAB);
        assert_eq!(model.cycle_document_tab(true, cx), None);
    });
}
