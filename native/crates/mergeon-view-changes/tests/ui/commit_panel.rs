//! `CommitPanel` (`RepositoryCommitPanel.test.js`): la caja suelta, con sus
//! props puestas a mano y lo que avisa registrado.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window,
};
use mergeon_ui::{InputState, TextareaState};
use mergeon_view_changes::{CommitPanel, compose_commit_message, split_commit_message};

use crate::support::TestApp;

#[derive(Clone)]
struct Props {
    push_after_commit: bool,
    can_commit: bool,
    staged_count: usize,
    branch: &'static str,
    upstream: &'static str,
    amend: bool,
    can_amend: bool,
    amend_is_pushed: bool,
    sign: bool,
    shortcut_hints: bool,
}

impl Default for Props {
    /// Las de `renderPanel` del original.
    fn default() -> Self {
        Self {
            push_after_commit: false,
            can_commit: true,
            staged_count: 2,
            branch: "main",
            upstream: "origin/main",
            amend: false,
            can_amend: false,
            amend_is_pushed: false,
            sign: false,
            shortcut_hints: true,
        }
    }
}

struct PanelHost {
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    props: Props,
    log: Rc<RefCell<Vec<String>>>,
}

impl Render for PanelHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let props = self.props.clone();
        let record = |what: &'static str| {
            let log = self.log.clone();
            move |value: &bool, _: &mut Window, _: &mut gpui_kit::App| {
                log.borrow_mut().push(format!("{what}:{value}"));
            }
        };
        let log = self.log.clone();
        CommitPanel::new(&self.title, &self.body)
            .shortcut_hints(props.shortcut_hints)
            .sign(props.sign)
            .on_sign_change(record("sign"))
            .push_after_commit(props.push_after_commit)
            .committable(props.can_commit)
            .staged_count(props.staged_count)
            .branch(props.branch)
            .upstream(props.upstream)
            .amend(props.amend)
            .amendable(props.can_amend)
            .amend_is_pushed(props.amend_is_pushed)
            .on_push_after_commit_change(record("push"))
            .on_amend_change(record("amend"))
            .on_dismiss_hints({
                let log = self.log.clone();
                move |_, _| log.borrow_mut().push("dismiss-hints".to_string())
            })
            .on_commit(move |_, _| log.borrow_mut().push("commit".to_string()))
    }
}

struct Panel {
    app: TestApp,
    host: Entity<PanelHost>,
    log: Rc<RefCell<Vec<String>>>,
}

impl Panel {
    fn new(cx: &mut TestAppContext, message: &str, props: Props) -> Self {
        let app = TestApp::new(cx);
        let log: Rc<RefCell<Vec<String>>> = Rc::default();
        let message = message.to_string();
        let host = cx
            .update_window(app.window, {
                let log = log.clone();
                move |_, window, cx| {
                    let (title, body) = split_commit_message(&message);
                    let title = cx.new(|cx| {
                        let mut state = InputState::new(window, cx).placeholder("Summary");
                        state.set_value(title, window, cx);
                        state
                    });
                    let body = cx.new(|cx| {
                        let mut state = TextareaState::new(window, cx).placeholder("Description");
                        state.set_value(body, window, cx);
                        state
                    });
                    cx.new(|_| PanelHost {
                        title,
                        body,
                        props,
                        log,
                    })
                }
            })
            .expect("la ventana de prueba se cerró");
        let shown = host.clone();
        app.show(cx, move |_, _| shown.clone().into_any_element());
        Self { app, host, log }
    }

    fn update(&self, cx: &mut TestAppContext, change: impl FnOnce(&mut Props)) {
        self.host.update(cx, |host, cx| {
            change(&mut host.props);
            cx.notify();
        });
        cx.run_until_parked();
    }

    /// El mensaje unido, como lo usará el commit.
    fn message(&self, cx: &mut TestAppContext) -> String {
        self.host.read_with(cx, |host, cx| {
            compose_commit_message(&host.title.read(cx).value(), &host.body.read(cx).value())
        })
    }

    fn parts(&self, cx: &mut TestAppContext) -> (String, String) {
        self.host.read_with(cx, |host, cx| {
            (
                host.title.read(cx).value().to_string(),
                host.body.read(cx).value().to_string(),
            )
        })
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

#[gpui_kit::test]
async fn disables_commit_when_can_commit_is_false(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "test commit",
        Props {
            can_commit: false,
            ..Default::default()
        },
    );

    panel.app.click(cx, "commit");
    assert!(panel.log().is_empty());
}

#[gpui_kit::test]
async fn emits_message_checkbox_and_commit_actions(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "", Props::default());

    panel.app.click(cx, "commit-title");
    panel
        .app
        .in_window(cx, |window, cx| window.input("feat: update", cx));
    panel.app.click(cx, "push-after-commit");
    panel.app.click(cx, "commit");

    assert_eq!(panel.message(cx), "feat: update");
    assert_eq!(panel.log(), ["push:true", "commit"]);
}

// ---------------------------------------------------------------------------
// Dónde cae y qué falta
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn says_where_the_commit_is_going(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "arregla el parser", Props::default());

    // La rama va en una línea discreta encima; el botón dice cuántos archivos.
    assert_eq!(
        panel.app.label(cx, "commit-target").as_deref(),
        Some("Committing to main")
    );
    assert_eq!(
        panel.app.label(cx, "commit-action").as_deref(),
        Some("Commit 2 files")
    );
    assert_eq!(
        panel.app.label(cx, "commit-staged-count").as_deref(),
        Some("2 staged")
    );
    let (target, title) = panel.app.in_window(cx, |window, _| {
        (
            window.find("commit-target").bounds(),
            window.find("commit-title").bounds(),
        )
    });
    assert!(target.bottom() <= title.top());

    panel.update(cx, |props| props.push_after_commit = true);
    assert_eq!(
        panel.app.label(cx, "commit-action").as_deref(),
        Some("Commit & Push 2 files")
    );

    // Con HEAD separado no hay rama que nombrar.
    panel.update(cx, |props| props.branch = "");
    assert_eq!(
        panel.app.label(cx, "commit-target").as_deref(),
        Some("Committing on a detached HEAD")
    );
}

/// A 400px (el ancho por defecto de la columna) todo cabe sin salirse: las
/// opciones, "Fixup" y el botón de commit.
#[gpui_kit::test]
async fn fits_a_400px_column(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            can_amend: true,
            ..Default::default()
        },
    );
    panel.app.show(cx, {
        let host = panel.host.clone();
        move |_, _| {
            gpui_kit::div()
                .w(gpui_kit::px(400.))
                .child(host.clone())
                .into_any_element()
        }
    });
    let bounds = panel.app.in_window(cx, |window, _| {
        [
            "commit-panel",
            "commit-action",
            "commit-fixup-action",
            "amend-last-commit",
            "push-after-commit",
            "sign-commit",
        ]
        .map(|id| window.find(id).bounds())
    });
    let panel_bounds = bounds[0];
    assert_eq!(panel_bounds.size.width, gpui_kit::px(400.));
    for (index, element) in bounds.iter().enumerate().skip(1) {
        assert!(
            element.right() <= panel_bounds.right() && element.left() >= panel_bounds.left(),
            "element {index} {element:?} overflows {panel_bounds:?}"
        );
    }
    // Fixup y commit en la misma fila, el commit a la derecha.
    assert_eq!(bounds[1].top(), bounds[2].top());
    assert!(bounds[2].right() <= bounds[1].left());
}

// ---------------------------------------------------------------------------
// Asunto y cuerpo
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn counts_the_subject_against_72_characters(cx: &mut TestAppContext) {
    let subject = "feat(checkout): ETA y error recuperable en envío";
    let panel = Panel::new(cx, subject, Props::default());
    assert_eq!(
        panel.app.label(cx, "commit-title-counter").as_deref(),
        Some("48/72")
    );
    // El color va por tramos (`subject_tone`): neutro, ámbar y rojo.
    assert_eq!(
        mergeon_view_changes::subject_tone(48),
        mergeon_view_changes::SubjectTone::Neutral
    );
    assert_eq!(
        mergeon_view_changes::subject_tone(60),
        mergeon_view_changes::SubjectTone::Warning
    );

    panel.app.click(cx, "commit-title");
    panel.app.in_window(cx, |window, cx| {
        window.input(&"x".repeat(25), cx);
    });
    assert_eq!(
        panel.app.label(cx, "commit-title-counter").as_deref(),
        Some("73/72")
    );
}

#[gpui_kit::test]
async fn enter_in_the_subject_moves_on_to_the_body(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "", Props::default());

    panel.app.click(cx, "commit-title");
    panel.app.in_window(cx, |window, cx| {
        window.input("asunto", cx);
        window.press("enter", cx);
        window.input("cuerpo", cx);
    });

    assert_eq!(
        panel.parts(cx),
        ("asunto".to_string(), "cuerpo".to_string())
    );
    assert_eq!(panel.message(cx), "asunto\n\ncuerpo");
    assert!(panel.log().is_empty());
}

#[gpui_kit::test]
async fn offers_to_sign_the_commit(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "arregla el parser", Props::default());
    assert_eq!(panel.app.label(cx, "sign-commit").as_deref(), Some("Sign"));

    panel.app.click(cx, "sign-commit");
    panel.update(cx, |props| props.sign = true);
    panel.app.click(cx, "sign-commit");

    assert_eq!(panel.log(), ["sign:true", "sign:false"]);
}

#[gpui_kit::test]
async fn shows_the_list_keys_above_the_box_when_asked(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "", Props::default());
    assert_eq!(
        panel.app.label(cx, "changes-shortcut-hints").as_deref(),
        Some("Space stage / unstage · D discard · ↑ ↓ move")
    );
    let (hints, title) = panel.app.in_window(cx, |window, _| {
        (
            window.find("changes-shortcut-hints").bounds(),
            window.find("commit-title").bounds(),
        )
    });
    assert!(hints.bottom() <= title.top());

    panel.update(cx, |props| props.shortcut_hints = false);
    assert!(!panel.app.exists(cx, "changes-shortcut-hints"));
}

/// La `×` de los atajos avisa a quien aloja la caja (que lo recuerda).
#[gpui_kit::test]
async fn the_list_keys_can_be_dismissed(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "", Props::default());
    assert!(panel.app.exists(cx, "changes-shortcut-hints"));
    panel.app.click(cx, "changes-shortcut-hints-close");
    assert_eq!(panel.log(), ["dismiss-hints"]);
}

#[gpui_kit::test]
async fn says_what_is_missing_when_nothing_is_staged(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            can_commit: false,
            staged_count: 0,
            ..Default::default()
        },
    );

    assert_eq!(
        panel.app.label(cx, "commit-blocked").as_deref(),
        Some("Stage something to commit")
    );
}

#[gpui_kit::test]
async fn says_what_is_missing_when_there_is_no_message(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "   ",
        Props {
            can_commit: false,
            ..Default::default()
        },
    );

    assert_eq!(
        panel.app.label(cx, "commit-blocked").as_deref(),
        Some("Write a message to commit")
    );
}

#[gpui_kit::test]
async fn says_nothing_when_everything_is_in_place(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "arregla el parser", Props::default());

    assert!(!panel.app.exists(cx, "commit-blocked"));
}

/// El atajo es `Ctrl+Enter` (o `Cmd+Enter`) y no mete un salto de línea: el
/// área de texto de gpui-kit lo usa para eso.
#[gpui_kit::test]
async fn commits_with_the_keyboard(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "arregla el parser", Props::default());

    panel.app.click(cx, "commit-title");
    panel
        .app
        .in_window(cx, |window, cx| window.press("ctrl-enter", cx));

    assert_eq!(panel.log(), ["commit"]);
    assert_eq!(panel.message(cx), "arregla el parser");

    // También desde el cuerpo, sin partir la línea.
    panel.app.click(cx, "commit-body");
    panel
        .app
        .in_window(cx, |window, cx| window.press("cmd-enter", cx));
    assert_eq!(panel.log(), ["commit", "commit"]);
    assert_eq!(panel.message(cx), "arregla el parser");
}

#[gpui_kit::test]
async fn does_not_commit_with_the_keyboard_when_it_could_not_with_the_mouse(
    cx: &mut TestAppContext,
) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            can_commit: false,
            staged_count: 0,
            ..Default::default()
        },
    );

    panel.app.click(cx, "commit-title");
    panel
        .app
        .in_window(cx, |window, cx| window.press("cmd-enter", cx));
    panel
        .app
        .in_window(cx, |window, cx| window.press("ctrl-enter", cx));

    assert!(panel.log().is_empty());
    assert_eq!(panel.message(cx), "arregla el parser");
}

/// En el cuerpo, un Enter a secas sigue partiendo la línea.
#[gpui_kit::test]
async fn a_plain_enter_still_breaks_the_line_in_the_body(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "asunto", Props::default());

    panel.app.click(cx, "commit-body");
    panel.app.in_window(cx, |window, cx| {
        window.input("uno", cx);
        window.press("enter", cx);
        window.input("dos", cx);
    });

    assert_eq!(
        panel.parts(cx),
        ("asunto".to_string(), "uno\ndos".to_string())
    );
    assert!(panel.log().is_empty());
}

// ---------------------------------------------------------------------------
// Amend
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn offers_amend_only_when_there_is_a_commit_to_amend(cx: &mut TestAppContext) {
    let panel = Panel::new(cx, "arregla el parser", Props::default());
    assert!(!panel.app.exists(cx, "amend-last-commit"));

    panel.update(cx, |props| props.can_amend = true);

    assert_eq!(
        panel.app.label(cx, "amend-last-commit").as_deref(),
        Some("Amend")
    );
    panel.app.click(cx, "amend-last-commit");
    assert_eq!(panel.log(), ["amend:true"]);
}

#[gpui_kit::test]
async fn says_what_the_button_will_do(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            can_amend: true,
            ..Default::default()
        },
    );
    assert_eq!(
        panel.app.label(cx, "commit-action").as_deref(),
        Some("Commit 2 files")
    );

    panel.update(cx, |props| props.amend = true);
    assert_eq!(
        panel.app.label(cx, "commit-action").as_deref(),
        Some("Amend with 2 files")
    );
    assert_eq!(
        panel.app.label(cx, "commit-target").as_deref(),
        Some("Amending the last commit on main")
    );

    panel.update(cx, |props| {
        props.push_after_commit = true;
        props.staged_count = 0;
    });
    assert_eq!(
        panel.app.label(cx, "commit-action").as_deref(),
        Some("Amend & Push")
    );
}

#[gpui_kit::test]
async fn does_not_ask_to_stage_anything_when_amending(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            amend: true,
            can_amend: true,
            staged_count: 0,
            ..Default::default()
        },
    );

    assert!(!panel.app.exists(cx, "commit-blocked"));
}

#[gpui_kit::test]
async fn warns_before_rewriting_a_commit_that_is_already_pushed(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            amend: true,
            can_amend: true,
            amend_is_pushed: true,
            ..Default::default()
        },
    );
    assert!(
        panel
            .app
            .label(cx, "commit-warning")
            .unwrap_or_default()
            .contains("already on origin/main")
    );

    panel.update(cx, |props| props.amend_is_pushed = false);

    assert!(!panel.app.exists(cx, "commit-warning"));
}

#[gpui_kit::test]
async fn keeps_quiet_about_the_rewrite_while_amend_is_off(cx: &mut TestAppContext) {
    let panel = Panel::new(
        cx,
        "arregla el parser",
        Props {
            can_amend: true,
            amend_is_pushed: true,
            ..Default::default()
        },
    );

    assert!(!panel.app.exists(cx, "commit-warning"));
}
