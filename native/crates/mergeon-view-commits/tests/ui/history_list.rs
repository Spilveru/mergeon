//! `CommitHistoryList.test.js`: la lista sola, con filas hechas a mano.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, TestAppContext, TextRun, Window, div, font, px, size,
};
use mergeon_model::commit_view_utils::{
    CommitGraph, CommitRef, CommitRow, GraphLane, RefType, badge_title, build_commit_graph,
    group_ref_badges, hidden_badge_title,
};
use mergeon_ui::menu::MenuItem;
use mergeon_view_commits::{
    CommitHistoryList, CommitHistoryListEvent, GRAPH_LABEL_INSET, ROW_HEIGHT,
    WORKING_CHANGES_ROW_ID, WorkingChanges, ref_badge_title, refs_tooltip,
};

use crate::support::TestApp;

fn commit(hash: &str, message: &str, refs: Vec<CommitRef>) -> CommitRow {
    CommitRow {
        hash: hash.to_string(),
        full_hash: hash.to_string(),
        message: message.to_string(),
        author: "Test User".to_string(),
        date: "today".to_string(),
        color: "var(--primary)".to_string(),
        refs,
        ..Default::default()
    }
}

fn reference(name: &str, kind: RefType, is_head: bool) -> CommitRef {
    CommitRef {
        name: name.to_string(),
        kind,
        is_head,
        color: "var(--primary)".to_string(),
    }
}

/// Una lista lineal de `count` commits, con HEAD en `head` (si lo hay).
fn history(count: usize, head: Option<usize>) -> Vec<CommitRow> {
    let rows = (0..count)
        .map(|index| {
            let refs = if head == Some(index) {
                vec![reference("main", RefType::Local, true)]
            } else {
                Vec::new()
            };
            CommitRow {
                parents: if index + 1 < count {
                    format!("c{}", index + 1)
                } else {
                    String::new()
                },
                ..commit(&format!("c{index}"), &format!("commit {index}"), refs)
            }
        })
        .collect();
    build_commit_graph(rows)
}

fn mount_list(app: &TestApp, cx: &mut TestAppContext) -> Entity<CommitHistoryList> {
    app.mount(cx, CommitHistoryList::new)
}

fn show(list: &Entity<CommitHistoryList>, commits: Vec<CommitRow>, cx: &mut TestAppContext) {
    list.update(cx, |list, cx| list.set_commits(Arc::new(commits), cx));
}

fn row(hash: &str) -> SharedString {
    SharedString::from(format!("commit:{hash}"))
}

#[gpui_kit::test]
async fn renders_commits_and_emits_selection_plus_context_menu_events(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let menus = Rc::new(RefCell::new(0));
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&list, move |_, event: &CommitHistoryListEvent, _| {
            events.borrow_mut().push(match event {
                CommitHistoryListEvent::SelectCommit(_) => "select",
                CommitHistoryListEvent::CheckoutCommit(_) => "checkout",
                CommitHistoryListEvent::LoadMore => "load-more",
                CommitHistoryListEvent::OpenWorkingChanges => "open-working-changes",
                CommitHistoryListEvent::ToggleCommit(_) => "toggle",
                CommitHistoryListEvent::ExtendSelection(_) => "extend",
                CommitHistoryListEvent::CheckoutRef { .. } => "checkout-ref",
            });
        })
        .detach();
    });
    list.update(cx, |list, cx| {
        let menus = menus.clone();
        list.set_context_menu(move |_, _, _| {
            *menus.borrow_mut() += 1;
            vec![MenuItem::new("Checkout")]
        });
        list.set_selected_commit_hash(Some("abc1234"), cx);
    });
    show(
        &list,
        vec![commit(
            "abc1234",
            "Initial commit",
            vec![reference("main", RefType::Local, false)],
        )],
        cx,
    );

    app.in_window(cx, |window, cx| window.click(row("abc1234"), cx));
    assert_eq!(*events.borrow(), ["select"]);
    events.borrow_mut().clear();
    app.in_window(cx, |window, cx| window.double_click(row("abc1234"), cx));
    // El doble clic llega después de sus dos clics, como en el navegador.
    assert_eq!(*events.borrow(), ["select", "select", "checkout"]);
    app.in_window(cx, |window, cx| window.right_click(row("abc1234"), cx));
    assert_eq!(*menus.borrow(), 1);
    assert!(app.menu_is_open(cx));
}

/// Un pull mueve HEAD. Si el usuario estaba leyendo historia vieja, la lista
/// no puede arrastrarlo de vuelta a la punta.
#[gpui_kit::test]
async fn does_not_chase_head_when_the_user_has_scrolled_away_from_the_tip(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    show(&list, history(60, Some(0)), cx);
    app.in_window(cx, |_, _| {});
    list.update(cx, |list, _| list.set_scroll_top(400.));
    app.in_window(cx, |_, _| {});

    // Llega un commit nuevo con HEAD encima: la lista no debe saltar, y lo que
    // se veía sigue en su sitio (una fila más abajo).
    let mut pulled = vec![commit(
        "new1234",
        "Pulled commit",
        vec![reference("main", RefType::Local, true)],
    )];
    pulled.extend(history(60, None));
    show(&list, pulled, cx);
    app.in_window(cx, |_, _| {});

    // 400 + una fila de 28px.
    assert_eq!(list.read_with(cx, |list, _| list.scroll_top()), 428.);
}

#[gpui_kit::test]
async fn brings_head_into_view_when_the_user_is_sitting_at_the_tip(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    show(&list, history(60, None), cx);
    app.in_window(cx, |_, _| {});
    assert_eq!(list.read_with(cx, |list, _| list.scroll_top()), 0.);

    show(&list, history(60, Some(50)), cx);
    app.in_window(cx, |_, _| {});

    let (top, painted) = list.read_with(cx, |list, _| (list.scroll_top(), list.painted_rows()));
    assert!(top > 0.);
    assert!(painted.contains(&50));
}

#[gpui_kit::test]
async fn marks_the_commit_head_is_parked_on_and_leaves_the_rest_untouched(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let commits = vec![
        commit(
            "abc1234",
            "Current commit",
            vec![reference("main", RefType::Local, true)],
        ),
        commit(
            "def5678",
            "Older commit",
            vec![reference("origin/main", RefType::Remote, false)],
        ),
    ];
    show(&list, commits.clone(), cx);

    let flags: Vec<bool> = list.read_with(cx, |list, _| {
        commits
            .iter()
            .map(|commit| list.row_flags(commit).is_head)
            .collect()
    });
    assert_eq!(flags, [true, false]);
    // El tooltip del badge dice dónde estás.
    let badges = group_ref_badges(&commits[0].refs);
    assert_eq!(
        badge_title(&badges[0]).as_deref(),
        Some("You are here · HEAD is on main")
    );
    assert!(app.exists(cx, row("abc1234")));
}

#[gpui_kit::test]
async fn marks_a_detached_head_where_no_branch_points_at_the_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let commits = vec![commit(
        "abc1234",
        "Detached commit",
        vec![reference("HEAD", RefType::Head, true)],
    )];
    show(&list, commits.clone(), cx);

    assert!(list.read_with(cx, |list, _| list.row_flags(&commits[0]).is_head));
    assert_eq!(
        badge_title(&group_ref_badges(&commits[0].refs)[0]).as_deref(),
        Some("You are here · HEAD detached at this commit")
    );
}

#[gpui_kit::test]
async fn uses_one_shared_graph_width_so_lanes_stay_aligned_across_rows(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let lane = |continues| GraphLane {
        index: 0,
        color: "var(--primary)".into(),
        active: true,
        arrives: true,
        continues,
    };
    let graph = |lane_index, lane_count| CommitGraph {
        lane: lane_index,
        color: "var(--primary)".into(),
        lane_count,
        lanes: vec![lane(true)],
        edges: Vec::new(),
    };
    let commits = vec![
        CommitRow {
            graph: Some(graph(0, 1)),
            ..commit("first", "First", Vec::new())
        },
        CommitRow {
            graph: Some(graph(1, 3)),
            ..commit("second", "Second", Vec::new())
        },
    ];
    list.update(cx, |list, cx| list.set_graph_lane_count(3, cx));
    show(&list, commits, cx);
    app.in_window(cx, |_, _| {});

    // 3 carriles cada 14px con 12px de margen (52px), y 28px de alto = la
    // altura fija de la fila.
    assert_eq!(list.read_with(cx, |list, _| list.graph_width()), 52.);
    for hash in ["first", "second"] {
        let bounds = app.in_window(cx, |window, _| window.find(row(hash)).bounds());
        assert_eq!(f32::from(bounds.size.height), 28.);
    }
}

fn measured(text: &str) -> f64 {
    // Ancho aproximado de Geist Mono a 11px.
    text.chars().count() as f64 * 6.6
}

#[gpui_kit::test]
async fn puts_the_refs_before_the_message_and_folds_origin_into_the_local_badge(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let row = commit(
        "abc1234",
        "Some work",
        vec![
            reference("main", RefType::Local, true),
            reference("origin/main", RefType::Remote, false),
        ],
    );
    show(&list, vec![row.clone()], cx);
    app.in_window(cx, |_, _| {});

    let layout = list.read_with(cx, |list, _| list.badge_layout(&row, measured));
    assert_eq!(layout.hidden, 0);
    assert_eq!(layout.visible.len(), 1);
    assert_eq!(layout.visible[0].name, "main");
    assert_eq!(
        layout.visible[0].remotes.as_deref(),
        Some(&["origin".to_string()][..])
    );
    assert!(badge_title(&layout.visible[0]).is_some_and(|title| title.contains("origin/main")));
}

#[gpui_kit::test]
async fn keeps_a_remote_branch_whole_when_there_is_no_local_copy_on_the_commit(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let row = commit(
        "abc1234",
        "Some work",
        vec![reference("origin/feature/x", RefType::Remote, false)],
    );
    show(&list, vec![row.clone()], cx);

    let layout = list.read_with(cx, |list, _| list.badge_layout(&row, measured));
    assert_eq!(layout.visible[0].name, "origin/feature/x");
    assert_eq!(layout.visible[0].remotes, None);
}

/// Con 300px quedan ~126px para badges: cabe `main`, no cabe el nombre
/// largo, y ese se resume en +1 con su nombre completo en el título.
#[gpui_kit::test]
async fn summarises_the_badges_that_do_not_fit_as_plus_n_with_their_full_names(
    cx: &mut TestAppContext,
) {
    let app = TestApp::with_width(300., cx);
    let list = mount_list(&app, cx);
    let row = commit(
        "abc1234",
        "Some work",
        vec![
            reference("main", RefType::Local, true),
            reference("feature/a-very-long-branch-name", RefType::Local, false),
        ],
    );
    show(&list, vec![row.clone()], cx);
    app.in_window(cx, |_, _| {});
    app.in_window(cx, |_, _| {});

    let layout = list.read_with(cx, |list, _| list.badge_layout(&row, measured));
    let names: Vec<&str> = layout
        .visible
        .iter()
        .map(|badge| badge.name.as_str())
        .collect();
    assert_eq!(names, ["main"]);
    assert_eq!(layout.hidden, 1);
    assert_eq!(
        hidden_badge_title(&group_ref_badges(&row.refs), layout.visible.len()),
        "feature/a-very-long-branch-name"
    );
    assert!(app.exists(cx, "ref-more"));
}

/// Con un solo carril el grafo mide 24px: la etiqueta "GRAPH" se montaba sobre
/// "MESSAGE" ("GRAMESSAGE"). La columna tiene que dejarle sitio.
#[gpui_kit::test]
async fn keeps_the_graph_label_clear_of_the_message_column_with_one_lane(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    show(&list, history(3, Some(0)), cx);

    let (graph, message, label_width) = app.in_window(cx, |window, cx| {
        let graph = window.find("column:graph").bounds();
        let message = window.find("column:message").bounds();
        let text = SharedString::from("GRAPH");
        let line = window.text_system().shape_line(
            text.clone(),
            px(10.5),
            &[TextRun {
                len: text.len(),
                font: font(cx.theme().mono_font_family.clone()),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        (graph, message, line.width)
    });

    assert_eq!(list.read_with(cx, |list, _| list.graph_lane_count()), 1);
    // Sangría + texto + el hueco de 10px antes de la columna siguiente.
    assert!(
        graph.size.width >= px(GRAPH_LABEL_INSET + 10.) + label_width,
        "la columna del grafo ({:?}) no deja sitio a \"GRAPH\" ({label_width:?})",
        graph.size.width
    );
    assert!(message.left() >= graph.right());
    // Las filas alinean el mensaje con su cabecera.
    let row_graph = list.read_with(cx, |list, _| list.graph_column_width());
    assert_eq!(px(row_graph), graph.size.width);
}

#[gpui_kit::test]
async fn puts_the_uncommitted_changes_on_top_without_shifting_commit_indices(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&list, move |_, event: &CommitHistoryListEvent, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    show(&list, history(5, Some(0)), cx);
    let before = app.in_window(cx, |window, _| window.find(row("c0")).bounds());

    list.update(cx, |list, cx| {
        list.set_working_changes(WorkingChanges::new(5, 2), cx)
    });

    assert_eq!(
        app.label(cx, WORKING_CHANGES_ROW_ID).as_deref(),
        Some("5 uncommitted changes \u{b7} 2 staged")
    );
    let after = app.in_window(cx, |window, _| window.find(row("c0")).bounds());
    assert_eq!(after.top() - before.top(), px(ROW_HEIGHT));
    let (bounds, painted) =
        list.read_with(cx, |list, _| (list.row_bounds("c0"), list.painted_rows()));
    assert_eq!(bounds.map(|bounds| bounds.top()), Some(after.top()));
    assert_eq!(painted, 0..5);

    app.click(cx, WORKING_CHANGES_ROW_ID);
    assert_eq!(
        *events.borrow(),
        [CommitHistoryListEvent::OpenWorkingChanges]
    );

    list.update(cx, |list, cx| list.set_working_changes(None, cx));
    assert!(!app.exists(cx, WORKING_CHANGES_ROW_ID));
    let gone = app.in_window(cx, |window, _| window.find(row("c0")).bounds());
    assert_eq!(gone.top(), before.top());
}

#[gpui_kit::test]
async fn says_how_many_changes_are_waiting_and_how_many_are_staged(_cx: &mut TestAppContext) {
    assert_eq!(WorkingChanges::new(0, 0), None);
    assert_eq!(
        WorkingChanges::new(1, 0).map(|changes| changes.label()),
        Some("1 uncommitted change \u{b7} 0 staged".to_string())
    );
    // Nunca más preparados que cambios.
    assert_eq!(
        WorkingChanges::new(2, 9).map(|changes| changes.staged),
        Some(2)
    );
}

/// La lista dentro de un panel de ancho fijo, como el historial junto a su
/// divisor.
struct Pane {
    width: f32,
    list: Entity<CommitHistoryList>,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .h_full()
            .w(px(self.width))
            .child(self.list.clone())
    }
}

/// Con el ancho del panel en `width`, un solo frame: qué columnas lleva la
/// cabecera en ese mismo frame.
fn header_columns_after_one_frame(
    app: &TestApp,
    pane: &Entity<Pane>,
    width: f32,
    cx: &mut TestAppContext,
) -> (bool, bool) {
    cx.update_window(app.window, |_, window, cx| {
        pane.update(cx, |pane, cx| {
            pane.width = width;
            cx.notify();
        });
        window.render_frame(cx);
        (
            window.try_find("column:author").is_some(),
            window.try_find("column:date").is_some(),
        )
    })
    .expect("la ventana de prueba se cerró")
}

/// Al arrastrar el divisor del historial, las cabeceras de columna se
/// quedaban con el ancho del frame anterior (AUTHOR y DATE sobre una lista
/// que ya no los tenía) hasta el siguiente repintado. Ahora se recolocan en el
/// mismo frame.
#[gpui_kit::test]
async fn the_column_headers_follow_the_width_in_the_same_frame(cx: &mut TestAppContext) {
    let app = TestApp::with_width(1200., cx);
    let pane = app.mount(cx, |window, cx| Pane {
        width: 900.,
        list: cx.new(|cx| CommitHistoryList::new(window, cx)),
    });
    let list = pane.read_with(cx, |pane, _| pane.list.clone());
    show(&list, history(5, Some(0)), cx);
    assert!(app.exists(cx, "column:author"));
    assert!(app.exists(cx, "column:date"));

    // Estrecho: sin autor ni fecha, ya en el primer frame.
    assert_eq!(
        header_columns_after_one_frame(&app, &pane, 300., cx),
        (false, false)
    );
    // Intermedio: la fecha cabe, el autor no.
    assert_eq!(
        header_columns_after_one_frame(&app, &pane, 450., cx),
        (false, true)
    );
    // Ancho otra vez: las dos.
    assert_eq!(
        header_columns_after_one_frame(&app, &pane, 900., cx),
        (true, true)
    );
}

/// `main` y `origin/main` en el mismo commit: una sola etiqueta con la nube
/// del remoto. El doble clic en una etiqueta de rama pide el checkout de esa
/// rama (y no el del commit: el segundo clic no llega a la fila); en un tag,
/// sigue siendo el de la fila.
#[gpui_kit::test]
async fn a_double_click_on_a_branch_label_asks_to_check_out_that_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let list = mount_list(&app, cx);
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&list, move |_, event: &CommitHistoryListEvent, _| {
            events.borrow_mut().push(match event {
                CommitHistoryListEvent::SelectCommit(_) => "select".to_string(),
                CommitHistoryListEvent::CheckoutCommit(_) => "checkout".to_string(),
                CommitHistoryListEvent::CheckoutRef { commit, name, kind } => {
                    format!("checkout-ref {name} {} @{}", kind.as_str(), commit.hash)
                }
                _ => "other".to_string(),
            });
        })
        .detach();
    });
    show(
        &list,
        vec![commit(
            "abc1234",
            "Some work",
            vec![
                reference("feature", RefType::Local, false),
                reference("origin/feature", RefType::Remote, false),
                reference("origin/other", RefType::Remote, false),
                reference("v1.0", RefType::Tag, false),
            ],
        )],
        cx,
    );
    app.in_window(cx, |_, _| {});

    // `feature` y `origin/feature`: una etiqueta, con su nube.
    let (labels, cloud) = app.in_window(cx, |window, _| {
        let scope = window.within(row("abc1234"));
        let labels: Vec<String> = (0usize..3)
            .filter_map(|index| scope.try_find(("ref", index)))
            .filter_map(|pill| pill.label().map(str::to_string))
            .collect();
        (labels, scope.try_find("ref-remote:origin").is_some())
    });
    assert_eq!(labels, ["feature", "origin/other", "v1.0"]);
    assert!(cloud);

    app.in_window(cx, |window, cx| {
        window
            .within(row("abc1234"))
            .double_click(("ref", 0usize), cx)
    });
    assert_eq!(
        *events.borrow(),
        ["select", "checkout-ref feature local @abc1234"]
    );
    events.borrow_mut().clear();

    app.in_window(cx, |window, cx| {
        window
            .within(row("abc1234"))
            .double_click(("ref", 1usize), cx)
    });
    assert_eq!(
        *events.borrow(),
        ["select", "checkout-ref origin/other remote @abc1234"]
    );
    events.borrow_mut().clear();

    // Un tag no es una rama: el doble clic sigue siendo el de la fila.
    app.in_window(cx, |window, cx| {
        window
            .within(row("abc1234"))
            .double_click(("ref", 2usize), cx)
    });
    assert_eq!(*events.borrow(), ["select", "select", "checkout"]);
}

#[test]
fn the_tooltips_name_every_ref_and_offer_the_double_click() {
    let refs = vec![
        reference("main", RefType::Local, true),
        reference("origin/main", RefType::Remote, false),
        reference("feature", RefType::Local, false),
        reference("v1.0", RefType::Tag, false),
    ];
    // El "+N" lista todas las refs del commit, no solo las que no caben.
    assert_eq!(
        refs_tooltip(&refs),
        "main (HEAD)\norigin/main (remote)\nfeature\nv1.0 (tag)"
    );
    assert_eq!(
        refs_tooltip(&[reference("HEAD", RefType::Head, true)]),
        "HEAD (detached)"
    );

    let badges = group_ref_badges(&refs);
    assert_eq!(
        ref_badge_title(&badges[0]).as_deref(),
        Some("You are here · HEAD is on main · In sync with origin/main")
    );
    assert_eq!(
        ref_badge_title(&badges[1]).as_deref(),
        Some("Double-click to check out")
    );
    assert_eq!(ref_badge_title(&badges[2]), None);
}

/// Cuenta las veces que se mide un nombre de ref.
fn counting(count: &Rc<Cell<usize>>) -> impl Fn(&str) -> f64 + 'static {
    let count = count.clone();
    move |text| {
        count.set(count.get() + 1);
        measured(text)
    }
}

/// Las filas colocan sus badges una vez: desplazar la lista o pasar el
/// ratón (cada frame vuelve a armar las filas) no mide de nuevo los nombres.
/// Otro ancho, otra fuente u otras refs sí invalidan lo guardado.
#[gpui_kit::test]
async fn caches_the_badge_layout_of_each_row_until_the_width_or_its_refs_change(
    cx: &mut TestAppContext,
) {
    let app = TestApp::with_width(1200., cx);
    let list = mount_list(&app, cx);
    let row = commit(
        "abc1234",
        "Some work",
        vec![
            reference("main", RefType::Local, true),
            reference("feature/a-very-long-branch-name", RefType::Local, false),
        ],
    );
    let plain = commit("def5678", "No refs here", Vec::new());
    show(&list, vec![row.clone(), plain.clone()], cx);
    app.in_window(cx, |_, _| {});
    app.in_window(cx, |_, _| {});
    let font = cx.update(|cx| cx.theme().mono_font_family.clone());
    let measures = Rc::new(Cell::new(0));

    // La fila pintada ya lo guardó (la que no tiene refs no guarda nada).
    assert_eq!(list.read_with(cx, |list, _| list.cached_badge_layouts()), 1);
    let first = list.read_with(cx, |list, _| {
        list.cached_badge_layout(&row, &font, counting(&measures))
    });
    assert_eq!(measures.get(), 0, "the painted layout was not reused");
    assert_eq!(first.hidden, 0);
    assert_eq!(first.visible.len(), 2);

    // Más frames (scroll, hover): lo mismo, sin medir.
    for _ in 0..3 {
        app.in_window(cx, |_, _| {});
    }
    let again = list.read_with(cx, |list, _| {
        list.cached_badge_layout(&row, &font, counting(&measures))
    });
    assert!(Rc::ptr_eq(&first, &again));
    assert_eq!(measures.get(), 0);

    // Otra fuente: se vuelve a colocar.
    let other_font = SharedString::from("Some Other Mono");
    list.read_with(cx, |list, _| {
        list.cached_badge_layout(&row, &other_font, counting(&measures))
    });
    assert!(measures.get() > 0, "another font reused the old layout");
    app.in_window(cx, |_, _| {});

    // La lista se estrecha: la fila vuelve a colocar sus badges y la rama
    // larga ya no cabe.
    cx.simulate_window_resize(app.window, size(px(300.), px(800.)));
    app.in_window(cx, |_, _| {});
    app.in_window(cx, |_, _| {});
    measures.set(0);
    let narrow = list.read_with(cx, |list, _| {
        list.cached_badge_layout(&row, &font, counting(&measures))
    });
    assert_eq!(measures.get(), 0, "the row did not cache its new layout");
    assert!(!Rc::ptr_eq(&first, &narrow));
    assert_eq!(narrow.visible.len(), 1);
    assert_eq!(narrow.hidden, 1);

    // El mismo commit con otras refs, aunque no pase por `set_commits`.
    let retagged = commit(
        "abc1234",
        "Some work",
        vec![reference("main", RefType::Local, true)],
    );
    let layout = list.read_with(cx, |list, _| {
        list.cached_badge_layout(&retagged, &font, counting(&measures))
    });
    assert!(
        !Rc::ptr_eq(&narrow, &layout),
        "other refs reused the old layout"
    );
    assert_eq!(layout.hidden, 0);
    assert_eq!(layout.visible.len(), 1);

    // Commits nuevos: nada de lo guardado sobrevive.
    let moved = commit(
        "abc1234",
        "Some work",
        vec![reference("release", RefType::Local, false)],
    );
    show(&list, vec![moved.clone(), plain], cx);
    app.in_window(cx, |_, _| {});
    assert_eq!(list.read_with(cx, |list, _| list.cached_badge_layouts()), 1);
    measures.set(0);
    let layout = list.read_with(cx, |list, _| {
        list.cached_badge_layout(&moved, &font, counting(&measures))
    });
    assert_eq!(measures.get(), 0);
    let names: Vec<&str> = layout
        .visible
        .iter()
        .map(|badge| badge.name.as_str())
        .collect();
    assert_eq!(names, ["release"]);
}
