//! Tests de interacción de los visores: ventanas sin pantalla con la misma
//! raíz que la app (`mergeon_ui::init` + `gpui_kit::open_window`), clics, rueda
//! y teclas reales, y comprobación de lo que recibe quien usa cada visor.
//!
//! Portan la intención de `FileDiffViewer.test.js` y de lo que
//! `ChangesView.test.js`, `CommitChangesTab.test.js` y `StashDiffDialog.test.js`
//! esperan de estos componentes. `highlight.test.js` se prueba junto a su
//! módulo (`src/highlight.rs`).

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem, Context, Entity,
    InputEvent as _, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
    Styled as _, Subscription, TestAppContext, Window, WindowBounds, WindowOptions, div, point, px,
    size,
};
use mergeon_model::diff::{DiffLineType, SplitCellType, inline_segments};
use mergeon_model::patch::{PatchDirection, extract_hunk_patch, extract_lines_patch};
use mergeon_view_diff::highlight::{Language, TokenKind};
use mergeon_view_diff::{
    BlameLine, BlameState, BlameViewer, BlameViewerEvent, DiffPanel, DiffViewMode, DiffViewer,
    DiffViewerEvent, FileContent, FileContentViewer, FileContentViewerEvent, FileTreeRow,
    HunkAction, ImageDiff, ImageDiffViewer, ImageVersion, LinesRequest, PatchAction,
    ScrollPosition, file_stats, ignore_whitespace_tooltip, panel_status_label, view_toggle_label,
    whole_file_tooltip,
};

const DIFF: &str = "diff --git a/src/app.js b/src/app.js\n\
index 4f77f5a..b3eda6e 100644\n\
--- a/src/app.js\n\
+++ b/src/app.js\n\
@@ -1,3 +1,3 @@\n   const indentado = true\n-call(alpha)\n+call(gamma)\n";

const TWO_HUNKS: &str = "diff --git a/src/app.js b/src/app.js\n\
index 4f77f5a..b3eda6e 100644\n\
--- a/src/app.js\n\
+++ b/src/app.js\n\
@@ -1,1 +1,1 @@\n-uno\n+UNO\n@@ -20,1 +20,1 @@\n-doce\n+DOCE\n";

/// Vista de prueba: dibuja lo que le pida el test en cada frame.
struct Host {
    content: HostContent,
}

type HostContent = Box<dyn Fn(&mut Window, &mut Context<Host>) -> AnyElement>;

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child((self.content)(window, cx))
    }
}

const WINDOW_WIDTH: f32 = 900.;
const WINDOW_HEIGHT: f32 = 600.;

/// Abre una ventana como la de la app y devuelve lo que construya `build`
/// (las entidades de los visores) para que el test las consulte.
fn open<T: 'static>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut Context<Host>) -> (T, HostContent) + 'static,
) -> (AnyWindowHandle, T) {
    cx.update(|cx| {
        mergeon_ui::init(cx);
        mergeon_view_diff::init(cx);
        cx.set_reduce_motion(true);
        let mut built = None;
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let (value, content) = build(window, cx);
                    built = Some(value);
                    Host { content }
                })
            },
        )
        .expect("no se pudo abrir la ventana de prueba");
        (window, built.expect("la vista de prueba se construyó"))
    })
}

/// El contenido de una ventana que solo pinta una entidad.
fn showing<V: Render>(entity: &Entity<V>) -> HostContent {
    let entity = entity.clone();
    Box::new(move |_, _| entity.clone().into_any_element())
}

/// Una ventana con un visor de diff a todo su tamaño.
fn open_diff(
    cx: &mut TestAppContext,
    diff: &str,
    path: &str,
    mode: DiffViewMode,
    action: Option<HunkAction>,
) -> (AnyWindowHandle, Entity<DiffViewer>) {
    let (diff, path) = (diff.to_string(), path.to_string());
    open(cx, move |window, cx| {
        let viewer = cx.new(|cx| {
            let mut viewer = DiffViewer::new(window, cx);
            viewer.set_diff(diff, path, cx);
            viewer.set_view_mode(mode, cx);
            viewer.set_hunk_action(action, cx);
            viewer
        });
        let content = showing(&viewer);
        (viewer, content)
    })
}

/// Ejecuta `f` dentro de la ventana, con un frame ya dibujado.
fn in_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    let result = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            f(window, cx)
        })
        .expect("la ventana de prueba se cerró");
    cx.run_until_parked();
    result
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    // Dos frames: el segundo recoge lo que el primero midió.
    in_window(cx, window, |_, _| {});
    in_window(cx, window, |_, _| {});
}

fn move_to(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
    window.dispatch_event(
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }
        .to_platform_input(),
        cx,
    );
}

fn click(window: &mut Window, cx: &mut App, x: f32, y: f32, count: usize) {
    let position = point(px(x), px(y));
    move_to(window, cx, position);
    for click_count in 1..=count {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count,
            }
            .to_platform_input(),
            cx,
        );
    }
    window.render_frame(cx);
}

/// Lleva el cursor a una fila del visor: su acción de bloque solo se enseña
/// (y solo entonces se puede pulsar) con el cursor encima de la cabecera.
fn hover(window: &mut Window, cx: &mut App, x: f32, y: f32) {
    move_to(window, cx, point(px(x), px(y)));
    window.render_frame(cx);
}

fn wheel(window: &mut Window, cx: &mut App, x: f32, y: f32, delta_x: f32, delta_y: f32) {
    let position = point(px(x), px(y));
    move_to(window, cx, position);
    window.dispatch_event(
        ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(delta_x), px(delta_y))),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

/// Recoge los eventos del visor de diff mientras viva la suscripción.
fn record_events(
    cx: &mut TestAppContext,
    viewer: &Entity<DiffViewer>,
) -> (Rc<RefCell<Vec<DiffViewerEvent>>>, Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let subscription = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(viewer, move |_, event: &DiffViewerEvent, _| {
            events.borrow_mut().push(event.clone());
        })
    });
    (events, subscription)
}

fn changed_pieces(viewer: &DiffViewer) -> Vec<String> {
    (0..viewer.lines().len())
        .flat_map(|index| viewer.line_segments(index))
        .filter(|segment| segment.changed)
        .map(|segment| segment.text)
        .collect()
}

// ---------------------------------------------------------------------------
// FileDiffViewer.test.js
// ---------------------------------------------------------------------------

// Lo que se pinta de una línea es su texto y nada más: ni el signo ni sangría
// de más.
#[gpui_kit::test]
async fn renders_a_line_with_exactly_its_own_text(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        let kinds: Vec<DiffLineType> = viewer.lines().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            [
                DiffLineType::Hunk,
                DiffLineType::Ctx,
                DiffLineType::Del,
                DiffLineType::Add
            ]
        );
        let context: String = viewer
            .line_segments(1)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(context, "  const indentado = true");
        // Las cuatro filas caben: se pintan todas.
        assert_eq!(viewer.painted_rows(), 0..4);
    });
}

#[gpui_kit::test]
async fn marks_only_the_part_of_the_line_that_changed(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        assert_eq!(changed_pieces(viewer.read(cx)), ["alpha", "gamma"]);
    });
}

#[gpui_kit::test]
async fn does_the_same_in_the_split_view(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Split, None);
    draw(cx, window);

    let rows = cx.update(|cx| viewer.update(cx, |viewer, _| viewer.split_rows()));
    let changed: Vec<String> = rows
        .iter()
        .flat_map(|row| [&row.left, &row.right])
        .flat_map(|cell| inline_segments(&cell.content, cell.inline))
        .filter(|segment| segment.changed)
        .map(|segment| segment.text)
        .collect();
    assert_eq!(changed, ["alpha", "gamma"]);

    // La línea de contexto sale igual en los dos lados, sin espacios de más.
    assert!(
        rows.iter()
            .any(|row| row.left.content == "  const indentado = true"
                && row.right.content == "  const indentado = true")
    );
    // Cabecera, contexto y la línea modificada emparejada: tres filas.
    cx.update(|cx| {
        assert_eq!(viewer.read(cx).painted_rows(), 0..3);
        assert_eq!(viewer.read(cx).view_mode(), DiffViewMode::Split);
    });
}

// El color sale de la extensión del archivo, no de adivinar el contenido.
#[gpui_kit::test]
async fn colours_the_code_with_the_language_of_the_file(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.language(), Some(Language::JavaScript));
        assert!(
            viewer
                .line_segments(1)
                .iter()
                .any(|segment| segment.token == Some(TokenKind::Keyword))
        );
    });
}

#[gpui_kit::test]
async fn leaves_the_code_plain_when_the_file_has_no_known_language(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "notas.desconocido", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.language(), None);
        let segments = viewer.line_segments(1);
        assert!(segments.iter().all(|segment| segment.token.is_none()));
        assert_eq!(segments[0].text, "  const indentado = true");
    });
}

// Colorear y marcar el cambio se aplican al mismo texto: si compitieran, la
// línea saldría rota.
#[gpui_kit::test]
async fn colours_and_marks_the_change_at_the_same_time(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(changed_pieces(viewer), ["alpha", "gamma"]);
        let deleted: String = viewer
            .line_segments(2)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(deleted, "call(alpha)");
    });
}

// Un archivo de dos dígitos no tiene por qué reservar sitio para cinco.
#[gpui_kit::test]
async fn sizes_the_line_gutter_to_the_longest_number_in_the_diff(cx: &mut TestAppContext) {
    let (_, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    cx.update(|cx| assert_eq!(viewer.read(cx).gutter_digits(), 2));

    let (_, viewer) = open_diff(
        cx,
        "diff --git a/x b/x\n@@ -12000,1 +12000,1 @@\n ok\n",
        "x",
        DiffViewMode::Unified,
        None,
    );
    cx.update(|cx| assert_eq!(viewer.read(cx).gutter_digits(), 5));
}

#[gpui_kit::test]
async fn says_so_when_there_is_nothing_to_show(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, "", "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(viewer.is_empty());
        assert_eq!(viewer.lines().len(), 0);
        // Sin diff no hay lienzo: no se pinta ninguna fila.
        assert_eq!(viewer.painted_rows(), 0..0);
    });
}

// Un diff sin bloques (un binario) no es "sin cambios": el visor queda vacío.
#[gpui_kit::test]
async fn shows_nothing_for_a_diff_without_hunks(cx: &mut TestAppContext) {
    let binary =
        "diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n";
    let (window, viewer) = open_diff(cx, binary, "logo.png", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(!viewer.is_empty());
        assert_eq!(viewer.lines().len(), 0);
        assert_eq!(viewer.hunk_count(), 0);
    });
}

// ---------------------------------------------------------------------------
// FileDiffViewer: staging one block
// ---------------------------------------------------------------------------

// En el detalle de un commit no hay nada que preparar.
#[gpui_kit::test]
async fn offers_nothing_when_the_caller_does_not_ask_for_it(cx: &mut TestAppContext) {
    let (window, _viewer) = open_diff(cx, TWO_HUNKS, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    in_window(cx, window, |window, _| {
        assert!(window.try_find(("hunk-action", 0usize)).is_none());
        assert!(window.try_find(("hunk-action", 1usize)).is_none());
    });
}

#[gpui_kit::test]
async fn offers_one_action_per_block(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    draw(cx, window);

    in_window(cx, window, |window, _| {
        for index in 0..2usize {
            let button = window.find(("hunk-action", index));
            assert_eq!(button.label(), Some("Stage hunk"));
            assert!(button.bounds().size.width > px(0.));
        }
        assert!(window.try_find(("hunk-action", 2usize)).is_none());
    });
    cx.update(|cx| assert_eq!(viewer.read(cx).hunk_count(), 2));
}

// Antes cada fila medía lo que la línea más larga: con líneas cortas los
// fondos y las cabeceras de hunk terminaban a media ventana y el botón del
// bloque quedaba pegado al código, con el resto del panel vacío.
#[gpui_kit::test]
async fn rows_fill_the_panel_even_when_every_line_is_short(cx: &mut TestAppContext) {
    let (window, _viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    draw(cx, window);

    in_window(cx, window, |window, _| {
        let button = window.find(("hunk-action", 0usize)).bounds();
        // El botón va al final de la fila: ahora, junto al borde derecho.
        assert!(
            button.right() > px(WINDOW_WIDTH - 40.),
            "botón en {button:?} con ventana de {WINDOW_WIDTH}px"
        );
    });
}

// El índice es lo que identifica al bloque cuando se construye el parche.
#[gpui_kit::test]
async fn says_which_block_was_chosen_with_its_patch(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    // La segunda cabecera: tras la primera (20px) y sus dos líneas (18px).
    in_window(cx, window, |window, cx| {
        hover(window, cx, 60., 66.);
        window.click(("hunk-action", 1usize), cx);
    });

    let requests: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            DiffViewerEvent::ApplyHunk(request) => Some(request.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.index, 1);
    assert_eq!(request.action, PatchAction::Stage);
    assert!(!request.action.is_reverse());

    // El parche es el de `mergeon_model::patch`: cabecera del archivo y solo
    // el segundo bloque.
    assert_eq!(request.patch, extract_hunk_patch(TWO_HUNKS, 1));
    let patch = request.patch.as_deref().expect("el diff trae cabecera");
    assert!(patch.starts_with("diff --git a/src/app.js b/src/app.js\n"));
    assert!(patch.contains("@@ -20,1 +20,1 @@\n-doce\n+DOCE\n"));
    assert!(!patch.contains("-uno"));
    assert!(patch.ends_with('\n'));
}

#[gpui_kit::test]
async fn says_unstage_when_the_file_is_already_staged(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Unstage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        assert_eq!(
            window.find(("hunk-action", 0usize)).label(),
            Some("Unstage hunk")
        );
        assert_eq!(
            window.find(("hunk-action", 1usize)).label(),
            Some("Unstage hunk")
        );
        hover(window, cx, 60., 10.);
        window.click(("hunk-action", 0usize), cx);
    });

    let events = events.borrow();
    let Some(DiffViewerEvent::ApplyHunk(request)) = events
        .iter()
        .find(|event| matches!(event, DiffViewerEvent::ApplyHunk(_)))
    else {
        panic!("el clic no pidió aplicar el bloque: {events:?}");
    };
    assert_eq!(request.index, 0);
    assert_eq!(request.action, PatchAction::Unstage);
    // Quitar del stage es aplicar el parche al revés.
    assert!(request.action.is_reverse());
}

// La acción de bloque solo existe en la vista unificada.
#[gpui_kit::test]
async fn offers_no_block_action_in_the_split_view(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Split,
        Some(HunkAction::Stage),
    );
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find(("hunk-action", 0usize)).is_none());
    });

    // Al volver a la vista unificada reaparece.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Unified, cx)
        })
    });
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find(("hunk-action", 0usize)).is_some());
    });
}

#[gpui_kit::test]
async fn a_diff_without_header_has_no_patch_to_apply(cx: &mut TestAppContext) {
    let headless = "@@ -1,1 +1,1 @@\n-uno\n+UNO\n";
    let (window, viewer) = open_diff(
        cx,
        headless,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        hover(window, cx, 60., 10.);
        window.click(("hunk-action", 0usize), cx);
    });

    let events = events.borrow();
    assert!(events.iter().any(|event| matches!(
        event,
        DiffViewerEvent::ApplyHunk(request) if request.index == 0 && request.patch.is_none()
    )));
}

// ---------------------------------------------------------------------------
// Descartar un bloque y elegir líneas sueltas
// ---------------------------------------------------------------------------

/// Filas: 0 `@@` (22px), 1 ` uno`, 2 `-dos`, 3 `+DOS`, 4 `+tres bis`,
/// 5 ` cuatro`, 6 `-cinco` (20px cada una), bajo el borde de 1px del visor.
const MIXED: &str = "diff --git a/f.txt b/f.txt\n\
index 1111111..2222222 100644\n\
--- a/f.txt\n\
+++ b/f.txt\n\
@@ -1,4 +1,4 @@\n uno\n-dos\n+DOS\n+tres bis\n cuatro\n-cinco\n";

/// Centro vertical de la fila `row` de `MIXED`.
fn row_y(row: usize) -> f32 {
    if row == 0 {
        12.
    } else {
        23. + 20. * (row - 1) as f32 + 10.
    }
}

/// Un punto del medianil (los números de línea).
const GUTTER_X: f32 = 10.;

fn press_at(window: &mut Window, cx: &mut App, x: f32, y: f32, modifiers: Modifiers) {
    let position = point(px(x), px(y));
    move_to(window, cx, position);
    window.dispatch_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

fn lines_requests(events: &RefCell<Vec<DiffViewerEvent>>) -> Vec<LinesRequest> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            DiffViewerEvent::ApplyLines(request) => Some(request.clone()),
            _ => None,
        })
        .collect()
}

fn selected_lines(cx: &mut TestAppContext, viewer: &Entity<DiffViewer>) -> Vec<usize> {
    cx.update(|cx| viewer.read(cx).selected_lines())
}

#[gpui_kit::test]
async fn offers_discard_next_to_stage_on_unstaged_changes(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        for index in [0usize, 1] {
            assert_eq!(
                window.find(("hunk-discard", index)).label(),
                Some("Discard hunk")
            );
        }
        // "Discard hunk" va a la izquierda de "Stage hunk", que sigue al borde.
        let discard = window.find(("hunk-discard", 1usize)).bounds();
        let stage = window.find(("hunk-action", 1usize)).bounds();
        assert!(discard.right() <= stage.left(), "{discard:?} {stage:?}");
        hover(window, cx, 60., 66.);
        window.click(("hunk-discard", 1usize), cx);
    });

    let events = events.borrow();
    let requests: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            DiffViewerEvent::ApplyHunk(request) => Some(request.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].index, 1);
    assert_eq!(requests[0].action, PatchAction::Discard);
    assert!(requests[0].action.is_reverse());
    assert!(requests[0].action.touches_worktree());
    assert_eq!(requests[0].patch, extract_hunk_patch(TWO_HUNKS, 1));
}

// Lo preparado no se descarta desde aquí: solo se quita del stage.
#[gpui_kit::test]
async fn offers_no_discard_on_staged_changes(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        TWO_HUNKS,
        "src/app.js",
        DiffViewMode::Unified,
        Some(HunkAction::Unstage),
    );
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find(("hunk-discard", 0usize)).is_none());
        assert!(window.try_find(("hunk-action", 0usize)).is_some());
    });
    cx.update(|cx| {
        assert_eq!(viewer.read(cx).patch_actions(), [PatchAction::Unstage]);
        // Un descarte pedido por código tampoco sale.
        viewer.update(cx, |viewer, cx| {
            viewer.apply_hunk_action(0, PatchAction::Discard, cx)
        });
    });
}

#[gpui_kit::test]
async fn picks_lines_from_the_gutter_and_stages_them(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    // Un clic en el medianil elige la línea; con mayúsculas se extiende.
    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(2), Modifiers::default());
    });
    assert_eq!(selected_lines(cx, &viewer), [2]);
    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(3), shift());
    });
    assert_eq!(selected_lines(cx, &viewer), [2, 3]);
    // No es una selección de texto.
    cx.update(|cx| assert_eq!(viewer.read(cx).selected_text(), None));
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        assert_eq!(window.find("lines-stage").label(), Some("Stage lines"));
        assert_eq!(window.find("lines-discard").label(), Some("Discard lines"));
        assert!(window.try_find("lines-unstage").is_none());
        // La barra va sobre la primera línea elegida.
        let bar = window.find("lines-stage").bounds();
        assert!(bar.top() >= px(43.) && bar.bottom() <= px(63.), "{bar:?}");
        window.click("lines-stage", cx);
    });

    let requests = lines_requests(&events);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].action, PatchAction::Stage);
    assert_eq!(requests[0].lines, [2, 3]);
    assert_eq!(
        requests[0].patch,
        extract_lines_patch(MIXED, &[2, 3], PatchDirection::Forward)
    );
    let patch = requests[0].patch.as_deref().expect("hay parche");
    assert!(
        patch.contains("@@ -1,4 +1,4 @@\n uno\n-dos\n+DOS\n cuatro\n cinco\n"),
        "{patch}"
    );
}

#[gpui_kit::test]
async fn dragging_down_the_gutter_picks_every_change_in_between(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        window.drag(
            point(px(GUTTER_X), px(row_y(1))),
            point(px(GUTTER_X), px(row_y(5))),
            cx,
        );
    });
    // El contexto (filas 1 y 5) no cuenta.
    assert_eq!(selected_lines(cx, &viewer), [2, 3, 4]);

    // Un clic en el medianil de la cabecera elige todo su bloque.
    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(0), Modifiers::default());
    });
    assert_eq!(selected_lines(cx, &viewer), [2, 3, 4, 6]);
}

#[gpui_kit::test]
async fn the_code_or_escape_drop_the_picked_lines(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(2), Modifiers::default());
    });
    assert_eq!(selected_lines(cx, &viewer), [2]);
    // Un clic en el código vuelve a la selección de texto.
    in_window(cx, window, |window, cx| {
        click(window, cx, 300., row_y(4), 1)
    });
    assert!(selected_lines(cx, &viewer).is_empty());
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("lines-stage").is_none())
    });

    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(6), Modifiers::default());
        window.press("escape", cx);
    });
    assert!(selected_lines(cx, &viewer).is_empty());

    // Un diff nuevo tampoco hereda la selección.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.select_lines(2..=3, cx);
            assert_eq!(viewer.selected_lines(), [2, 3]);
            viewer.set_diff(TWO_HUNKS, "f.txt", cx);
            assert!(viewer.selected_lines().is_empty());
        })
    });
}

#[gpui_kit::test]
async fn keys_apply_the_offered_action_to_the_picked_lines(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    // Sin líneas elegidas, las teclas no hacen nada.
    in_window(cx, window, |window, cx| {
        click(window, cx, 300., row_y(4), 1);
        window.press("s", cx);
    });
    assert!(lines_requests(&events).is_empty());

    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(6), Modifiers::default());
        window.press("u", cx);
        window.press("d", cx);
        window.press("s", cx);
    });
    let requests = lines_requests(&events);
    let actions: Vec<_> = requests.iter().map(|request| request.action).collect();
    // `U` no se ofrece sobre lo que no está preparado.
    assert_eq!(actions, [PatchAction::Discard, PatchAction::Stage]);
    assert_eq!(
        requests[0].patch,
        extract_lines_patch(MIXED, &[6], PatchDirection::Reverse)
    );
    assert_eq!(
        requests[1].patch,
        extract_lines_patch(MIXED, &[6], PatchDirection::Forward)
    );
}

/// Las pistas enseñan `S` / `U` / `D` en mayúscula: con Shift hacen lo mismo.
#[gpui_kit::test]
async fn the_line_keys_also_work_with_shift(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(6), Modifiers::default());
        window.press("shift-d", cx);
        window.press("shift-s", cx);
        // Con otro modificador ya no es la tecla de las líneas.
        window.press("ctrl-s", cx);
        window.press("alt-d", cx);
    });
    let actions: Vec<_> = lines_requests(&events)
        .iter()
        .map(|request| request.action)
        .collect();
    assert_eq!(actions, [PatchAction::Discard, PatchAction::Stage]);

    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Unstage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(3), Modifiers::default());
        window.press("shift-u", cx);
    });
    let actions: Vec<_> = lines_requests(&events)
        .iter()
        .map(|request| request.action)
        .collect();
    assert_eq!(actions, [PatchAction::Unstage]);
}

#[gpui_kit::test]
async fn staged_lines_can_only_be_unstaged(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Unstage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(3), Modifiers::default());
        press_at(window, cx, GUTTER_X, row_y(4), shift());
    });
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        assert!(window.try_find("lines-stage").is_none());
        assert!(window.try_find("lines-discard").is_none());
        window.press("s", cx);
        window.press("d", cx);
        window.click("lines-unstage", cx);
    });

    let requests = lines_requests(&events);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].action, PatchAction::Unstage);
    assert_eq!(requests[0].lines, [3, 4]);
    assert_eq!(
        requests[0].patch,
        extract_lines_patch(MIXED, &[3, 4], PatchDirection::Reverse)
    );
}

// Sin acción de bloque (detalle de un commit) o en la vista partida, el
// medianil no elige líneas.
#[gpui_kit::test]
async fn the_gutter_picks_nothing_where_nothing_can_be_applied(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, MIXED, "f.txt", DiffViewMode::Unified, None);
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(2), Modifiers::default());
    });
    assert!(selected_lines(cx, &viewer).is_empty());

    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Split,
        Some(HunkAction::Stage),
    );
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        press_at(window, cx, GUTTER_X, row_y(2), Modifiers::default());
    });
    assert!(selected_lines(cx, &viewer).is_empty());
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.select_lines(2..=3, cx);
            assert!(viewer.selected_lines().is_empty());
        })
    });
}

#[test]
fn patch_actions_say_what_they_do() {
    assert_eq!(PatchAction::Discard.hunk_label(), "Discard hunk");
    assert_eq!(PatchAction::Stage.lines_label(), "Stage lines");
    assert_eq!(PatchAction::Unstage.lines_label(), "Unstage lines");
    assert_eq!(PatchAction::Discard.lines_label(), "Discard lines");
    assert_eq!(PatchAction::Stage.direction(), PatchDirection::Forward);
    assert_eq!(PatchAction::Unstage.direction(), PatchDirection::Reverse);
    assert_eq!(PatchAction::Discard.direction(), PatchDirection::Reverse);
    assert!(!PatchAction::Unstage.touches_worktree());
    assert_eq!(
        HunkAction::Stage.patch_actions(),
        [PatchAction::Discard, PatchAction::Stage]
    );
    assert_eq!(HunkAction::Unstage.patch_actions(), [PatchAction::Unstage]);
    assert_eq!(HunkAction::Unstage.primary(), PatchAction::Unstage);
}

#[test]
fn hunk_actions_keep_the_texts_of_the_original() {
    assert_eq!(HunkAction::Stage.label(), "Stage hunk");
    assert_eq!(HunkAction::Unstage.label(), "Unstage hunk");
    assert_eq!(HunkAction::Stage.tooltip(), "Stage only this hunk");
    assert_eq!(HunkAction::Unstage.tooltip(), "Unstage only this hunk");
    assert_eq!(HunkAction::parse("stage"), Some(HunkAction::Stage));
    assert_eq!(HunkAction::parse("unstage"), Some(HunkAction::Unstage));
    assert_eq!(HunkAction::parse("discard"), None);
    assert_eq!(HunkAction::Unstage.as_str(), "unstage");
}

// ---------------------------------------------------------------------------
// Virtualización, scroll, selección y copia
// ---------------------------------------------------------------------------

fn large_diff(lines: usize) -> String {
    let mut diff = String::from("diff --git a/big.js b/big.js\n--- a/big.js\n+++ b/big.js\n");
    diff.push_str(&format!("@@ -1,{lines} +1,{lines} @@\n"));
    for line in 1..lines {
        diff.push_str(&format!(" const value{line} = compute({line})\n"));
    }
    diff
}

// "Open in editor": la línea de la selección o, sin ella, la primera visible.
#[gpui_kit::test]
async fn the_editor_line_follows_the_scroll_and_the_selection(cx: &mut TestAppContext) {
    let diff = large_diff(2_000);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    let editor_line = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            viewer.update(cx, |viewer, _| {
                (viewer.painted_rows(), viewer.editor_line())
            })
        })
    };

    // Arriba, la cabecera del bloque no está en el archivo: la línea 1.
    assert_eq!(editor_line(cx).1, Some(1));

    in_window(cx, window, |window, cx| {
        wheel(window, cx, 400., 300., 0., -9_000.);
    });
    draw(cx, window);
    // La fila i (tras la cabecera) es la línea i del archivo.
    let (rows, line) = editor_line(cx);
    assert!(rows.start > 100, "{rows:?}");
    assert_eq!(line, Some(rows.start as u32));

    // Con una selección manda su principio.
    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.select_all(cx)));
    assert_eq!(editor_line(cx).1, Some(1));
}

// `useVirtualRows`: solo existen las filas que caben en el visor.
#[gpui_kit::test]
async fn only_builds_the_rows_inside_the_viewport(cx: &mut TestAppContext) {
    let diff = large_diff(20_000);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    let first = cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert_eq!(viewer.row_count(), 20_000);
            viewer.painted_rows()
        })
    });
    assert_eq!(first.start, 0);
    // 600px de alto a 18px por fila: unas 34 filas, no 20.000.
    assert!(first.len() <= 40, "{first:?}");

    in_window(cx, window, |window, cx| {
        wheel(window, cx, 400., 300., 0., -90_000.);
    });
    draw(cx, window);

    let (scrolled, position) = cx.update(|cx| {
        let viewer = viewer.read(cx);
        (viewer.painted_rows(), viewer.scroll_position())
    });
    assert!(scrolled.len() <= 40, "{scrolled:?}");
    assert!(scrolled.start > 4_000, "{scrolled:?}");
    assert_eq!(position.top, 90_000.);
    assert!(
        events
            .borrow()
            .contains(&DiffViewerEvent::Scrolled(position)),
        "el scroll se avisa a quien aloja al visor"
    );

    // La vista partida tampoco monta más de lo que se ve.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx)
        })
    });
    draw(cx, window);
    let split = cx.update(|cx| viewer.read(cx).painted_rows());
    assert!(split.len() <= 40, "{split:?}");
}

// Un diff de 20.000 líneas se dibuja en lo que tarda un frame.
#[gpui_kit::test]
async fn draws_a_huge_diff_without_walking_all_of_it(cx: &mut TestAppContext) {
    let diff = large_diff(20_000);
    let (window, _viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);

    let started = std::time::Instant::now();
    let frames = 30;
    for step in 0..frames {
        in_window(cx, window, |window, cx| {
            wheel(window, cx, 400., 300., 0., -(step as f32) * 317.);
        });
    }
    let per_frame = started.elapsed() / frames;
    // Margen amplio: en esta máquina (sin optimizar y sin GPU) son ~5ms.
    assert!(
        per_frame < std::time::Duration::from_millis(100),
        "cada paso de scroll tardó {per_frame:?}"
    );
}

#[gpui_kit::test]
async fn restores_a_saved_scroll_position(cx: &mut TestAppContext) {
    let diff = large_diff(2_000);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_scroll_position(ScrollPosition::new(1_800., 0.), cx)
        })
    });
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.scroll_position().top, 1_800.);
        assert!(viewer.painted_rows().contains(&100));
        assert!(!viewer.painted_rows().contains(&0));
    });

    // Más allá del final se queda en el último tramo.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_scroll_position(ScrollPosition::new(9_999_999., 0.), cx)
        })
    });
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(viewer.painted_rows().contains(&1_999));
        // Líneas de 20px (Geist Mono 12/20 de Señal).
        assert!(viewer.scroll_position().top < 2_000. * 20.);
    });
}

#[gpui_kit::test]
async fn a_new_diff_starts_at_the_top(cx: &mut TestAppContext) {
    let diff = large_diff(2_000);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        wheel(window, cx, 400., 300., 0., -5_000.)
    });
    draw(cx, window);
    cx.update(|cx| assert_eq!(viewer.read(cx).scroll_position().top, 5_000.));

    // El mismo diff no mueve nada...
    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.set_diff(diff, "big.js", cx)));
    draw(cx, window);
    cx.update(|cx| assert_eq!(viewer.read(cx).scroll_position().top, 5_000.));

    // ...otro vuelve al principio.
    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.set_diff(DIFF, "src/app.js", cx)));
    draw(cx, window);
    cx.update(|cx| {
        assert_eq!(viewer.read(cx).scroll_position(), ScrollPosition::default());
        assert_eq!(viewer.read(cx).painted_rows(), 0..4);
    });
}

#[gpui_kit::test]
async fn copies_the_selected_code_without_the_diff_signs(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);
    cx.update(|cx| assert_eq!(viewer.read(cx).selected_text(), None));

    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.select_all(cx)));
    draw(cx, window);
    let selected = cx.update(|cx| viewer.read(cx).selected_text());
    assert_eq!(
        selected.as_deref(),
        Some("@@ -1,3 +1,3 @@\n  const indentado = true\ncall(alpha)\ncall(gamma)")
    );

    // Con el foco en el visor, Ctrl/Cmd+C lo lleva al portapapeles.
    in_window(cx, window, |window, cx| {
        click(window, cx, 300., 400., 1);
        window.press("secondary-a", cx);
        window.press("secondary-c", cx);
    });
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        ClipboardItem::new_string(selected.unwrap_or_default()).text()
    );

    // Escape suelta la selección.
    in_window(cx, window, |window, cx| window.press("escape", cx));
    cx.update(|cx| assert_eq!(viewer.read(cx).selected_text(), None));
}

#[gpui_kit::test]
async fn selects_by_dragging_over_the_code(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    // De la fila de contexto (y≈29) a la última (y≈65), de borde a borde.
    in_window(cx, window, |window, cx| {
        window.drag(point(px(70.), px(29.)), point(px(880.), px(65.)), cx);
    });
    draw(cx, window);
    let selected = cx
        .update(|cx| viewer.read(cx).selected_text())
        .unwrap_or_default();
    assert!(
        selected.ends_with("call(alpha)\ncall(gamma)"),
        "{selected:?}"
    );
    assert!(selected.contains("indentado = true"), "{selected:?}");
    assert!(!selected.contains("@@"), "{selected:?}");

    // Un clic suelto no selecciona nada.
    in_window(cx, window, |window, cx| click(window, cx, 300., 400., 1));
    cx.update(|cx| assert_eq!(viewer.read(cx).selected_text(), None));
}

// En la vista partida la selección no salta del lado viejo al nuevo.
#[gpui_kit::test]
async fn keeps_a_split_selection_on_one_side(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Split, None);
    draw(cx, window);

    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.select_all(cx)));
    let selected = cx.update(|cx| viewer.read(cx).selected_text());
    assert_eq!(
        selected.as_deref(),
        Some("@@ -1,3 +1,3 @@\n  const indentado = true\ncall(gamma)")
    );
}

#[gpui_kit::test]
async fn copies_tabs_as_tabs(cx: &mut TestAppContext) {
    let diff = "diff --git a/Makefile b/Makefile\n--- a/Makefile\n+++ b/Makefile\n@@ -1,2 +1,2 @@\n build:\n-\tgo build\n+\tgo build ./...\n";
    let (window, viewer) = open_diff(cx, diff, "Makefile", DiffViewMode::Unified, None);
    draw(cx, window);

    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.select_all(cx)));
    let selected = cx
        .update(|cx| viewer.read(cx).selected_text())
        .unwrap_or_default();
    assert!(
        selected.ends_with("build:\n\tgo build\n\tgo build ./..."),
        "{selected:?}"
    );
}

// Los finales de línea de Windows no se pintan ni se copian.
#[gpui_kit::test]
async fn drops_the_carriage_return_of_windows_line_endings(cx: &mut TestAppContext) {
    let diff =
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\r\n-uno\r\n+dos\r\n";
    let (window, viewer) = open_diff(cx, diff, "a.txt", DiffViewMode::Unified, None);
    draw(cx, window);
    cx.update(|cx| {
        let contents: Vec<String> = viewer
            .read(cx)
            .lines()
            .map(|line| line.content.clone())
            .collect();
        assert_eq!(contents, ["@@ -1,1 +1,1 @@", "uno", "dos"]);
    });

    let (window, viewer, _events, _subscription) = open_file(cx);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file("a.txt", Some(FileContent::text("uno\r\ndos\r\n")), cx);
            viewer.select_all(cx);
        })
    });
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.line(0), Some("uno"));
        assert_eq!(viewer.selected_text().as_deref(), Some("uno\ndos"));
    });
}

// ---------------------------------------------------------------------------
// RepositoryDiffPanel
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PanelState {
    diff: String,
    file: Option<(String, String)>,
    loading: bool,
    too_large: Option<u64>,
    mode: DiffViewMode,
    ignore_whitespace: bool,
    hunk_action: Option<HunkAction>,
    cannot_discard: bool,
    whole_file: bool,
    image: Option<Entity<ImageDiffViewer>>,
    clicks: Vec<&'static str>,
}

fn open_panel(
    cx: &mut TestAppContext,
    state: PanelState,
) -> (
    AnyWindowHandle,
    (Entity<DiffViewer>, Rc<RefCell<PanelState>>),
) {
    open(cx, move |window, cx| {
        let viewer = cx.new(|cx| DiffViewer::new(window, cx));
        let state = Rc::new(RefCell::new(state));
        let content: HostContent = {
            let (viewer, state) = (viewer.clone(), state.clone());
            Box::new(move |_, _| {
                let current = state.borrow();
                let record = |name: &'static str| {
                    let state = state.clone();
                    move |_: &gpui_kit::ClickEvent, _: &mut Window, _: &mut App| {
                        state.borrow_mut().clicks.push(name)
                    }
                };
                DiffPanel::new(&viewer)
                    .file_opt(
                        current
                            .file
                            .clone()
                            .map(|(path, status)| (path.into(), status.into())),
                    )
                    .diff(current.diff.clone())
                    .loading(current.loading)
                    .too_large(current.too_large)
                    .view_mode(current.mode)
                    .ignore_whitespace(current.ignore_whitespace)
                    .hunk_action(current.hunk_action)
                    .whole_file(current.whole_file)
                    .image(current.image.clone())
                    .discardable(!current.cannot_discard)
                    .on_toggle_whole_file(record("toggle-whole-file"))
                    .on_toggle_view(record("toggle-view"))
                    .on_toggle_ignore_whitespace(record("toggle-whitespace"))
                    .on_discard(record("discard"))
                    .into_any_element()
            })
        };
        ((viewer, state), content)
    })
}

fn modified(path: &str) -> Option<(String, String)> {
    Some((path.to_string(), "M".to_string()))
}

#[gpui_kit::test]
async fn the_panel_shows_the_diff_of_the_selected_file(cx: &mut TestAppContext) {
    let (window, (viewer, state)) = open_panel(
        cx,
        PanelState {
            diff: TWO_HUNKS.to_string(),
            file: modified("src/app.js"),
            hunk_action: Some(HunkAction::Stage),
            ..Default::default()
        },
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);

    // El panel pone al día al visor: diff, ruta (y con ella el lenguaje) y acción.
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.diff().as_ref(), TWO_HUNKS);
        assert_eq!(viewer.file_path().as_ref(), "src/app.js");
        assert_eq!(viewer.language(), Some(Language::JavaScript));
        assert_eq!(viewer.hunk_action(), Some(HunkAction::Stage));
        assert_eq!(viewer.painted_rows(), 0..6);
    });

    in_window(cx, window, |window, cx| {
        assert_eq!(
            window.find("toggle-ignore-whitespace").label(),
            Some("Ignore spaces")
        );
        assert_eq!(window.find("toggle-diff-view").label(), Some("Split"));
        assert_eq!(window.find("discard-selected").label(), Some("Discard"));

        window.click("toggle-ignore-whitespace", cx);
        window.click("toggle-diff-view", cx);
        window.click("discard-selected", cx);
        // La cabecera del bloque, bajo la barra del panel.
        let button = window.find(("hunk-action", 0usize)).bounds().center();
        hover(window, cx, f32::from(button.x), f32::from(button.y));
        window.click(("hunk-action", 0usize), cx);
    });

    assert_eq!(
        state.borrow().clicks,
        ["toggle-whitespace", "toggle-view", "discard"]
    );
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        DiffViewerEvent::ApplyHunk(request) if request.index == 0 && request.patch.is_some()
    )));
}

#[gpui_kit::test]
async fn the_panel_follows_the_view_mode_and_the_whitespace_switch(cx: &mut TestAppContext) {
    let (window, (viewer, state)) = open_panel(
        cx,
        PanelState {
            diff: TWO_HUNKS.to_string(),
            file: modified("src/app.js"),
            hunk_action: Some(HunkAction::Stage),
            ..Default::default()
        },
    );
    draw(cx, window);

    {
        let mut state = state.borrow_mut();
        state.mode = DiffViewMode::Split;
        // Con "Ignore spaces" no se ofrece preparar bloques sueltos.
        state.ignore_whitespace = true;
        state.hunk_action = None;
    }
    draw(cx, window);

    in_window(cx, window, |window, _| {
        assert_eq!(window.find("toggle-diff-view").label(), Some("Unified"));
        assert!(window.try_find(("hunk-action", 0usize)).is_none());
    });
    cx.update(|cx| {
        assert_eq!(viewer.read(cx).view_mode(), DiffViewMode::Split);
        assert_eq!(viewer.read(cx).hunk_action(), None);
    });
}

#[gpui_kit::test]
async fn the_panel_does_not_discard_when_it_is_not_allowed(cx: &mut TestAppContext) {
    let (window, (_viewer, state)) = open_panel(
        cx,
        PanelState {
            diff: DIFF.to_string(),
            file: modified("src/app.js"),
            cannot_discard: true,
            ..Default::default()
        },
    );
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        window.click("discard-selected", cx);
        window.click("toggle-diff-view", cx);
    });
    // "Discard" no responde; el resto de la cabecera sí.
    assert_eq!(state.borrow().clicks, ["toggle-view"]);
}

#[gpui_kit::test]
async fn the_panel_announces_what_it_cannot_show(cx: &mut TestAppContext) {
    // Cargando, demasiado grande o sin cambios: el visor no se monta.
    for state in [
        PanelState {
            file: modified("src/app.js"),
            loading: true,
            diff: DIFF.to_string(),
            ..Default::default()
        },
        PanelState {
            file: modified("src/app.js"),
            too_large: Some(3_000_000),
            ..Default::default()
        },
        PanelState {
            file: modified("src/app.js"),
            ..Default::default()
        },
    ] {
        let (window, (viewer, _state)) = open_panel(cx, state);
        draw(cx, window);
        in_window(cx, window, |window, _| {
            assert!(window.try_find("toggle-diff-view").is_some());
        });
        cx.update(|cx| assert_eq!(viewer.read(cx).painted_rows(), 0..0));
    }

    // Sin archivo seleccionado no hay panel.
    let (window, _) = open_panel(cx, PanelState::default());
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("toggle-diff-view").is_none());
    });
}

#[test]
fn the_panel_keeps_the_texts_of_the_original() {
    assert_eq!(view_toggle_label(DiffViewMode::Unified), "Split");
    assert_eq!(view_toggle_label(DiffViewMode::Split), "Unified");
    assert_eq!(
        ignore_whitespace_tooltip(true),
        "Ignoring whitespace — click to bring it back"
    );
    assert!(ignore_whitespace_tooltip(false).starts_with("Hide whitespace-only changes"));
    for (status, label) in [
        ("M", "M"),
        ("A", "A"),
        ("D", "D"),
        ("R", "R"),
        ("C", "C"),
        ("?", "U"),
        ("??", "U"),
        ("U", "U"),
        ("T", "T"),
    ] {
        assert_eq!(panel_status_label(status), label);
    }
}

// ---------------------------------------------------------------------------
// FileContentViewer
// ---------------------------------------------------------------------------

fn open_file(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<FileContentViewer>,
    Rc<RefCell<Vec<FileContentViewerEvent>>>,
    Subscription,
) {
    let (window, viewer) = open(cx, |window, cx| {
        let viewer = cx.new(|cx| FileContentViewer::new(window, cx));
        let content = showing(&viewer);
        (viewer, content)
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let subscription = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&viewer, move |_, event: &FileContentViewerEvent, _| {
            events.borrow_mut().push(event.clone());
        })
    });
    (window, viewer, events, subscription)
}

#[gpui_kit::test]
async fn shows_the_lines_of_a_file(cx: &mut TestAppContext) {
    let (window, viewer, _events, _subscription) = open_file(cx);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file(
                "src/app.js",
                Some(FileContent::text("const a = 1\n\nmain()\n")),
                cx,
            )
        })
    });
    draw(cx, window);

    cx.update(|cx| {
        let viewer = viewer.read(cx);
        // El salto final no es una línea más.
        assert_eq!(viewer.line_count(), 3);
        assert_eq!(viewer.line(0), Some("const a = 1"));
        assert_eq!(viewer.line(1), Some(""));
        assert_eq!(viewer.line(2), Some("main()"));
        assert_eq!(viewer.language(), Some(Language::JavaScript));
        assert_eq!(viewer.status_text(), None);
        assert_eq!(viewer.painted_rows(), 0..3);
    });

    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.select_all(cx)));
    assert_eq!(
        cx.update(|cx| viewer.read(cx).selected_text()).as_deref(),
        Some("const a = 1\n\nmain()")
    );
}

// Un binario o un archivo enorme se anuncian; volcarlos no ayuda.
#[gpui_kit::test]
async fn announces_what_it_does_not_dump(cx: &mut TestAppContext) {
    let (window, viewer, _events, _subscription) = open_file(cx);
    let status = |cx: &mut TestAppContext, file: Option<FileContent>| {
        cx.update(|cx| {
            viewer.update(cx, |viewer, cx| {
                viewer.set_file("assets/logo.png", file, cx);
                (viewer.status_text(), viewer.line_count())
            })
        })
    };

    assert_eq!(
        status(cx, Some(FileContent::binary(2_148))),
        (Some("Binary file, 2.1 KB".into()), 0)
    );
    assert_eq!(
        status(cx, Some(FileContent::too_large(8_400_000))),
        (Some("Too large to show here, 8.0 MB".into()), 0)
    );
    assert_eq!(
        status(cx, Some(FileContent::text(""))),
        (Some("Empty file".into()), 0)
    );
    assert_eq!(status(cx, None), (Some("Empty file".into()), 0));
    draw(cx, window);

    // El error gana al contenido, y cargando no se dice nada (va el spinner).
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file("a.js", Some(FileContent::text("x\n")), cx);
            viewer.set_error("This file or commit is no longer available.", cx);
            assert_eq!(
                viewer.status_text(),
                Some("This file or commit is no longer available.".into())
            );
            viewer.set_loading(true, cx);
            assert_eq!(viewer.status_text(), None);
            assert!(viewer.is_loading());
            viewer.set_loading(false, cx);
            viewer.set_error("", cx);
            assert_eq!(viewer.status_text(), None);
        })
    });
    draw(cx, window);
    cx.update(|cx| assert_eq!(viewer.read(cx).painted_rows(), 0..1));
}

#[gpui_kit::test]
async fn the_header_closes_the_file(cx: &mut TestAppContext) {
    let (window, viewer, events, _subscription) = open_file(cx);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file("src/app.js", Some(FileContent::text("a\n")), cx)
        })
    });
    draw(cx, window);

    in_window(cx, window, |window, cx| window.click("close-file", cx));
    assert_eq!(*events.borrow(), [FileContentViewerEvent::Close]);

    // `hide-header`: sin cabecera no hay botón de cerrar.
    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.set_header_hidden(true, cx)));
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("close-file").is_none());
    });
}

#[gpui_kit::test]
async fn a_long_file_only_builds_what_is_visible(cx: &mut TestAppContext) {
    let (window, viewer, events, _subscription) = open_file(cx);
    let content: String = (0..20_000).map(|line| format!("line {line}\n")).collect();
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_header_hidden(true, cx);
            viewer.set_file("big.txt", Some(FileContent::text(content)), cx);
        })
    });
    draw(cx, window);
    cx.update(|cx| {
        assert_eq!(viewer.read(cx).line_count(), 20_000);
        assert!(viewer.read(cx).painted_rows().len() <= 40);
    });

    in_window(cx, window, |window, cx| {
        wheel(window, cx, 400., 300., 0., -36_000.)
    });
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        // 36.000px con líneas de 20px (Señal): la fila 1.800.
        assert_eq!(viewer.painted_rows().start, 1_800);
        assert_eq!(viewer.scroll_position().top, 36_000.);
    });
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        FileContentViewerEvent::Scrolled(position) if position.top == 36_000.
    )));
}

// ---------------------------------------------------------------------------
// BlameViewer
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn shows_who_touched_each_line(cx: &mut TestAppContext) {
    let (window, viewer) = open(cx, |window, cx| {
        let viewer = cx.new(|cx| BlameViewer::new(window, cx));
        let content = showing(&viewer);
        (viewer, content)
    });
    draw(cx, window);
    cx.update(|cx| {
        assert_eq!(
            viewer.read(cx).status_text(),
            Some("Loading blame...".into())
        );
    });

    let lines = vec![
        BlameLine::new(
            "e65ddb9",
            "Ana Torres",
            Some(1_788_257_220_000),
            "Initial commit",
            "import x",
        ),
        BlameLine::new("0000000", "Not Committed Yet", None, "", "\tmain()"),
    ];
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_state(BlameState::Lines(lines), cx)
        })
    });
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.status_text(), None);
        assert_eq!(viewer.line_count(), 2);
        assert_eq!(viewer.painted_rows(), 0..2);
    });

    for (state, text) in [
        (
            BlameState::Error("fatal: no such path".into()),
            "Error: fatal: no such path",
        ),
        (
            BlameState::TooLarge(8_400_000),
            "Too large to blame here, 8.0 MB",
        ),
    ] {
        cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.set_state(state, cx)));
        draw(cx, window);
        cx.update(|cx| {
            assert_eq!(viewer.read(cx).status_text(), Some(text.into()));
            assert_eq!(viewer.read(cx).line_count(), 0);
        });
    }
}

/// Blame navegable: cada bloque (líneas seguidas del mismo commit) es un
/// blanco sobre la columna de metadatos; el clic pide su commit y el menú,
/// el blame de la revisión anterior. Las líneas sin commit no navegan.
#[gpui_kit::test]
async fn a_blame_block_opens_its_commit_or_the_previous_revision(cx: &mut TestAppContext) {
    let (window, viewer) = open(cx, |window, cx| {
        let viewer = cx.new(|cx| BlameViewer::new(window, cx));
        let content = showing(&viewer);
        (viewer, content)
    });
    let events: Rc<RefCell<Vec<BlameViewerEvent>>> = Rc::default();
    let _subscription = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&viewer, move |_, event: &BlameViewerEvent, _| {
            events.borrow_mut().push(event.clone());
        })
    });
    let first = "e65ddb9e65ddb9e65ddb9e65ddb9e65ddb9e65dd";
    let second = "a1b2c3da1b2c3da1b2c3da1b2c3da1b2c3da1b2c";
    let parent = "f00dfacef00dfacef00dfacef00dfacef00dface";
    let lines = vec![
        BlameLine::new("e65ddb9", "Ana", Some(1_788_257_220_000), "Initial", "a").commit(first),
        BlameLine::new("e65ddb9", "Ana", Some(1_788_257_220_000), "Initial", "b").commit(first),
        BlameLine::new("a1b2c3d", "Leo", Some(1_788_257_220_000), "Tweak", "c")
            .commit(second)
            .previous(parent, "old/name.txt"),
        BlameLine::new("0000000", "Not Committed Yet", None, "", "d")
            .commit("0000000000000000000000000000000000000000"),
    ];
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_state(BlameState::Lines(lines), cx)
        })
    });
    draw(cx, window);

    cx.update(|cx| {
        assert_eq!(viewer.read(cx).blocks_in(0..4), [(0, 2), (2, 1)]);
    });
    in_window(cx, window, |window, _| {
        assert!(window.try_find("blame-commit:0").is_some());
        assert!(window.try_find("blame-commit:2").is_some());
        // Sin commit: sin blanco.
        assert!(window.try_find("blame-commit:3").is_none());
    });

    in_window(cx, window, |window, cx| window.click("blame-commit:2", cx));
    assert_eq!(
        events.borrow().last(),
        Some(&BlameViewerEvent::OpenCommit {
            hash: second.into()
        })
    );

    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.blame_previous_at(2, cx)));
    assert_eq!(
        events.borrow().last(),
        Some(&BlameViewerEvent::BlamePrevious {
            hash: parent.into(),
            path: "old/name.txt".into()
        })
    );
    // La primera versión de una línea no tiene revisión anterior; una línea
    // sin commit no lleva a ninguno.
    let before = events.borrow().len();
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.blame_previous_at(0, cx);
            viewer.open_commit_at(3, cx);
        })
    });
    assert_eq!(events.borrow().len(), before);
}

// ---------------------------------------------------------------------------
// FileTreeRow
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn a_file_row_reports_click_and_double_click(cx: &mut TestAppContext) {
    let (window, log) = open(cx, |_, _| {
        let log: Rc<RefCell<Vec<String>>> = Rc::default();
        let content: HostContent = {
            let log = log.clone();
            Box::new(move |_, _| {
                let record = |what: &'static str| {
                    let log = log.clone();
                    move |_: &gpui_kit::ClickEvent, _: &mut Window, _: &mut App| {
                        log.borrow_mut().push(what.to_string())
                    }
                };
                div()
                    .child(
                        FileTreeRow::folder("folder:src", "src").on_activate(record("toggle src")),
                    )
                    .child(
                        FileTreeRow::file("file:src/app.js", "app.js")
                            .depth(1)
                            .path("src/app.js")
                            .selected(true)
                            .leading(mergeon_ui::status_badge("M"))
                            .trailing(div().children(file_stats(9, 2)))
                            .on_activate(record("select app.js"))
                            .on_open_new(record("open app.js")),
                    )
                    .into_any_element()
            })
        };
        (log, content)
    });
    draw(cx, window);

    // Filas de 1.6rem: la carpeta ocupa 0–25.6 y el archivo 25.6–51.2.
    in_window(cx, window, |window, cx| click(window, cx, 120., 12., 1));
    assert_eq!(*log.borrow(), ["toggle src"]);

    log.borrow_mut().clear();
    in_window(cx, window, |window, cx| click(window, cx, 120., 38., 1));
    assert_eq!(*log.borrow(), ["select app.js"]);

    // El doble clic llega después de sus dos clics, como en el navegador.
    log.borrow_mut().clear();
    in_window(cx, window, |window, cx| click(window, cx, 120., 38., 2));
    assert_eq!(
        *log.borrow(),
        ["select app.js", "select app.js", "open app.js"]
    );
}

/// Una guía de 1px por nivel de sangría, bajo la flecha de la carpeta de ese
/// nivel; un archivo de la raíz no lleva ninguna.
#[gpui_kit::test]
async fn tree_rows_draw_one_indent_guide_per_level(cx: &mut TestAppContext) {
    let (window, _) = open(cx, |_, _| {
        let content: HostContent = Box::new(move |_, _| {
            use gpui_kit::{InteractiveElement as _, TestSupportExt as _};
            // Las filas sin guías se envuelven para poder buscar dentro.
            div()
                .child(
                    div()
                        .id("root-folder")
                        .test_support()
                        .child(FileTreeRow::folder("folder:src", "src")),
                )
                .child(FileTreeRow::folder("folder:src/ui", "ui").depth(1))
                .child(FileTreeRow::file("file:src/ui/app.js", "app.js").depth(2))
                .child(
                    FileTreeRow::file("file:src/ui/main.js", "main.js")
                        .depth(2)
                        .selected(true),
                )
                .child(
                    div()
                        .id("root-file")
                        .test_support()
                        .child(FileTreeRow::file("file:README.md", "README.md")),
                )
                .into_any_element()
        });
        ((), content)
    });
    draw(cx, window);

    in_window(cx, window, |window, _| {
        let guide = |window: &mut Window, row: &str, level: usize| {
            window
                .within(row.to_string())
                .try_find(("tree-guide", level))
                .map(|guide| guide.bounds())
        };
        assert!(guide(window, "root-folder", 0).is_none());
        assert!(guide(window, "root-file", 0).is_none());
        assert!(guide(window, "folder:src/ui", 0).is_some());
        assert!(guide(window, "folder:src/ui", 1).is_none());

        let first = guide(window, "file:src/ui/app.js", 0).expect("level 0");
        let second = guide(window, "file:src/ui/app.js", 1).expect("level 1");
        assert!(guide(window, "file:src/ui/app.js", 2).is_none());
        // 1px de ancho, todo el alto de la fila, un nivel (0.9rem) entre ellas.
        assert_eq!(first.size.width, px(1.));
        assert!(first.size.height >= px(26.), "{first:?}");
        assert_eq!(first.origin.y, second.origin.y);
        assert_eq!(first.size.height, second.size.height);
        // La fila de `app.js` es la tercera: las guías la cubren entera.
        assert_eq!(first.origin.y, first.size.height * 2.);
        assert!((f32::from(second.origin.x - first.origin.x) - 14.4).abs() < 0.5);
        // La seleccionada también las lleva (en su tinta).
        assert!(guide(window, "file:src/ui/main.js", 1).is_some());
    });
}

#[test]
fn file_stats_show_only_what_changed() {
    assert!(file_stats(0, 0).is_none());
    let stats = file_stats(12, 0).expect("hay altas");
    assert_eq!(stats.labels(), (Some("+12".to_string()), None));
    let stats = file_stats(3, 4).expect("hay cambios");
    // El menos es el tipográfico (U+2212), como en el original.
    assert_eq!(
        stats.labels(),
        (Some("+3".to_string()), Some("\u{2212}4".to_string()))
    );
}

#[test]
fn a_tree_row_takes_its_shape_from_the_model() {
    use mergeon_model::file_tree::build_file_tree_rows;

    let files = ["src/app.js".to_string(), "README.md".to_string()];
    let rows = build_file_tree_rows(&files, |path| path.as_str(), |_| true);
    let folders: Vec<bool> = rows
        .iter()
        .map(|row| FileTreeRow::from_row(row).is_folder())
        .collect();
    assert_eq!(folders, [true, false, false]);
}

/// `MIXED` seguido de tantas líneas de contexto que pasa del tamaño que el
/// visor analiza en el acto: se analiza en segundo plano.
fn big_mixed(extra: usize) -> String {
    let mut diff = MIXED.to_string();
    for line in 0..extra {
        diff.push_str(&format!(
            " relleno número {line} de un diff que no cabe en el acto\n"
        ));
    }
    diff
}

// Elegir un archivo con un diff enorme no congela la ventana: `set_diff`
// vuelve sin analizarlo, se sigue viendo el diff anterior y el nuevo aparece
// cuando el análisis (fuera del hilo de la interfaz) termina.
#[gpui_kit::test]
async fn a_huge_diff_is_parsed_off_the_ui_thread(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);
    assert!(cx.update(|cx| !viewer.read(cx).is_parsing()));

    let big = large_diff(20_000);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(big.clone(), "big.js", cx);
            assert!(viewer.is_parsing());
            assert_eq!(viewer.diff().as_ref(), big);
            assert!(!viewer.is_empty());
            // Aún no se analizó: lo que hay es el diff anterior, y sus líneas
            // no sirven para abrir el archivo nuevo en el editor.
            assert_eq!(viewer.lines().len(), 4);
            assert_eq!(viewer.editor_line(), None);
        })
    });
    // Un frame antes de que acabe sigue pintando el diff anterior.
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("ventana abierta");
    assert_eq!(cx.update(|cx| viewer.read(cx).painted_rows()), 0..4);

    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(!viewer.is_parsing());
        assert_eq!(viewer.lines().len(), 20_000);
        assert_eq!(viewer.painted_rows().start, 0);
        assert!(viewer.painted_rows().len() > 4);
    });

    // Un diff pequeño no pasa por segundo plano: no hay ni un frame con el
    // anterior.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(DIFF, "src/app.js", cx);
            assert!(!viewer.is_parsing());
            assert_eq!(viewer.lines().len(), 4);
        })
    });
}

// Un análisis que acaba tarde no pisa el diff que se pidió después.
#[gpui_kit::test]
async fn an_old_diff_does_not_replace_a_newer_one(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, DIFF, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);

    let first = large_diff(20_000);
    let second = big_mixed(3_000);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(first.clone(), "big.js", cx);
            viewer.set_diff(second.clone(), "f.txt", cx);
        })
    });
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(!viewer.is_parsing());
        assert_eq!(viewer.diff().as_ref(), second);
        assert_eq!(viewer.file_path().as_ref(), "f.txt");
        assert_eq!(viewer.lines().len(), 7 + 3_000);
        assert_eq!(
            viewer.lines().nth(1).map(|line| line.content.as_str()),
            Some("uno")
        );
    });

    // Tampoco pisa a uno pequeño (que se aplica en el acto)...
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(first.clone(), "big.js", cx);
            viewer.set_diff(TWO_HUNKS, "src/app.js", cx);
        })
    });
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.diff().as_ref(), TWO_HUNKS);
        assert_eq!(viewer.hunk_count(), 2);
        assert_eq!(viewer.lines().len(), 6);
    });

    // ...ni a volver al que ya se veía.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(first.clone(), "big.js", cx);
            viewer.set_diff(TWO_HUNKS, "src/app.js", cx);
            assert!(!viewer.is_parsing());
        })
    });
    draw(cx, window);
    cx.update(|cx| assert_eq!(viewer.read(cx).diff().as_ref(), TWO_HUNKS));
}

// Con el diff analizado en segundo plano, elegir líneas desde el medianil y
// prepararlas sigue igual; mientras otro se analiza, lo que se ve no admite
// acciones.
#[gpui_kit::test]
async fn picking_lines_works_on_a_diff_parsed_in_the_background(cx: &mut TestAppContext) {
    let diff = big_mixed(3_000);
    let (window, viewer) = open_diff(
        cx,
        &diff,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    draw(cx, window);
    assert!(cx.update(|cx| !viewer.read(cx).is_parsing()));

    in_window(cx, window, |window, cx| {
        assert!(window.try_find(("hunk-action", 0usize)).is_some());
        window.drag(
            point(px(GUTTER_X), px(row_y(1))),
            point(px(GUTTER_X), px(row_y(5))),
            cx,
        );
    });
    assert_eq!(selected_lines(cx, &viewer), [2, 3, 4]);
    draw(cx, window);
    in_window(cx, window, |window, cx| window.press("s", cx));
    let requests = lines_requests(&events);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].action, PatchAction::Stage);
    assert_eq!(requests[0].lines, [2, 3, 4]);
    assert_eq!(
        requests[0].patch,
        extract_lines_patch(&diff, &[2, 3, 4], PatchDirection::Forward)
    );

    // Otro diff grande en camino: ni botones de bloque ni líneas elegidas
    // sobre el que todavía se ve.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(big_mixed(3_001), "f.txt", cx);
            assert!(viewer.is_parsing());
            assert!(viewer.selected_lines().is_empty());
            viewer.select_lines(2..=3, cx);
            assert!(viewer.selected_lines().is_empty());
        })
    });
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("hunk-action", 0usize)).is_none());
    })
    .expect("ventana abierta");
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            assert!(!viewer.is_parsing());
            viewer.select_lines(2..=3, cx);
            assert_eq!(viewer.selected_lines(), [2, 3]);
        })
    });
}

// Desplazarse por lo ya visto (o un frame sin cambios) no vuelve a dar forma
// al texto: sale de la caché del lienzo.
#[gpui_kit::test]
async fn scrolling_back_does_not_shape_the_text_again(cx: &mut TestAppContext) {
    let diff = large_diff(2_000);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    let shaped = |cx: &mut TestAppContext| cx.update(|cx| viewer.read(cx).shaped_text_count());

    let first = shaped(cx);
    assert!(first > 0);
    draw(cx, window);
    assert_eq!(shaped(cx), first, "un frame sin cambios no da forma a nada");

    in_window(cx, window, |window, cx| {
        wheel(window, cx, 400., 300., 0., -3_000.)
    });
    draw(cx, window);
    let scrolled = shaped(cx);
    assert!(scrolled > first);

    in_window(cx, window, |window, cx| {
        wheel(window, cx, 400., 300., 0., 3_000.)
    });
    draw(cx, window);
    assert_eq!(cx.update(|cx| viewer.read(cx).painted_rows().start), 0);
    assert_eq!(shaped(cx), scrolled, "volver arriba sale de la caché");
}

// ---------------------------------------------------------------------------
// Buscar en el visor (Ctrl+F) y saltar entre bloques de cambios (F7)
// ---------------------------------------------------------------------------

/// Un diff con `blocks` bloques de cambios separados por mucho contexto.
fn spaced_changes_diff(blocks: usize, gap: usize) -> String {
    let lines = blocks * (gap + 1);
    let mut diff = String::from("diff --git a/big.js b/big.js\n--- a/big.js\n+++ b/big.js\n");
    diff.push_str(&format!("@@ -1,{lines} +1,{lines} @@\n"));
    for block in 0..blocks {
        for line in 0..gap {
            diff.push_str(&format!(" const context{block}_{line} = 1\n"));
        }
        diff.push_str(&format!("-needle({block}) old\n+needle({block}) new\n"));
    }
    diff
}

fn find_status(
    cx: &mut TestAppContext,
    viewer: &Entity<DiffViewer>,
) -> mergeon_view_diff::FindStatus {
    cx.update(|cx| viewer.read(cx).find_status(cx))
}

/// Clic en el código del visor (le da el foco) y Ctrl/Cmd+F.
fn focus_and_find(cx: &mut TestAppContext, window: AnyWindowHandle) {
    in_window(cx, window, |window, cx| {
        click(window, cx, 400., 200., 1);
        window.press("secondary-f", cx);
    });
}

#[gpui_kit::test]
async fn ctrl_f_in_the_diff_counts_and_walks_the_matches(cx: &mut TestAppContext) {
    let diff = spaced_changes_diff(3, 4);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    assert!(!find_status(cx, &viewer).open);

    focus_and_find(cx, window);
    in_window(cx, window, |window, cx| window.input("NEEDLE", cx));
    draw(cx, window);
    let status = find_status(cx, &viewer);
    assert!(status.open);
    assert_eq!(status.query, "NEEDLE");
    // Sin "Aa", da igual la caja: las bajas y las altas de los tres bloques.
    assert_eq!(status.matches.len(), 6);
    assert_eq!(status.current, Some(0));
    in_window(cx, window, |window, _| {
        assert_eq!(window.find("find-count").label(), Some("1 / 6"));
    });

    // Enter / Shift+Enter, con el foco en el campo.
    in_window(cx, window, |window, cx| {
        window.press("enter", cx);
        window.press("enter", cx);
    });
    assert_eq!(find_status(cx, &viewer).current, Some(2));
    in_window(cx, window, |window, cx| window.press("shift-enter", cx));
    assert_eq!(find_status(cx, &viewer).current, Some(1));
    // ↑ da la vuelta desde la primera.
    in_window(cx, window, |window, cx| {
        window.click("find-previous", cx);
        window.click("find-previous", cx);
    });
    assert_eq!(find_status(cx, &viewer).current, Some(5));
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert_eq!(window.find("find-count").label(), Some("6 / 6"));
    });

    // Esc cierra la barra y deja el foco en el visor: otro Esc no hace nada
    // y Ctrl+F la vuelve a abrir con la misma consulta.
    in_window(cx, window, |window, cx| window.press("escape", cx));
    draw(cx, window);
    assert!(!find_status(cx, &viewer).open);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("find-bar").is_none())
    });
    in_window(cx, window, |window, cx| window.press("secondary-f", cx));
    let status = find_status(cx, &viewer);
    assert!(status.open);
    assert_eq!(status.query, "NEEDLE");
}

#[gpui_kit::test]
async fn closing_find_during_a_background_search_clears_the_cache_for_reopen(
    cx: &mut TestAppContext,
) {
    // Más de `SYNC_SEARCH_ROWS` (20_000) para que la búsqueda corra en segundo plano.
    let diff = large_diff(20_005);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    cx.run_until_parked();
    draw(cx, window);

    focus_and_find(cx, window);
    // Cancelar la búsqueda antes de que termine en segundo plano.
    in_window(cx, window, |window, cx| {
        window.input("value19999", cx);
        window.press("escape", cx);
    });
    cx.run_until_parked();
    let closed = find_status(cx, &viewer);
    assert!(!closed.open);
    assert!(!closed.searching);
    assert!(closed.matches.is_empty());

    // Reabrir con la misma consulta debe relanzar la búsqueda y devolver la coincidencia.
    in_window(cx, window, |window, cx| window.press("secondary-f", cx));
    cx.run_until_parked();
    draw(cx, window);
    let reopened = find_status(cx, &viewer);
    assert!(reopened.open);
    assert!(!reopened.searching);
    assert_eq!(reopened.query, "value19999");
    assert_eq!(reopened.matches.len(), 1);
}

#[gpui_kit::test]
async fn the_current_match_is_scrolled_into_view(cx: &mut TestAppContext) {
    // 3 bloques con 300 líneas de contexto delante de cada uno: el tercero
    // queda muy por debajo de la ventana.
    let diff = spaced_changes_diff(3, 300);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    assert!(cx.update(|cx| viewer.read(cx).painted_rows().end) < 100);

    focus_and_find(cx, window);
    in_window(cx, window, |window, cx| window.input("needle(2) new", cx));
    draw(cx, window);
    let status = find_status(cx, &viewer);
    assert_eq!(status.matches.len(), 1);
    let (row, _, _) = status.matches[0].clone();
    assert!(row > 900);
    let painted = cx.update(|cx| viewer.read(cx).painted_rows());
    assert!(painted.contains(&row), "{painted:?} no contiene {row}");
}

#[gpui_kit::test]
async fn match_case_and_regex_options(cx: &mut TestAppContext) {
    let diff = spaced_changes_diff(2, 2);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    focus_and_find(cx, window);
    in_window(cx, window, |window, cx| window.input("Needle", cx));
    assert_eq!(find_status(cx, &viewer).matches.len(), 4);

    // Aa: "Needle" ya no casa con "needle".
    in_window(cx, window, |window, cx| window.click("find-case", cx));
    let status = find_status(cx, &viewer);
    assert!(status.case_sensitive);
    assert_eq!(status.matches.len(), 0);
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert_eq!(window.find("find-count").label(), Some("No results"));
    });
    in_window(cx, window, |window, cx| window.click("find-case", cx));

    // .*: el texto pasa a ser una expresión (Ctrl+F vuelve al campo con
    // todo seleccionado: lo que se escribe lo reemplaza).
    in_window(cx, window, |window, cx| {
        window.click("find-regex", cx);
        window.press("secondary-f", cx);
        window.input(r"needle\(1\) (old|new)", cx);
    });
    let status = find_status(cx, &viewer);
    assert!(status.regex && !status.invalid);
    assert_eq!(status.query, r"needle\(1\) (old|new)");
    assert_eq!(status.matches.len(), 2);

    // Una expresión que no compila lo dice y no cuenta nada.
    in_window(cx, window, |window, cx| {
        window.press("secondary-f", cx);
        window.input("needle(", cx);
    });
    let status = find_status(cx, &viewer);
    assert!(status.invalid);
    assert!(status.matches.is_empty());
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert_eq!(window.find("find-count").label(), Some("Invalid regex"));
    });
}

#[gpui_kit::test]
async fn finds_in_the_split_view_and_in_a_file(cx: &mut TestAppContext) {
    let diff = spaced_changes_diff(2, 2);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Split, None);
    draw(cx, window);
    focus_and_find(cx, window);
    in_window(cx, window, |window, cx| window.input("context0_1", cx));
    let status = find_status(cx, &viewer);
    // El contexto sale en los dos lados.
    assert_eq!(
        status
            .matches
            .iter()
            .map(|(_, column, _)| *column)
            .collect::<Vec<_>>(),
        [0, 1]
    );

    let (window, file, _events, _subscription) = open_file(cx);
    let content: String = (0..500).map(|line| format!("line {line}\n")).collect();
    cx.update(|cx| {
        file.update(cx, |viewer, cx| {
            viewer.set_file("notes.txt", Some(FileContent::text(content)), cx)
        })
    });
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        click(window, cx, 400., 200., 1);
        window.press("secondary-f", cx);
        window.input("line 450", cx);
    });
    draw(cx, window);
    let status = cx.update(|cx| file.read(cx).find_status(cx));
    assert_eq!(status.matches.len(), 1);
    assert_eq!(status.counter, "1 / 1");
    let painted = cx.update(|cx| file.read(cx).painted_rows());
    assert!(painted.contains(&450), "{painted:?}");
}

#[gpui_kit::test]
async fn f7_and_alt_arrows_walk_the_change_blocks(cx: &mut TestAppContext) {
    let diff = spaced_changes_diff(3, 300);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);
    let starts = cx.update(|cx| viewer.update(cx, |viewer, _| viewer.change_starts()));
    assert_eq!(starts.len(), 3);

    in_window(cx, window, |window, cx| {
        click(window, cx, 400., 200., 1);
        window.press("f7", cx);
    });
    draw(cx, window);
    let painted = |cx: &mut TestAppContext| cx.update(|cx| viewer.read(cx).painted_rows());
    assert!(painted(cx).contains(&starts[0]));
    in_window(cx, window, |window, cx| window.press("f7", cx));
    draw(cx, window);
    assert!(painted(cx).contains(&starts[1]));
    assert!(!painted(cx).contains(&starts[0]));
    in_window(cx, window, |window, cx| window.press("alt-down", cx));
    draw(cx, window);
    assert!(painted(cx).contains(&starts[2]));
    // Desde el último, vuelve al primero.
    in_window(cx, window, |window, cx| window.press("f7", cx));
    draw(cx, window);
    assert!(painted(cx).contains(&starts[0]));
    // Hacia atrás, también dando la vuelta.
    in_window(cx, window, |window, cx| window.press("shift-f7", cx));
    draw(cx, window);
    assert!(painted(cx).contains(&starts[2]));
    in_window(cx, window, |window, cx| window.press("alt-up", cx));
    draw(cx, window);
    assert!(painted(cx).contains(&starts[1]));
}

gpui_kit::actions!(viewer_tests, [GlobalSearch]);

/// Ctrl+F con el foco en el visor busca en el visor; fuera de él sigue
/// siendo el atajo global (en la app, "Search commits").
#[gpui_kit::test]
async fn ctrl_f_outside_the_viewer_keeps_the_global_shortcut(cx: &mut TestAppContext) {
    let global = Rc::new(std::cell::Cell::new(0));
    cx.update(|cx| {
        cx.bind_keys([gpui_kit::KeyBinding::new("secondary-f", GlobalSearch, None)]);
        let global = global.clone();
        cx.on_action(move |_: &GlobalSearch, _| global.set(global.get() + 1));
    });
    let diff = spaced_changes_diff(1, 2);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    draw(cx, window);

    // Sin foco en el visor: el global.
    in_window(cx, window, |window, cx| window.press("secondary-f", cx));
    assert_eq!(global.get(), 1);
    assert!(!find_status(cx, &viewer).open);

    // Con el foco en el visor: la barra, y el global no se entera.
    focus_and_find(cx, window);
    assert_eq!(global.get(), 1);
    assert!(find_status(cx, &viewer).open);
}

// ---------------------------------------------------------------------------
// "Whole file", marcas de cambios e imágenes
// ---------------------------------------------------------------------------

/// Un diff de 300 líneas de contexto con un cambio en la 20 y un alta en la
/// 250, como el de "Whole file".
fn long_diff() -> String {
    let mut diff = String::from(
        "diff --git a/f.txt b/f.txt\nindex 1111111..2222222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,300 +1,301 @@\n",
    );
    for line in 1..=300 {
        if line == 20 {
            diff.push_str("-old 20\n+new 20\n");
        } else {
            diff.push_str(&format!(" line {line}\n"));
        }
        if line == 250 {
            diff.push_str("+added after 250\n");
        }
    }
    diff
}

// Con el archivo entero el diff es un solo bloque: sin "Stage hunk" ni
// "Discard hunk" (serían el archivo entero), pero las líneas sueltas siguen.
#[gpui_kit::test]
async fn whole_file_drops_the_block_buttons_but_keeps_the_lines(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(
        cx,
        MIXED,
        "f.txt",
        DiffViewMode::Unified,
        Some(HunkAction::Stage),
    );
    let (events, _subscription) = record_events(cx, &viewer);
    cx.update(|cx| viewer.update(cx, |viewer, cx| viewer.set_hunk_buttons(false, cx)));
    draw(cx, window);

    in_window(cx, window, |window, cx| {
        assert!(window.try_find(("hunk-action", 0usize)).is_none());
        assert!(window.try_find(("hunk-discard", 0usize)).is_none());
        press_at(window, cx, GUTTER_X, row_y(2), Modifiers::default());
    });
    assert_eq!(selected_lines(cx, &viewer), [2]);
    draw(cx, window);
    in_window(cx, window, |window, cx| window.click("lines-stage", cx));
    // Por código tampoco se pide un bloque.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| viewer.apply_hunk(0, cx));
    });

    let events = events.borrow();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DiffViewerEvent::ApplyHunk(_)))
    );
    let requests: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            DiffViewerEvent::ApplyLines(request) => Some(request.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].lines, [2]);
}

// Una marca por bloque de cambios en la tira junto a la barra de
// desplazamiento, a su altura proporcional; un clic lleva al bloque.
#[gpui_kit::test]
async fn marks_the_changes_next_to_the_scrollbar_and_jumps_on_click(cx: &mut TestAppContext) {
    let diff = long_diff();
    let (window, viewer) = open_diff(cx, &diff, "f.txt", DiffViewMode::Unified, None);
    draw(cx, window);

    let markers = cx.update(|cx| viewer.update(cx, |viewer, _| viewer.change_markers()));
    let rows: Vec<usize> = markers.iter().map(|marker| marker.row).collect();
    // Fila 0 es la cabecera `@@`: la 20 del archivo es la fila 20.
    assert_eq!(rows, [20, 252]);
    assert!(markers[0].additions && markers[0].deletions);
    assert!(markers[1].additions && !markers[1].deletions);
    assert!(cx.update(|cx| viewer.read(cx).painted_rows()).start == 0);

    let strip = in_window(cx, window, |window, _| {
        window.find("change-markers").bounds()
    });
    assert!(strip.size.width > px(0.) && strip.size.height > px(100.));
    // La tira queda a la derecha del visor, fuera del texto.
    assert!(f32::from(strip.right()) >= WINDOW_WIDTH - 2.);

    let y = strip.top() + strip.size.height * markers[1].top + px(1.);
    in_window(cx, window, |window, cx| {
        click(window, cx, f32::from(strip.center().x), f32::from(y), 1);
    });
    draw(cx, window);
    let painted = cx.update(|cx| viewer.read(cx).painted_rows());
    assert!(painted.contains(&252), "{painted:?}");

    // La vista partida tiene sus propias marcas (bajas y altas en la misma fila).
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx)
        })
    });
    let split = cx.update(|cx| viewer.update(cx, |viewer, _| viewer.change_markers()));
    assert_eq!(split.len(), 2);
    assert!(split[0].row < split[1].row);
    draw(cx, window);
    assert!(in_window(cx, window, |window, _| window
        .try_find("change-markers")
        .is_some()));
}

#[gpui_kit::test]
async fn shows_no_marker_strip_without_changes(cx: &mut TestAppContext) {
    let rename =
        "diff --git a/a.txt b/b.txt\nsimilarity index 100%\nrename from a.txt\nrename to b.txt\n";
    let (window, viewer) = open_diff(cx, rename, "b.txt", DiffViewMode::Unified, None);
    draw(cx, window);
    assert!(cx.update(|cx| viewer.update(cx, |viewer, _| viewer.change_markers().is_empty())));
    assert!(!in_window(cx, window, |window, _| window
        .try_find("change-markers")
        .is_some()));
}

#[gpui_kit::test]
async fn the_panel_offers_whole_file_and_explains_the_single_block(cx: &mut TestAppContext) {
    let (window, (viewer, state)) = open_panel(
        cx,
        PanelState {
            diff: MIXED.to_string(),
            file: modified("f.txt"),
            hunk_action: Some(HunkAction::Stage),
            ..Default::default()
        },
    );
    draw(cx, window);
    in_window(cx, window, |window, cx| {
        assert_eq!(window.find("toggle-whole-file").label(), Some("Whole file"));
        assert!(window.try_find(("hunk-action", 0usize)).is_some());
        assert!(window.try_find("diff-note").is_none());
        window.click("toggle-whole-file", cx);
    });
    assert_eq!(state.borrow().clicks, ["toggle-whole-file"]);

    state.borrow_mut().whole_file = true;
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find(("hunk-action", 0usize)).is_none());
        assert!(window.try_find("diff-note").is_some());
    });
    cx.update(|cx| assert!(!viewer.read(cx).has_hunk_buttons()));
    assert_ne!(whole_file_tooltip(true), whole_file_tooltip(false));
}

/// Un PNG de 1×1 de verdad.
const PIXEL_PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn pixel() -> ImageVersion {
    ImageVersion::new(
        "logo.png",
        PIXEL_PNG.to_vec(),
        PIXEL_PNG.len() as u64,
        false,
    )
}

// Una imagen cambiada se ve antes y después, con píxeles y bytes; añadida,
// solo "después"; en el panel de cambios, en lugar del diff.
#[gpui_kit::test]
async fn shows_an_image_before_and_after(cx: &mut TestAppContext) {
    let (window, images) = open(cx, |_, cx| {
        let images = cx.new(ImageDiffViewer::new);
        let content = showing(&images);
        (images, content)
    });
    cx.update(|cx| images.update(cx, |images, cx| images.set_loading(cx)));
    draw(cx, window);
    assert!(in_window(cx, window, |window, _| window
        .try_find("image-loading")
        .is_some()));

    cx.update(|cx| {
        images.update(cx, |images, cx| {
            images.set_images(
                ImageDiff {
                    before: Some(pixel()),
                    after: Some(ImageVersion::new("logo.png", Vec::new(), 30 << 20, true)),
                },
                cx,
            )
        })
    });
    draw(cx, window);
    in_window(cx, window, |window, _| {
        let before = window.find("image-before");
        let after = window.find("image-after");
        assert!(
            before
                .label()
                .is_some_and(|label| label.starts_with("1 × 1 px"))
        );
        assert!(after.label().is_some_and(|label| label.contains("MB")));
        assert!(before.bounds().right() <= after.bounds().left());
    });

    cx.update(|cx| {
        images.update(cx, |images, cx| {
            images.set_images(
                ImageDiff {
                    before: None,
                    after: Some(pixel()),
                },
                cx,
            )
        })
    });
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("image-before").is_none());
        assert!(window.try_find("image-after").is_some());
    });

    cx.update(|cx| {
        images.update(cx, |images, cx| {
            images.set_error("Failed to read the image", cx)
        })
    });
    draw(cx, window);
    assert_eq!(
        in_window(cx, window, |window, _| window
            .find("image-error")
            .label()
            .map(str::to_string)),
        Some("Failed to read the image".to_string())
    );
}

#[gpui_kit::test]
async fn the_panel_shows_the_image_instead_of_the_diff(cx: &mut TestAppContext) {
    let images = cx.update(|cx| cx.new(ImageDiffViewer::new));
    cx.update(|cx| {
        images.update(cx, |images, cx| {
            images.set_images(
                ImageDiff {
                    before: Some(pixel()),
                    after: Some(pixel()),
                },
                cx,
            )
        })
    });
    let (window, _) = open_panel(
        cx,
        PanelState {
            diff: "diff --git a/logo.png b/logo.png\nindex 1..2 100644\nBinary files a/logo.png and b/logo.png differ\n".to_string(),
            file: modified("logo.png"),
            image: Some(images),
            ..Default::default()
        },
    );
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("image-before").is_some());
        assert!(window.try_find("image-after").is_some());
        assert!(window.try_find("diff-empty").is_none());
    });
}

const SVG_DIFF: &str = "diff --git a/img/logo.svg b/img/logo.svg\n\
index 1111111..2222222 100644\n\
--- a/img/logo.svg\n\
+++ b/img/logo.svg\n\
@@ -1,3 +1,3 @@\n <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"120\">\n\
-  <circle cx=\"60\" cy=\"60\" r=\"50\" fill=\"red\"/>\n\
+  <circle cx=\"60\" cy=\"60\" r=\"50\" fill=\"blue\"/>\n </svg>\n";

// Un SVG es imagen y texto: "Image | Text" cambia entre las dos versiones
// pintadas y el diff de su XML (con sus interruptores). Un PNG no lo lleva.
#[gpui_kit::test]
async fn an_svg_switches_between_the_image_and_its_text_diff(cx: &mut TestAppContext) {
    let images = cx.update(|cx| cx.new(ImageDiffViewer::new));
    cx.update(|cx| {
        images.update(cx, |images, cx| {
            images.set_images(
                ImageDiff {
                    before: Some(pixel()),
                    after: Some(pixel()),
                },
                cx,
            )
        })
    });
    let (window, (viewer, state)) = open_panel(
        cx,
        PanelState {
            diff: SVG_DIFF.to_string(),
            file: modified("img/logo.svg"),
            image: Some(images.clone()),
            ..Default::default()
        },
    );
    draw(cx, window);
    // Por defecto, la imagen.
    in_window(cx, window, |window, _| {
        assert!(window.try_find("image-before").is_some());
        assert_eq!(window.find("tab-svg-image").selected(), Some(true));
        assert_ne!(window.find("tab-svg-text").selected(), Some(true));
        assert!(window.try_find("toggle-whole-file").is_none());
    });
    assert!(cx.update(|cx| !images.read(cx).svg_as_text()));

    in_window(cx, window, |window, cx| window.click("tab-svg-text", cx));
    draw(cx, window);
    assert!(cx.update(|cx| images.read(cx).svg_as_text()));
    in_window(cx, window, |window, _| {
        assert!(window.try_find("image-before").is_none());
        assert_eq!(window.find("tab-svg-text").selected(), Some(true));
        assert!(window.try_find("toggle-whole-file").is_some());
        assert!(window.try_find("toggle-ignore-whitespace").is_some());
        assert!(window.try_find("toggle-diff-view").is_some());
    });
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.diff().as_ref(), SVG_DIFF);
        assert_eq!(viewer.painted_rows(), 0..5);
    });

    in_window(cx, window, |window, cx| window.click("tab-svg-image", cx));
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("image-before").is_some());
        assert!(window.try_find("toggle-whole-file").is_none());
    });

    // Un PNG no tiene texto: ni conmutador.
    state.borrow_mut().file = modified("img/logo.png");
    draw(cx, window);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("image-before").is_some());
        assert!(window.try_find("tab-svg-text").is_none());
    });
}

// Mientras GPUI decodifica una imagen (más de 200 ms: una foto de varios MB)
// el recuadro dice "Decoding image…" en lugar de quedarse vacío.
#[gpui_kit::test]
async fn says_it_is_decoding_an_image_that_takes_a_while(cx: &mut TestAppContext) {
    let (window, images) = open(cx, |_, cx| {
        let images = cx.new(ImageDiffViewer::new);
        let content = showing(&images);
        (images, content)
    });
    cx.update(|cx| {
        images.update(cx, |images, cx| {
            images.set_images(
                ImageDiff {
                    before: None,
                    after: Some(pixel()),
                },
                cx,
            )
        })
    });
    // Sin dejar correr las tareas en segundo plano, la decodificación no
    // acaba: el primer frame la empieza y apunta la hora.
    let frame = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("image-after-decoding").is_some()
        })
        .unwrap()
    };
    assert!(!frame(cx), "una imagen rápida no parpadea");
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(frame(cx), "pasado el retardo se dice que se decodifica");

    // Decodificada, la imagen y nada más.
    cx.run_until_parked();
    draw(cx, window);
    assert!(!in_window(cx, window, |window, _| window
        .try_find("image-after-decoding")
        .is_some()));
}

/// El mismo archivo de 600 líneas con un cambio cada 40: entero ("Whole
/// file") o por bloques con 3 líneas de contexto.
fn whole_and_hunks() -> (String, String) {
    let header =
        "diff --git a/f.txt b/f.txt\nindex 1111111..2222222 100644\n--- a/f.txt\n+++ b/f.txt\n";
    let changed = |line: u32| line.is_multiple_of(40) && line < 600;
    let mut whole = format!("{header}@@ -1,600 +1,600 @@\n");
    for line in 1..=600 {
        if changed(line) {
            whole.push_str(&format!("-old {line}\n+new {line}\n"));
        } else {
            whole.push_str(&format!(" line {line}\n"));
        }
    }
    let mut hunks = header.to_string();
    for change in (1..=600).filter(|line| changed(*line)) {
        hunks.push_str(&format!("@@ -{0},7 +{0},7 @@\n", change - 3));
        for line in change - 3..=change + 3 {
            if line == change {
                hunks.push_str(&format!("-old {line}\n+new {line}\n"));
            } else {
                hunks.push_str(&format!(" line {line}\n"));
            }
        }
    }
    (whole, hunks)
}

// "Whole file" cambia de diff: el visor vuelve a la línea que se leía, a la
// misma altura, no al mismo desplazamiento en píxeles; si la línea no está
// en los bloques, a la más cercana.
#[gpui_kit::test]
async fn keeps_the_line_in_view_when_switching_to_another_diff_of_the_file(
    cx: &mut TestAppContext,
) {
    let (whole, hunks) = whole_and_hunks();
    for mode in [DiffViewMode::Unified, DiffViewMode::Split] {
        let (window, viewer) = open_diff(cx, &hunks, "f.txt", mode, None);
        draw(cx, window);
        // Por bloques, hacia la mitad (bloque de la línea 320, más o menos).
        cx.update(|cx| {
            viewer.update(cx, |viewer, cx| {
                viewer.set_scroll_position(ScrollPosition::new(1_210., 0.), cx)
            })
        });
        draw(cx, window);
        let anchor = cx
            .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
            .expect("hay una línea arriba");
        assert!((200..440).contains(&anchor.line), "{anchor:?}");
        assert!(anchor.offset <= 0. && anchor.offset > -40., "{anchor:?}");

        cx.update(|cx| {
            viewer.update(cx, |viewer, cx| {
                viewer.set_diff(whole.clone(), "f.txt", cx);
                viewer.scroll_to_line(&anchor, cx);
            })
        });
        draw(cx, window);
        let again = cx
            .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
            .unwrap();
        assert_eq!(again.line, anchor.line, "{mode:?}");
        assert!(
            (again.offset - anchor.offset).abs() < 1.,
            "{again:?} {anchor:?}"
        );
        // En píxeles es otro sitio: el diff entero tiene todas las líneas.
        let top = cx.update(|cx| viewer.read(cx).scroll_position().top);
        assert!(top > 1_210. * 2., "{mode:?}: {top}");

        // Al revés: la línea 215 no está en los bloques; la más cercana es
        // la 203 (último contexto del bloque de la 200), no la 237.
        let line_215 = cx.update(|cx| {
            viewer.update(cx, |viewer, cx| {
                viewer.set_scroll_position(ScrollPosition::new(0., 0.), cx);
                let row = viewer
                    .lines()
                    .position(|line| line.new_num == Some(215))
                    .unwrap();
                ScrollPosition::new(row as f32 * 20., 0.)
            })
        });
        if mode == DiffViewMode::Unified {
            cx.update(|cx| {
                viewer.update(cx, |viewer, cx| viewer.set_scroll_position(line_215, cx))
            });
            draw(cx, window);
            let anchor = cx
                .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
                .unwrap();
            assert!((212..=218).contains(&anchor.line), "{anchor:?}");
            cx.update(|cx| {
                viewer.update(cx, |viewer, cx| {
                    viewer.set_diff(hunks.clone(), "f.txt", cx);
                    viewer.scroll_to_line(&anchor, cx);
                })
            });
            draw(cx, window);
            let nearest = cx
                .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
                .unwrap();
            assert_eq!(nearest.line, 203);
        }

        // Otro archivo no se toca.
        cx.update(|cx| {
            viewer.update(cx, |viewer, cx| {
                viewer.set_diff(hunks.replace("f.txt", "g.txt"), "g.txt", cx);
                viewer.scroll_to_line(&anchor, cx);
            })
        });
        draw(cx, window);
        assert_eq!(cx.update(|cx| viewer.read(cx).scroll_position().top), 0.);
    }
}

// Un diff grande se analiza en segundo plano: la línea se aplica cuando
// aparece, por encima del desplazamiento que se restaure mientras tanto.
#[gpui_kit::test]
async fn keeps_the_line_of_a_diff_parsed_in_the_background(cx: &mut TestAppContext) {
    let (whole, hunks) = whole_and_hunks();
    let (window, viewer) = open_diff(cx, &hunks, "f.txt", DiffViewMode::Unified, None);
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_scroll_position(ScrollPosition::new(1_210., 0.), cx)
        })
    });
    draw(cx, window);
    let anchor = cx
        .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
        .unwrap();
    // Relleno de contexto al final para pasar del tope del análisis síncrono.
    let mut big = whole.replacen("@@ -1,600 +1,600 @@", "@@ -1,20600 +1,20600 @@", 1);
    big.push_str(&" pad\n".repeat(20_000));
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_diff(big, "f.txt", cx);
            viewer.scroll_to_line(&anchor, cx);
            viewer.set_scroll_position(ScrollPosition::new(1_210., 0.), cx);
            assert!(viewer.is_parsing());
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    let again = cx
        .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
        .unwrap();
    assert_eq!(again.line, anchor.line);
}

// ---------------------------------------------------------------------------
// Filas de la vista partida, solo cuando se piden
// ---------------------------------------------------------------------------

/// Las marcas apuntan a filas con cambios de la vista en la que se ven.
fn assert_markers_point_at_changes(viewer: &mut DiffViewer) {
    let markers = viewer.change_markers();
    assert!(!markers.is_empty());
    match viewer.view_mode() {
        DiffViewMode::Unified => {
            let lines: Vec<DiffLineType> = viewer.lines().map(|line| line.kind).collect();
            for marker in markers.iter() {
                assert!(
                    matches!(lines[marker.row], DiffLineType::Add | DiffLineType::Del),
                    "{marker:?}"
                );
            }
        }
        DiffViewMode::Split => {
            let rows = viewer.split_rows();
            for marker in markers.iter() {
                let row = &rows[marker.row];
                assert!(
                    row.left.kind == SplitCellType::Del || row.right.kind == SplitCellType::Add,
                    "{marker:?}"
                );
            }
        }
    }
}

// En la vista unificada no se calculan las filas de la partida (casi la
// mitad del análisis): ni con un diff pequeño ni con uno grande. Al pasar a
// la partida, un diff pequeño las calcula en el acto.
#[gpui_kit::test]
async fn the_unified_view_does_not_build_split_rows(cx: &mut TestAppContext) {
    let (window, viewer) = open_diff(cx, TWO_HUNKS, "src/app.js", DiffViewMode::Unified, None);
    draw(cx, window);
    assert!(cx.update(|cx| !viewer.read(cx).has_split_rows()));

    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx);
            // En el acto: nada en segundo plano.
            assert!(!viewer.is_building_split_rows());
            assert!(viewer.has_split_rows());
            assert_eq!(viewer.view_mode(), DiffViewMode::Split);
            // Dos bloques de una baja y una alta: cabecera y fila partida.
            assert_eq!(viewer.row_count(), 4);
            assert_markers_point_at_changes(viewer);
        })
    });
    draw(cx, window);
    assert_eq!(cx.update(|cx| viewer.read(cx).painted_rows()), 0..4);

    // Un diff grande en la unificada tampoco las calcula.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Unified, cx);
            viewer.set_diff(spaced_changes_diff(400, 50), "big.js", cx);
            assert!(viewer.is_parsing());
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(!viewer.is_parsing());
        assert!(!viewer.has_split_rows());
        assert_eq!(viewer.lines().len(), 1 + 400 * 52);
    });

    // Un diff grande pedido ya en la partida sí las trae del análisis.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx);
            viewer.set_diff(spaced_changes_diff(400, 51), "big.js", cx);
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert!(!viewer.is_parsing());
            assert!(!viewer.is_building_split_rows());
            assert!(viewer.has_split_rows());
            assert_eq!(viewer.row_count(), 1 + 400 * 52);
        })
    });
}

// Con un diff grande, las filas de la partida se calculan fuera del hilo de
// la interfaz al pasar a ella. Mientras tanto se sigue viendo la unificada,
// con sus marcas y su búsqueda; al llegar, la búsqueda se repite sobre la
// partida y la línea pedida entretanto queda a la vista.
#[gpui_kit::test]
async fn a_huge_diff_builds_its_split_rows_off_the_ui_thread(cx: &mut TestAppContext) {
    let diff = spaced_changes_diff(400, 50);
    let (window, viewer) = open_diff(cx, &diff, "big.js", DiffViewMode::Unified, None);
    cx.run_until_parked();
    draw(cx, window);
    assert!(cx.update(|cx| !viewer.read(cx).has_split_rows()));

    focus_and_find(cx, window);
    in_window(cx, window, |window, cx| window.input("needle(123)", cx));
    draw(cx, window);
    let status = find_status(cx, &viewer);
    let unified: Vec<(usize, usize)> = status
        .matches
        .iter()
        .map(|(row, column, _)| (*row, *column))
        .collect();
    // La baja y la alta del bloque 123, una debajo de otra.
    let del_row = 1 + 123 * 52 + 50;
    assert_eq!(unified, [(del_row, 0), (del_row + 1, 0)]);

    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            // Media fila de contexto asomando por arriba (fila 2.550).
            viewer.set_scroll_position(ScrollPosition::new(51_010., 0.), cx)
        })
    });
    draw(cx, window);
    let anchor = cx
        .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
        .expect("hay una línea arriba");
    assert!(anchor.line > 2_000, "{anchor:?}");
    assert!(anchor.offset < 0., "{anchor:?}");

    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx);
            assert_eq!(viewer.view_mode(), DiffViewMode::Split);
            assert!(viewer.is_building_split_rows());
            assert!(!viewer.has_split_rows());
            // Lo que se ve sigue siendo la unificada, y todo lo que depende
            // de la vista es de ella.
            assert_eq!(viewer.row_count(), 1 + 400 * 52);
            assert_markers_point_at_changes_of_unified(viewer);
            viewer.scroll_to_line(&anchor, cx);
        })
    });
    // Un frame antes de que acabe: la unificada, sin las filas partidas.
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("ventana abierta");
    assert!(cx.update(|cx| viewer.read(cx).is_building_split_rows()));
    assert_eq!(
        find_status(cx, &viewer)
            .matches
            .iter()
            .map(|(row, column, _)| (*row, *column))
            .collect::<Vec<_>>(),
        unified
    );

    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert!(!viewer.is_building_split_rows());
            assert!(viewer.has_split_rows());
            // Cada baja y su alta, en la misma fila.
            assert_eq!(viewer.row_count(), 1 + 400 * 51);
            assert_markers_point_at_changes(viewer);
            let rows = viewer.split_rows();
            let row = &rows[1 + 123 * 51 + 50];
            assert_eq!(row.left.kind, SplitCellType::Del);
            assert_eq!(row.right.kind, SplitCellType::Add);
        })
    });
    let split: Vec<(usize, usize)> = find_status(cx, &viewer)
        .matches
        .iter()
        .map(|(row, column, _)| (*row, *column))
        .collect();
    let row = 1 + 123 * 51 + 50;
    assert_eq!(split, [(row, 0), (row, 1)]);
    let again = cx
        .update(|cx| viewer.update(cx, |viewer, _| viewer.line_anchor()))
        .expect("hay una línea arriba");
    assert_eq!(again.line, anchor.line);
    assert!(
        (again.offset - anchor.offset).abs() < 1.,
        "{again:?} {anchor:?}"
    );

    // De vuelta a la unificada: en el acto, con las filas ya calculadas.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Unified, cx);
            assert!(!viewer.is_building_split_rows());
            assert_eq!(viewer.row_count(), 1 + 400 * 52);
            viewer.set_view_mode(DiffViewMode::Split, cx);
            assert!(!viewer.is_building_split_rows());
            assert_eq!(viewer.row_count(), 1 + 400 * 51);
        })
    });
}

fn assert_markers_point_at_changes_of_unified(viewer: &mut DiffViewer) {
    let markers = viewer.change_markers();
    assert!(!markers.is_empty());
    let lines: Vec<DiffLineType> = viewer.lines().map(|line| line.kind).collect();
    for marker in markers.iter() {
        assert!(
            matches!(lines[marker.row], DiffLineType::Add | DiffLineType::Del),
            "{marker:?}"
        );
    }
}

// Cambiar de archivo (o volver a la unificada) mientras se calculan las
// filas partidas descarta ese cálculo: no pisa lo que se pidió después.
#[gpui_kit::test]
async fn switching_files_mid_split_build_discards_the_stale_rows(cx: &mut TestAppContext) {
    let first = spaced_changes_diff(400, 50);
    let (window, viewer) = open_diff(cx, &first, "big.js", DiffViewMode::Unified, None);
    cx.run_until_parked();
    draw(cx, window);

    // Otro diff grande: se analiza ya con sus filas partidas.
    let second = big_mixed(3_000);
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx);
            assert!(viewer.is_building_split_rows());
            viewer.set_diff(second.clone(), "f.txt", cx);
            assert!(!viewer.is_building_split_rows());
            assert!(viewer.is_parsing());
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    let expected_second = cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert!(!viewer.is_parsing());
            assert!(!viewer.is_building_split_rows());
            assert_eq!(viewer.diff().as_ref(), second);
            assert_eq!(viewer.file_path().as_ref(), "f.txt");
            assert_eq!(viewer.lines().len(), 7 + 3_000);
            let rows = viewer.split_rows();
            assert_markers_point_at_changes(viewer);
            rows.len()
        })
    });
    assert!(expected_second < 7 + 3_000);

    // A uno pequeño: se aplica en el acto, ya partido.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Unified, cx);
            viewer.set_diff(first.clone(), "big.js", cx);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx);
            assert!(viewer.is_building_split_rows());
            viewer.set_diff(TWO_HUNKS, "src/app.js", cx);
            assert!(!viewer.is_building_split_rows());
            assert!(viewer.has_split_rows());
            assert_eq!(viewer.row_count(), 4);
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert_eq!(viewer.diff().as_ref(), TWO_HUNKS);
            assert_eq!(viewer.row_count(), 4);
        })
    });

    // Partida y otra vez unificada antes de que acabe: nada que aplicar.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Unified, cx);
            viewer.set_diff(first.clone(), "big.js", cx);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx);
            viewer.set_view_mode(DiffViewMode::Unified, cx);
            assert!(!viewer.is_building_split_rows());
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert!(!viewer.has_split_rows());
            assert_eq!(viewer.view_mode(), DiffViewMode::Unified);
            assert_eq!(viewer.row_count(), 1 + 400 * 52);
        })
    });

    // Y una segunda petición sí llega, con las filas de este diff.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_view_mode(DiffViewMode::Split, cx)
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        viewer.update(cx, |viewer, _| {
            assert!(viewer.has_split_rows());
            assert_eq!(viewer.row_count(), 1 + 400 * 51);
        })
    });
}

// ---------------------------------------------------------------------------
// FileContentViewer: líneas de un archivo grande, fuera del hilo de la
// interfaz
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn a_small_file_is_indexed_at_once(cx: &mut TestAppContext) {
    let (_window, viewer, _events, _subscription) = open_file(cx);
    let content: String = (0..500).map(|line| format!("line {line}\r\n")).collect();
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file("notes.txt", Some(FileContent::text(content)), cx);
            assert!(!viewer.is_indexing());
            assert_eq!(viewer.line_count(), 500);
            assert_eq!(viewer.line(499), Some("line 499"));
        })
    });
}

#[gpui_kit::test]
async fn a_large_file_is_indexed_off_the_ui_thread(cx: &mut TestAppContext) {
    let (window, viewer, _events, _subscription) = open_file(cx);
    let content: String = (0..20_000).map(|line| format!("line {line}\r\n")).collect();
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_header_hidden(true, cx);
            viewer.set_file("big.txt", Some(FileContent::text(content)), cx);
            assert!(viewer.is_indexing());
            // Aún sin líneas, y sin decir "Empty file".
            assert_eq!(viewer.line_count(), 0);
            assert_eq!(viewer.status_text(), None);
            // Lo que se restaure mientras tanto se aplica al acabar.
            viewer.set_scroll_position(ScrollPosition::new(36_000., 0.), cx);
            assert_eq!(viewer.scroll_position().top, 36_000.);
        })
    });
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("file-indexing").is_some());
    })
    .expect("ventana abierta");

    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(!viewer.is_indexing());
        assert_eq!(viewer.line_count(), 20_000);
        assert_eq!(viewer.line(1_234), Some("line 1234"));
        assert_eq!(viewer.line(19_999), Some("line 19999"));
        assert_eq!(viewer.painted_rows().start, 1_800);
        assert_eq!(viewer.scroll_position().top, 36_000.);
    });

    // La búsqueda va sobre las líneas indexadas.
    in_window(cx, window, |window, cx| {
        click(window, cx, 400., 200., 1);
        window.press("secondary-f", cx);
        window.input("line 19998", cx);
    });
    draw(cx, window);
    let status = cx.update(|cx| viewer.read(cx).find_status(cx));
    assert_eq!(
        status
            .matches
            .iter()
            .map(|(row, _, _)| *row)
            .collect::<Vec<_>>(),
        [19_998]
    );
}

#[gpui_kit::test]
async fn an_old_file_index_does_not_replace_a_newer_one(cx: &mut TestAppContext) {
    let (window, viewer, _events, _subscription) = open_file(cx);
    let big =
        |name: &str| -> String { (0..20_000).map(|line| format!("{name} {line}\n")).collect() };
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file("a.txt", Some(FileContent::text(big("alpha"))), cx);
            viewer.set_file("b.txt", Some(FileContent::text(big("beta"))), cx);
            assert!(viewer.is_indexing());
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert!(!viewer.is_indexing());
        assert_eq!(viewer.path().as_ref(), "b.txt");
        assert_eq!(viewer.line_count(), 20_000);
        assert_eq!(viewer.line(7), Some("beta 7"));
    });

    // Tampoco pisa a uno pequeño, que se indexa en el acto.
    cx.update(|cx| {
        viewer.update(cx, |viewer, cx| {
            viewer.set_file("a.txt", Some(FileContent::text(big("alpha"))), cx);
            viewer.set_file("c.txt", Some(FileContent::text("uno\ndos\n")), cx);
            assert!(!viewer.is_indexing());
            assert_eq!(viewer.line_count(), 2);
        })
    });
    cx.run_until_parked();
    draw(cx, window);
    cx.update(|cx| {
        let viewer = viewer.read(cx);
        assert_eq!(viewer.path().as_ref(), "c.txt");
        assert_eq!(viewer.line_count(), 2);
        assert_eq!(viewer.line(1), Some("dos"));
    });
}
