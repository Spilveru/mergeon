//! `ChangesSidebar` (`RepositoryChangesSidebar.test.js`): la lista suelta, con
//! sus archivos puestos a mano y lo que avisa registrado.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, Render, TestAppContext, Window,
};
use mergeon_gitcmd::{ConflictState, FileStatus};
use mergeon_ui::status_label;
use mergeon_view_changes::{
    ChangesSection, ChangesSidebar, FileAction, FileActionKind, MultiSelection, effective_status,
};

use crate::support::TestApp;
use gpui_kit::{ParentElement as _, Styled as _};

fn file(path: &str, index: &str, working_dir: &str) -> FileStatus {
    FileStatus {
        path: path.to_string(),
        index: index.to_string(),
        working_dir: working_dir.to_string(),
        from: None,
    }
}

#[derive(Clone, Default)]
struct Props {
    conflict_state: Option<ConflictState>,
    conflicted: Vec<FileStatus>,
    unstaged: Vec<FileStatus>,
    untracked: Vec<FileStatus>,
    staged: Vec<FileStatus>,
    selected: Option<String>,
    head_hash: Option<String>,
    /// Como la vista: acciones con `on_file_action`, selección múltiple y
    /// conmutador Árbol / Lista.
    actions: bool,
    selected_section: Option<ChangesSection>,
    multi: Option<MultiSelection>,
    flat: bool,
}

fn describe(action: &FileAction) -> String {
    let kind = match action.kind {
        FileActionKind::Stage => "stage",
        FileActionKind::Unstage => "unstage",
        FileActionKind::Discard => "discard",
    };
    let files: Vec<&str> = action.files.iter().map(|file| file.path.as_str()).collect();
    format!(
        "{kind}:{}:[{}]:next={}",
        action.section.as_str(),
        files.join(","),
        action.next.as_ref().map_or("-", |file| file.path.as_str())
    )
}

struct SidebarHost {
    props: Props,
    log: Rc<RefCell<Vec<String>>>,
    focus: FocusHandle,
}

impl Render for SidebarHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let props = self.props.clone();
        let file_event = |what: &'static str| {
            let log = self.log.clone();
            move |file: &FileStatus, _: &mut Window, _: &mut gpui_kit::App| {
                log.borrow_mut().push(format!("{what}:{}", file.path));
            }
        };
        let event = |what: &'static str| {
            let log = self.log.clone();
            move |_: &mut Window, _: &mut gpui_kit::App| log.borrow_mut().push(what.to_string())
        };
        let has_changes = !(props.conflicted.is_empty()
            && props.unstaged.is_empty()
            && props.untracked.is_empty()
            && props.staged.is_empty());

        let sidebar = ChangesSidebar::new("changes-list")
            .selected_section(props.selected_section)
            .multi_selection(props.multi.clone().map(std::rc::Rc::new))
            .flat(props.flat);
        let sidebar = if props.actions {
            sidebar
                .on_file_action(cx.listener(|this, action: &FileAction, _, cx| {
                    this.log.borrow_mut().push(describe(action));
                    // Como la vista: la selección pasa al siguiente.
                    if let Some(next) = &action.next {
                        this.props.selected = Some(next.path.clone());
                        this.props.multi = None;
                    }
                    cx.notify();
                }))
                .on_multi_select(cx.listener(|this, selection: &MultiSelection, _, cx| {
                    this.log.borrow_mut().push(format!(
                        "multi:{}:[{}]",
                        selection.section.as_str(),
                        selection.paths.join(",")
                    ));
                    this.props.multi = Some(selection.clone());
                    cx.notify();
                }))
                .on_layout_change(cx.listener(|this, flat: &bool, _, cx| {
                    this.log.borrow_mut().push(format!("layout:{flat}"));
                    this.props.flat = *flat;
                    cx.notify();
                }))
        } else {
            sidebar
        };
        sidebar
            .has_changes(has_changes)
            .conflict_state(props.conflict_state)
            .conflicted_files(props.conflicted)
            .unstaged_files(props.unstaged)
            .untracked_files(props.untracked)
            .staged_files(props.staged)
            .selected_file_path(props.selected)
            // Como la vista: elegir un archivo lo deja elegido.
            .on_select_file(cx.listener(|this, file: &FileStatus, _, cx| {
                this.log.borrow_mut().push(format!("select:{}", file.path));
                this.props.selected = Some(file.path.clone());
                this.props.multi = None;
                cx.notify();
            }))
            .on_open_file(file_event("open"))
            .on_stage_file(file_event("stage"))
            .on_unstage_file(file_event("unstage"))
            .on_discard_file(file_event("discard"))
            .track_focus(&self.focus)
            .on_stage_all(event("stage-all"))
            .on_unstage_all(event("unstage-all"))
            .on_discard_all(event("discard-all"))
            .head_hash(props.head_hash.clone())
            .on_open_terminal(event("open-terminal"))
            .on_open_folder(event("open-folder"))
    }
}

struct Sidebar {
    app: TestApp,
    host: Entity<SidebarHost>,
    log: Rc<RefCell<Vec<String>>>,
}

impl Sidebar {
    fn new(cx: &mut TestAppContext, props: Props) -> Self {
        let app = TestApp::new(cx);
        let log: Rc<RefCell<Vec<String>>> = Rc::default();
        let host = cx.new(|cx| SidebarHost {
            props,
            log: log.clone(),
            focus: cx.focus_handle(),
        });
        let shown = host.clone();
        app.show(cx, move |_, _| {
            gpui_kit::div()
                .w(gpui_kit::px(330.))
                .h(gpui_kit::px(700.))
                .child(shown.clone())
                .into_any_element()
        });
        Self { app, host, log }
    }

    fn update(&self, cx: &mut TestAppContext, change: impl FnOnce(&mut Props)) {
        self.host.update(cx, |host, cx| {
            change(&mut host.props);
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn clear_log(&self) {
        self.log.borrow_mut().clear();
    }

    /// `(checked, indeterminate)` de una casilla.
    fn check_state(&self, cx: &mut TestAppContext, id: &str) -> (Option<bool>, Option<bool>) {
        let id = gpui_kit::SharedString::from(id.to_string());
        self.app.in_window(cx, |window, _| {
            let check = window.find(id);
            (check.checked(), check.indeterminate())
        })
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        self.app.in_window(cx, |window, cx| window.press(key, cx));
    }
}

/// El banner de secuencia ya no se pinta aquí: vive bajo la barra de
/// herramientas (antes aparecía duplicado en la vista Changes).
#[gpui_kit::test]
async fn renders_the_three_file_sections(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            conflict_state: Some(ConflictState {
                in_conflict: true,
                initial_count: 2,
                pending_count: 1,
                resolved_count: 1,
                pending_files: vec!["src/a.js".to_string()],
                ..Default::default()
            }),
            conflicted: vec![file("src/a.js", "U", "U")],
            unstaged: vec![file("src/b.js", " ", "M")],
            untracked: vec![file("src/c.js", "?", "?")],
            staged: vec![file("src/d.js", "M", " ")],
            selected: Some("src/a.js".to_string()),
            ..Default::default()
        },
    );
    let app = &sidebar.app;

    for section in ["conflict-section", "unstaged-section", "staged-section"] {
        assert!(app.exists(cx, section), "`{section}` should be drawn");
    }
    assert!(!app.exists(cx, "operation-banner"));
    // Lo que no se sigue va en "Unstaged Changes", y el progreso de los
    // conflictos sale del estado: 1 de 2 (el inicial, no los que quedan).
    assert!(app.exists(cx, "unstaged:file:src/c.js"));
    assert_eq!(app.label(cx, "conflict-progress").as_deref(), Some("1 / 2"));
    assert!(app.exists(cx, "conflict:file:src/a.js"));
}

#[gpui_kit::test]
async fn renders_added_and_untracked_files_with_a_plus_badge(cx: &mut TestAppContext) {
    let untracked = file("src/new-file.js", "??", "??");
    let added = file("src/added-file.js", "A", " ");
    assert_eq!(status_label(effective_status(&untracked)), "+");
    assert_eq!(status_label(effective_status(&added)), "+");

    let sidebar = Sidebar::new(
        cx,
        Props {
            untracked: vec![untracked],
            staged: vec![added],
            ..Default::default()
        },
    );
    assert!(sidebar.app.exists(cx, "unstaged:file:src/new-file.js"));
    assert!(sidebar.app.exists(cx, "staged:file:src/added-file.js"));
}

#[gpui_kit::test]
async fn emits_file_and_bulk_actions_from_the_sidebar(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            staged: vec![file("src/d.js", "M", " ")],
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    // Las casillas por archivo sustituyen a "Stage selected" / "Unstage selected".
    assert!(!app.exists(cx, "stage-selected"));
    assert!(!app.exists(cx, "unstage-selected"));

    app.click(cx, "unstaged:file:src/b.js");
    // La casilla prepara o quita del stage sin elegir la fila.
    app.click(cx, "check:unstaged:file:src/b.js");
    app.click(cx, "check:staged:file:src/d.js");
    app.click(cx, "unstage-all");
    app.click(cx, "stage-all");

    assert_eq!(
        sidebar.log(),
        [
            "select:src/b.js",
            "stage:src/b.js",
            "unstage:src/d.js",
            "unstage-all",
            "stage-all"
        ]
    );
}

#[gpui_kit::test]
async fn the_section_checkboxes_stage_and_unstage_everything(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            untracked: vec![file("notes.txt", "?", "?")],
            staged: vec![file("src/d.js", "M", " ")],
            ..Default::default()
        },
    );

    assert_eq!(
        sidebar.check_state(cx, "unstaged-check"),
        (Some(false), Some(false))
    );
    assert_eq!(
        sidebar.check_state(cx, "staged-check"),
        (Some(true), Some(false))
    );
    assert_eq!(
        sidebar.check_state(cx, "check:unstaged:file:notes.txt"),
        (Some(false), Some(false))
    );
    assert_eq!(
        sidebar.check_state(cx, "check:staged:file:src/d.js"),
        (Some(true), Some(false))
    );

    sidebar.app.click(cx, "unstaged-check");
    sidebar.app.click(cx, "staged-check");
    assert_eq!(sidebar.log(), ["stage-all", "unstage-all"]);
}

// Un archivo a medio preparar está en las dos secciones: su casilla y las de
// las dos secciones salen mixtas.
#[gpui_kit::test]
async fn a_partially_staged_file_is_mixed_in_both_sections(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            staged: vec![file("src/b.js", "M", "M"), file("src/d.js", "A", " ")],
            ..Default::default()
        },
    );

    for id in [
        "check:unstaged:file:src/b.js",
        "check:staged:file:src/b.js",
        "unstaged-check",
        "staged-check",
    ] {
        assert_eq!(
            sidebar.check_state(cx, id),
            (Some(false), Some(true)),
            "{id} should be mixed"
        );
    }
    assert_eq!(
        sidebar.check_state(cx, "check:staged:file:src/d.js"),
        (Some(true), Some(false))
    );

    // Lo mixto se completa en sin preparar y se quita en preparados.
    sidebar.app.click(cx, "check:unstaged:file:src/b.js");
    sidebar.app.click(cx, "check:staged:file:src/b.js");
    assert_eq!(sidebar.log(), ["stage:src/b.js", "unstage:src/b.js"]);
}

#[gpui_kit::test]
async fn section_headers_are_30px_tall(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            staged: vec![file("src/d.js", "M", " ")],
            ..Default::default()
        },
    );
    let heights = sidebar.app.in_window(cx, |window, _| {
        ["unstaged-header", "staged-header"].map(|id| window.find(id).bounds().size.height)
    });
    // Señal: `.ch-h` mide 30px.
    for height in heights {
        assert_eq!(height, gpui_kit::px(30.));
    }
}

// ---------------------------------------------------------------------------
// Teclado
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn arrows_move_the_selection_through_the_visible_files(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/lib/b.js", " ", "M"), file("README.md", " ", "M")],
            staged: vec![file("LICENSE", "M", " ")],
            ..Default::default()
        },
    );
    // Un clic en la lista le da el foco.
    sidebar.app.click(cx, "unstaged:file:src/lib/b.js");
    sidebar.clear_log();

    // El orden es el de la lista: carpetas primero, luego preparados.
    sidebar.press(cx, "down");
    sidebar.press(cx, "down");
    sidebar.press(cx, "down");
    assert_eq!(sidebar.log(), ["select:README.md", "select:LICENSE"]);
    sidebar.press(cx, "up");
    sidebar.press(cx, "up");
    sidebar.press(cx, "up");
    assert_eq!(
        sidebar.log(),
        [
            "select:README.md",
            "select:LICENSE",
            "select:README.md",
            "select:src/lib/b.js"
        ]
    );

    // Lo que hay dentro de una carpeta plegada no se recorre (`src/lib` va
    // en una sola fila: `src` no tiene otra cosa).
    sidebar.app.click(cx, "unstaged:folder:src/lib");
    sidebar.app.click(cx, "unstaged:file:README.md");
    sidebar.clear_log();
    sidebar.press(cx, "up");
    sidebar.press(cx, "down");
    assert_eq!(sidebar.log(), ["select:LICENSE"]);
}

#[gpui_kit::test]
async fn space_stages_or_unstages_and_d_asks_to_discard(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            staged: vec![file("src/b.js", "M", "M"), file("src/d.js", "M", " ")],
            ..Default::default()
        },
    );
    sidebar.app.click(cx, "unstaged:file:src/b.js");
    sidebar.clear_log();

    sidebar.press(cx, "space");
    sidebar.press(cx, "d");
    // Abajo está la misma ruta en preparados: ahí Espacio la quita del stage
    // y D no descarta (solo se descarta lo que no está preparado).
    sidebar.press(cx, "down");
    sidebar.press(cx, "space");
    sidebar.press(cx, "d");
    sidebar.press(cx, "down");
    sidebar.press(cx, "space");

    assert_eq!(
        sidebar.log(),
        [
            "stage:src/b.js",
            "discard:src/b.js",
            "select:src/b.js",
            "unstage:src/b.js",
            "select:src/d.js",
            "unstage:src/d.js"
        ]
    );
}

/// La pista dice `D`: en mayúscula (Shift+D) también descarta. Espacio con
/// Shift sigue sin ser de la lista.
#[gpui_kit::test]
async fn shift_d_also_asks_to_discard(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            ..Default::default()
        },
    );
    sidebar.app.click(cx, "unstaged:file:src/b.js");
    sidebar.clear_log();

    sidebar.press(cx, "shift-d");
    sidebar.press(cx, "shift-space");
    sidebar.press(cx, "ctrl-shift-d");
    assert_eq!(sidebar.log(), ["discard:src/b.js"]);
}

#[gpui_kit::test]
async fn the_keys_belong_to_the_list_only_while_it_has_focus(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            selected: Some("src/b.js".to_string()),
            ..Default::default()
        },
    );

    sidebar.press(cx, "space");
    sidebar.press(cx, "down");
    sidebar.press(cx, "d");
    assert!(sidebar.log().is_empty());

    sidebar.app.click(cx, "unstaged:file:src/b.js");
    sidebar.clear_log();
    // Con un modificador ya no es la tecla de la lista.
    sidebar.press(cx, "ctrl-d");
    sidebar.press(cx, "shift-space");
    assert!(sidebar.log().is_empty());
    sidebar.press(cx, "space");
    assert_eq!(sidebar.log(), ["stage:src/b.js"]);
}

#[gpui_kit::test]
async fn asks_for_a_new_tab_on_double_click(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            ..Default::default()
        },
    );

    sidebar.app.double_click(cx, "unstaged:file:src/b.js");

    let opened: Vec<String> = sidebar
        .log()
        .into_iter()
        .filter(|entry| entry.starts_with("open:"))
        .collect();
    assert_eq!(opened, ["open:src/b.js"]);
}

#[gpui_kit::test]
async fn folders_fold_and_unfold(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/lib/b.js", " ", "M"), file("README.md", " ", "M")],
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    assert!(app.exists(cx, "unstaged:file:src/lib/b.js"));

    // `src/lib` es una sola fila (`src` no tiene otra cosa).
    assert!(!app.exists(cx, "unstaged:folder:src"));
    app.click(cx, "unstaged:folder:src/lib");
    assert!(!app.exists(cx, "unstaged:file:src/lib/b.js"));
    assert!(app.exists(cx, "unstaged:folder:src/lib"));
    assert!(app.exists(cx, "unstaged:file:README.md"));

    app.click(cx, "unstaged:folder:src/lib");
    assert!(app.exists(cx, "unstaged:file:src/lib/b.js"));
    // Plegar una carpeta no es elegir un archivo.
    assert!(sidebar.log().is_empty());
}

#[gpui_kit::test]
async fn bulk_actions_follow_what_each_section_holds(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            staged: vec![file("src/d.js", "M", " ")],
            ..Default::default()
        },
    );
    let app = &sidebar.app;

    // Sin nada sin preparar: ni "Stage all" ni "Discard all".
    assert!(!app.exists(cx, "stage-all"));
    assert!(!app.exists(cx, "discard-all"));
    assert!(app.exists(cx, "unstage-all"));
    assert!(app.exists(cx, "unstaged-section"));

    sidebar.update(cx, |props| {
        props.staged.clear();
        props.untracked = vec![file("notes.txt", "?", "?")];
    });
    assert!(app.exists(cx, "stage-all"));
    assert!(app.exists(cx, "discard-all"));
    assert!(!app.exists(cx, "unstage-all"));
    app.click(cx, "discard-all");
    assert_eq!(sidebar.log(), ["discard-all"]);
}

#[gpui_kit::test]
async fn says_when_the_working_directory_is_clean(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(cx, Props::default());

    assert!(sidebar.app.exists(cx, "changes-empty"));
    assert!(!sidebar.app.exists(cx, "unstaged-section"));
    // Sin hash de HEAD (repo sin commits) no se nombra ninguno.
    assert!(!sidebar.app.exists(cx, "changes-empty-head"));
}

/// El estado vacío nombra el commit de HEAD y ofrece abrir la terminal o la
/// carpeta con las acciones de la barra de herramientas.
#[gpui_kit::test]
async fn the_clean_state_names_head_and_opens_a_terminal_or_the_folder(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            head_hash: Some("c4e1a07".to_string()),
            ..Default::default()
        },
    );
    let app = &sidebar.app;

    assert!(app.exists(cx, "changes-empty-head"));
    app.click(cx, "changes-empty-terminal");
    app.click(cx, "changes-empty-folder");
    assert_eq!(sidebar.log(), ["open-terminal", "open-folder"]);
}

// ---------------------------------------------------------------------------
// Avanzar tras preparar o descartar
// ---------------------------------------------------------------------------

fn three_unstaged() -> Props {
    Props {
        unstaged: vec![
            file("a.js", " ", "M"),
            file("b.js", " ", "M"),
            file("c.js", " ", "M"),
        ],
        staged: vec![file("s.js", "M", " ")],
        actions: true,
        ..Default::default()
    }
}

/// Espacio y D sobre el elegido dicen adónde pasa la selección: al
/// siguiente de la misma sección, o al anterior si era el último.
#[gpui_kit::test]
async fn space_and_d_move_on_to_the_next_file_of_the_same_section(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(cx, three_unstaged());
    sidebar.app.click(cx, "unstaged:file:b.js");
    sidebar.clear_log();

    sidebar.press(cx, "space");
    assert_eq!(sidebar.log(), ["stage:unstaged:[b.js]:next=c.js"]);
    // El anfitrión eligió `c.js`: es el último, así que D vuelve a `a.js`
    // (b.js sigue en la lista porque nadie la releyó).
    sidebar.clear_log();
    sidebar.press(cx, "d");
    assert_eq!(sidebar.log(), ["discard:unstaged:[c.js]:next=b.js"]);

    // El único de su sección: no hay siguiente (la selección le sigue).
    sidebar.app.click(cx, "staged:file:s.js");
    sidebar.clear_log();
    sidebar.press(cx, "space");
    assert_eq!(sidebar.log(), ["unstage:staged:[s.js]:next=-"]);
}

/// La casilla de la fila elegida también avanza; la de otra fila no mueve la
/// selección.
#[gpui_kit::test]
async fn the_row_checkbox_advances_only_from_the_selected_row(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(cx, three_unstaged());
    sidebar.app.click(cx, "unstaged:file:a.js");
    sidebar.clear_log();

    sidebar.app.click(cx, "check:unstaged:file:c.js");
    sidebar.app.click(cx, "check:unstaged:file:a.js");
    assert_eq!(
        sidebar.log(),
        [
            "stage:unstaged:[c.js]:next=-",
            "stage:unstaged:[a.js]:next=b.js"
        ]
    );
}

// ---------------------------------------------------------------------------
// Selección múltiple
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn shift_and_ctrl_click_select_several_files_of_a_section(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(cx, three_unstaged());
    let app = &sidebar.app;
    app.click(cx, "unstaged:file:a.js");
    sidebar.clear_log();

    app.click_with(
        cx,
        "unstaged:file:c.js",
        gpui_kit::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    assert_eq!(sidebar.log(), ["multi:unstaged:[a.js,b.js,c.js]"]);

    // Ctrl+clic quita y pone.
    sidebar.clear_log();
    let ctrl = gpui_kit::Modifiers {
        control: true,
        ..Default::default()
    };
    app.click_with(cx, "unstaged:file:b.js", ctrl);
    assert_eq!(sidebar.log(), ["multi:unstaged:[a.js,c.js]"]);

    // Espacio y D actúan sobre toda la selección.
    sidebar.clear_log();
    sidebar.press(cx, "space");
    assert_eq!(sidebar.log(), ["stage:unstaged:[a.js,c.js]:next=b.js"]);

    // Un clic normal vuelve a un solo archivo.
    app.click(cx, "unstaged:file:a.js");
    app.click_with(cx, "unstaged:file:b.js", ctrl);
    sidebar.clear_log();
    sidebar.press(cx, "d");
    assert_eq!(sidebar.log(), ["discard:unstaged:[a.js,b.js]:next=c.js"]);
}

#[gpui_kit::test]
async fn ctrl_a_selects_the_whole_section(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(cx, three_unstaged());
    sidebar.app.click(cx, "staged:file:s.js");
    sidebar.app.click(cx, "unstaged:file:b.js");
    sidebar.clear_log();

    sidebar.press(cx, "ctrl-a");
    assert_eq!(sidebar.log(), ["multi:unstaged:[a.js,b.js,c.js]"]);
    // En preparados, D no descarta nada.
    sidebar.app.click(cx, "staged:file:s.js");
    sidebar.clear_log();
    sidebar.press(cx, "ctrl-a");
    // Un solo archivo: se elige sin más.
    assert_eq!(sidebar.log(), ["select:s.js"]);
}

/// La casilla de una carpeta prepara (o quita) todo lo de debajo.
#[gpui_kit::test]
async fn a_folder_checkbox_stages_the_whole_folder(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![
                file("src/lib/a.js", " ", "M"),
                file("src/b.js", " ", "M"),
                file("top.js", " ", "M"),
            ],
            staged: vec![file("docs/x.md", "M", " ")],
            actions: true,
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    assert_eq!(
        sidebar.check_state(cx, "check:unstaged:folder:src"),
        (Some(false), Some(false))
    );
    assert_eq!(
        sidebar.check_state(cx, "check:staged:folder:docs"),
        (Some(true), Some(false))
    );

    app.click(cx, "check:unstaged:folder:src");
    app.click(cx, "check:staged:folder:docs");
    assert_eq!(
        sidebar.log(),
        [
            "stage:unstaged:[src/lib/a.js,src/b.js]:next=-",
            "unstage:staged:[docs/x.md]:next=-"
        ]
    );
}

// ---------------------------------------------------------------------------
// Árbol compacto, lista plana y renombres
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn single_child_folders_share_a_row(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![
                file("src/a/b/one.js", " ", "M"),
                file("src/a/b/two.js", " ", "M"),
            ],
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    assert!(app.exists(cx, "unstaged:folder:src/a/b"));
    assert!(!app.exists(cx, "unstaged:folder:src"));
    assert!(!app.exists(cx, "unstaged:folder:src/a"));

    // Plegarla pliega la más honda.
    app.click(cx, "unstaged:folder:src/a/b");
    assert!(!app.exists(cx, "unstaged:file:src/a/b/one.js"));
}

#[gpui_kit::test]
async fn the_list_switches_between_tree_and_flat(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/lib/a.js", " ", "M"), file("top.js", " ", "M")],
            actions: true,
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    assert_eq!(app.label(cx, "changes-layout").as_deref(), Some("Tree"));
    assert_eq!(
        app.label(cx, "changes-list-count").as_deref(),
        Some("2 changed files")
    );
    assert!(app.exists(cx, "unstaged:folder:src/lib"));

    app.click(cx, "changes-layout-flat");
    assert_eq!(sidebar.log(), ["layout:true"]);
    assert_eq!(app.label(cx, "changes-layout").as_deref(), Some("List"));
    assert!(!app.exists(cx, "unstaged:folder:src/lib"));
    assert!(app.exists(cx, "unstaged:file:src/lib/a.js"));
    // El nombre, con su carpeta en gris al lado.
    assert_eq!(
        app.row_label(cx, "unstaged:file:src/lib/a.js").as_deref(),
        Some("a.js")
    );

    // El conmutador solo aparece si quien aloja la lista lo atiende.
    let plain = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("top.js", " ", "M")],
            ..Default::default()
        },
    );
    assert!(!plain.app.exists(cx, "changes-list-toolbar"));
}

#[gpui_kit::test]
async fn a_rename_shows_where_the_file_came_from(cx: &mut TestAppContext) {
    let mut renamed = file("src/new-name.js", "R", " ");
    renamed.from = Some("src/old-name.js".to_string());
    let mut moved = file("lib/moved.js", "R", " ");
    moved.from = Some("src/moved.js".to_string());
    let sidebar = Sidebar::new(
        cx,
        Props {
            staged: vec![renamed, moved],
            ..Default::default()
        },
    );
    assert_eq!(
        sidebar
            .app
            .row_label(cx, "staged:file:src/new-name.js")
            .as_deref(),
        Some("old-name.js \u{2192} new-name.js")
    );
    // De otra carpeta: la ruta vieja entera.
    assert_eq!(
        sidebar
            .app
            .row_label(cx, "staged:file:lib/moved.js")
            .as_deref(),
        Some("src/moved.js \u{2192} moved.js")
    );
}

/// Un archivo a medio preparar: solo la fila de la sección elegida sale
/// elegida; la otra lleva la marca tenue de "el mismo archivo".
#[gpui_kit::test]
async fn only_the_row_of_the_chosen_side_is_selected(cx: &mut TestAppContext) {
    let sidebar = Sidebar::new(
        cx,
        Props {
            unstaged: vec![file("src/b.js", " ", "M")],
            staged: vec![file("src/b.js", "M", "M")],
            selected: Some("src/b.js".to_string()),
            selected_section: Some(ChangesSection::Unstaged),
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    assert!(app.exists(cx, "same-file:staged:file:src/b.js"));
    assert!(!app.exists(cx, "same-file:unstaged:file:src/b.js"));

    sidebar.update(cx, |props| {
        props.selected_section = Some(ChangesSection::Staged)
    });
    assert!(app.exists(cx, "same-file:unstaged:file:src/b.js"));
    assert!(!app.exists(cx, "same-file:staged:file:src/b.js"));
}

// ---------------------------------------------------------------------------
// Conflictos
// ---------------------------------------------------------------------------

/// Con muchos conflictos su sección cede sitio: Unstaged y Staged se siguen
/// viendo. La cabecera lleva una barra fina con lo resuelto.
#[gpui_kit::test]
async fn many_conflicts_leave_room_for_the_other_sections(cx: &mut TestAppContext) {
    let conflicted: Vec<FileStatus> = (0..60)
        .map(|index| file(&format!("c{index:02}.txt"), "U", "U"))
        .collect();
    let sidebar = Sidebar::new(
        cx,
        Props {
            conflict_state: Some(ConflictState {
                in_conflict: true,
                initial_count: 80,
                pending_count: 60,
                resolved_count: 20,
                ..Default::default()
            }),
            conflicted,
            unstaged: vec![file("src/b.js", " ", "M")],
            staged: vec![file("src/d.js", "M", " ")],
            ..Default::default()
        },
    );
    let app = &sidebar.app;
    assert_eq!(
        app.label(cx, "conflict-progress").as_deref(),
        Some("20 / 80")
    );
    assert_eq!(
        app.label(cx, "conflict-progress-bar").as_deref(),
        Some("25%")
    );
    let (list, staged_row, unstaged_row, conflicts) = app.in_window(cx, |window, _| {
        (
            window.find("changes-list").bounds(),
            window.find("staged:file:src/d.js").bounds(),
            window.find("unstaged:file:src/b.js").bounds(),
            window.find("conflict-section").bounds(),
        )
    });
    assert!(
        staged_row.bottom() <= list.bottom(),
        "{staged_row:?} / {list:?}"
    );
    assert!(unstaged_row.bottom() <= list.bottom());
    assert!(conflicts.bottom() <= unstaged_row.top());
}
