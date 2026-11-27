//! Tests de interacción de los componentes de `mergeon_ui`: ventanas sin
//! pantalla con la misma raíz que la app (`gpui_kit::open_window`), clics y
//! teclas reales, y comprobación de lo que recibe quien usa el componente.
//!
//! Portan la intención de `GlobalDialogs.test.js`, `src/lib/ui/dialogs.test.js`
//! y `src/lib/ui/notify.test.js`; la lógica pura de esos archivos (recorte de
//! mensajes, avisos repetidos, receta de estado de archivo) se prueba junto a
//! su módulo.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, TestAppContext, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use mergeon_ui::dialog::{
    self, AppDialog, ConfirmOption, ConfirmOptions, ConfirmOutcome, ConfirmVariant, PromptOptions,
};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::toast::{self, ToastKind, ToastOptions, ToastProgress};
use mergeon_ui::{RadioOption, TabItem};

/// Vista de prueba: dibuja lo que le pida el test en cada frame.
struct Host {
    content: HostContent,
}

type HostContent = Box<dyn Fn(&mut Window, &mut Context<Host>) -> AnyElement>;

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().child((self.content)(window, cx))
    }
}

/// Abre una ventana como la de la app: `mergeon_ui::init` + `open_window`.
fn open(
    cx: &mut TestAppContext,
    content: impl Fn(&mut Window, &mut Context<Host>) -> AnyElement + 'static,
) -> AnyWindowHandle {
    cx.update(|cx| {
        mergeon_ui::init(cx);
        // Las animaciones de entrada usan el reloj real; sin movimiento
        // quedan resueltas en el primer frame.
        cx.set_reduce_motion(true);
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(900.), px(700.)),
                })),
                ..Default::default()
            },
            cx,
            |_, cx| {
                cx.new(|_| Host {
                    content: Box::new(content),
                })
            },
        )
        .expect("no se pudo abrir la ventana de prueba");
        window
    })
}

fn empty(_: &mut Window, _: &mut Context<Host>) -> AnyElement {
    div().into_any_element()
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

// ---------------------------------------------------------------------------
// confirm (dialogs.test.js)
// ---------------------------------------------------------------------------

fn confirm_options(title: &str) -> ConfirmOptions {
    ConfirmOptions {
        title: SharedString::from(title.to_string()),
        ..Default::default()
    }
}

#[gpui_kit::test]
async fn confirm_answers_true_when_accepted_and_closes(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = Rc::new(Cell::new(None));

    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            dialog::confirm(
                window,
                cx,
                confirm_options("Delete tag"),
                move |ok, _, _| answer.set(Some(ok)),
            );
        }
    });
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_titles(window, cx), ["Delete tag"]);
        window.click("confirm-accept", cx);
    });

    assert_eq!(answer.get(), Some(true));
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

#[gpui_kit::test]
async fn confirm_answers_false_when_cancelled(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = Rc::new(Cell::new(None));

    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            dialog::confirm(
                window,
                cx,
                confirm_options("Delete tag"),
                move |ok, _, _| answer.set(Some(ok)),
            );
        }
    });
    in_window(cx, window, |window, cx| window.click("confirm-cancel", cx));

    assert_eq!(answer.get(), Some(false));
}

/// Escape y la X descartan: responden `dismiss_value`, una sola vez.
#[gpui_kit::test]
async fn dismissing_a_confirm_answers_its_dismiss_value(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answers = Rc::new(RefCell::new(Vec::new()));

    for dismiss_value in [false, true] {
        in_window(cx, window, {
            let answers = answers.clone();
            move |window, cx| {
                dialog::confirm(
                    window,
                    cx,
                    ConfirmOptions {
                        dismiss_value,
                        ..confirm_options("Discard changes")
                    },
                    move |ok, _, _| answers.borrow_mut().push(ok),
                );
            }
        });
        in_window(cx, window, |window, cx| window.press("escape", cx));
        in_window(cx, window, |window, cx| {
            assert_eq!(dialog::open_count(window, cx), 0);
        });
    }

    assert_eq!(*answers.borrow(), [false, true]);
}

#[gpui_kit::test]
async fn enter_confirms(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = Rc::new(Cell::new(None));

    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            dialog::confirm(
                window,
                cx,
                ConfirmOptions {
                    variant: ConfirmVariant::Destructive,
                    ..confirm_options("Delete tag")
                },
                move |ok, _, _| answer.set(Some(ok)),
            );
        }
    });
    in_window(cx, window, |window, cx| window.press("enter", cx));

    assert_eq!(answer.get(), Some(true));
}

fn confirm_delete_branch(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
) -> Rc<Cell<Option<ConfirmOutcome>>> {
    let outcome = Rc::new(Cell::new(None));
    in_window(cx, window, {
        let outcome = outcome.clone();
        move |window, cx| {
            dialog::confirm_with_option(
                window,
                cx,
                confirm_options("Delete branch"),
                ConfirmOption {
                    label: "Also on origin".into(),
                    checked: false,
                },
                move |answer, _, _| outcome.set(Some(answer)),
            );
        }
    });
    outcome
}

/// Lo irreversible para los demás no debería venir marcado de fábrica.
#[gpui_kit::test]
async fn confirm_with_option_starts_with_the_option_off(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let outcome = confirm_delete_branch(cx, window);

    in_window(cx, window, |window, cx| window.click("confirm-accept", cx));

    assert_eq!(
        outcome.get(),
        Some(ConfirmOutcome {
            confirmed: true,
            checked: false
        })
    );
}

#[gpui_kit::test]
async fn confirm_with_option_carries_the_option_back_with_the_answer(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let outcome = confirm_delete_branch(cx, window);

    in_window(cx, window, |window, cx| window.click("confirm-option", cx));
    in_window(cx, window, |window, cx| window.click("confirm-accept", cx));

    assert_eq!(
        outcome.get(),
        Some(ConfirmOutcome {
            confirmed: true,
            checked: true
        })
    );
}

/// Cancelar cancela: la opción marcada no debe colarse como un sí.
#[gpui_kit::test]
async fn confirm_with_option_does_nothing_when_the_answer_is_no(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let outcome = confirm_delete_branch(cx, window);

    in_window(cx, window, |window, cx| window.click("confirm-option", cx));
    in_window(cx, window, |window, cx| window.click("confirm-cancel", cx));

    assert_eq!(
        outcome.get(),
        Some(ConfirmOutcome {
            confirmed: false,
            checked: true
        })
    );
}

/// `GlobalDialogs.test.js`: un confirm casi siempre lo lanza otro diálogo ya
/// abierto y tiene que quedar delante de él. El clic usa el hit testing real:
/// si el confirm quedara detrás, caería en el diálogo que lo pidió.
#[gpui_kit::test]
async fn keeps_the_confirm_above_a_dialog_opened_before_it(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = Rc::new(Cell::new(None));

    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            AppDialog::new()
                .title("Merge branches")
                .body(|_, _| div().h(px(320.)).child("Pick the branches to merge"))
                .open(window, cx);
            dialog::confirm(
                window,
                cx,
                confirm_options("Confirm merge"),
                move |ok, _, _| answer.set(Some(ok)),
            );
        }
    });
    in_window(cx, window, |window, cx| {
        assert_eq!(
            dialog::open_titles(window, cx),
            ["Merge branches", "Confirm merge"]
        );
        window.click("confirm-accept", cx);
    });

    assert_eq!(answer.get(), Some(true));
    // Responder cierra el confirm y deja en pie el diálogo que lo pidió.
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_titles(window, cx), ["Merge branches"]);
    });
}

#[gpui_kit::test]
async fn app_dialog_reports_dismissal_but_not_a_programmatic_close(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let dismissed = Rc::new(Cell::new(0));
    let open_dialog = {
        let dismissed = dismissed.clone();
        move |window: &mut Window, cx: &mut App| {
            let dismissed = dismissed.clone();
            AppDialog::new()
                .title("Fetch")
                .on_close(move |_, _| dismissed.set(dismissed.get() + 1))
                .open(window, cx);
        }
    };

    in_window(cx, window, open_dialog.clone());
    in_window(cx, window, |window, cx| {
        assert!(dialog::close(window, cx));
    });
    assert_eq!(dismissed.get(), 0);

    in_window(cx, window, open_dialog);
    in_window(cx, window, |window, cx| window.click("close", cx));
    assert_eq!(dismissed.get(), 1);
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

/// Avanza el foco con Tab hasta que el control `id` lo reciba.
fn focus_by_tab(window: &mut Window, cx: &mut App, id: &'static str) {
    window.render_frame(cx);
    for _ in 0..16 {
        if window.find(id).focused() == Some(true) {
            return;
        }
        window.focus_next(cx);
        window.render_frame(cx);
    }
    panic!("no se pudo enfocar `{id}` con Tab");
}

#[gpui_kit::test]
async fn enter_on_focused_cancel_button_cancels_a_confirm(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = Rc::new(Cell::new(None));

    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            dialog::confirm(
                window,
                cx,
                ConfirmOptions {
                    variant: ConfirmVariant::Destructive,
                    ..confirm_options("Delete branch")
                },
                move |ok, _, _| answer.set(Some(ok)),
            );
        }
    });
    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "confirm-cancel");
        window.press("enter", cx);
    });

    assert_eq!(answer.get(), Some(false));
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

#[gpui_kit::test]
async fn dialog_enter_confirms_from_root_and_from_a_checkbox(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let confirms = Rc::new(Cell::new(0usize));
    let submits = Rc::new(Cell::new(0usize));
    let cancels = Rc::new(Cell::new(0usize));
    let checked = Rc::new(Cell::new(false));
    let checkbox_changes = Rc::new(Cell::new(0usize));

    in_window(cx, window, {
        let confirms = confirms.clone();
        let submits = submits.clone();
        let cancels = cancels.clone();
        let checked = checked.clone();
        let checkbox_changes = checkbox_changes.clone();
        move |window, cx| {
            AppDialog::new()
                .title("Fetch")
                .body({
                    let checked = checked.clone();
                    let checkbox_changes = checkbox_changes.clone();
                    move |_, _| {
                        let checked = checked.clone();
                        let checkbox_changes = checkbox_changes.clone();
                        mergeon_ui::checkbox("dialog-prune")
                            .label("Prune deleted branches")
                            .checked(checked.get())
                            .on_change(move |value, window, _| {
                                checked.set(*value);
                                checkbox_changes.set(checkbox_changes.get() + 1);
                                window.refresh();
                            })
                    }
                })
                .footer({
                    let cancels = cancels.clone();
                    let submits = submits.clone();
                    move |_, _| {
                        let cancels = cancels.clone();
                        let submits = submits.clone();
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                mergeon_ui::button("dialog-cancel")
                                    .ghost()
                                    .sm()
                                    .label("Cancel")
                                    .on_click(move |_, _, _| cancels.set(cancels.get() + 1)),
                            )
                            .child(
                                mergeon_ui::button("dialog-submit")
                                    .sm()
                                    .label("Submit")
                                    .on_click(move |_, _, _| submits.set(submits.get() + 1)),
                            )
                    }
                })
                .on_confirm(move |_, _| {
                    confirms.set(confirms.get() + 1);
                    false
                })
                .open(window, cx);
        }
    });

    // Desde la raíz del diálogo, Enter ejecuta `on_confirm` una sola vez.
    in_window(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(confirms.get(), 1);
    assert_eq!(submits.get(), 0);
    assert_eq!(cancels.get(), 0);
    assert_eq!(checkbox_changes.get(), 0);

    // Con el foco en una casilla, Espacio la alterna y Enter sigue confirmando el diálogo.
    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "dialog-prune");
        window.press("space", cx);
    });
    assert!(checked.get());
    assert_eq!(checkbox_changes.get(), 1);
    assert_eq!(confirms.get(), 1);

    in_window(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(confirms.get(), 2);
    assert_eq!(checkbox_changes.get(), 1);
    assert_eq!(submits.get(), 0);
    assert_eq!(cancels.get(), 0);
}

#[gpui_kit::test]
async fn dialog_enter_and_space_on_focused_buttons_activate_only_that_button(
    cx: &mut TestAppContext,
) {
    let window = open(cx, empty);
    let confirms = Rc::new(Cell::new(0usize));
    let submits = Rc::new(Cell::new(0usize));
    let cancels = Rc::new(Cell::new(0usize));

    in_window(cx, window, {
        let confirms = confirms.clone();
        let submits = submits.clone();
        let cancels = cancels.clone();
        move |window, cx| {
            AppDialog::new()
                .title("Push")
                .footer({
                    let cancels = cancels.clone();
                    let submits = submits.clone();
                    move |_, _| {
                        let cancels = cancels.clone();
                        let submits = submits.clone();
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                mergeon_ui::button("dialog-cancel")
                                    .ghost()
                                    .sm()
                                    .label("Cancel")
                                    .on_click(move |_, _, _| cancels.set(cancels.get() + 1)),
                            )
                            .child(
                                mergeon_ui::button("dialog-submit")
                                    .sm()
                                    .label("Submit")
                                    .on_click(move |_, _, _| submits.set(submits.get() + 1)),
                            )
                    }
                })
                .on_confirm(move |_, _| {
                    confirms.set(confirms.get() + 1);
                    false
                })
                .open(window, cx);
        }
    });

    // Enter en Submit solo ejecuta su `on_click` una vez, sin disparar `on_confirm`.
    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "dialog-submit");
        window.press("enter", cx);
    });
    assert_eq!(submits.get(), 1);
    assert_eq!(confirms.get(), 0);
    assert_eq!(cancels.get(), 0);

    // Espacio sigue activando el botón enfocado.
    in_window(cx, window, |window, cx| window.press("space", cx));
    assert_eq!(submits.get(), 2);
    assert_eq!(confirms.get(), 0);
    assert_eq!(cancels.get(), 0);

    // Enter en Cancel solo ejecuta `Cancel`, nunca `on_confirm`.
    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "dialog-cancel");
        window.press("enter", cx);
    });
    assert_eq!(cancels.get(), 1);
    assert_eq!(submits.get(), 2);
    assert_eq!(confirms.get(), 0);
}

#[gpui_kit::test]
async fn dialog_enter_on_focused_close_button_dismisses_without_confirming(
    cx: &mut TestAppContext,
) {
    let window = open(cx, empty);
    let confirms = Rc::new(Cell::new(0usize));
    let closed = Rc::new(Cell::new(0usize));

    in_window(cx, window, {
        let confirms = confirms.clone();
        let closed = closed.clone();
        move |window, cx| {
            AppDialog::new()
                .title("Reset")
                .footer(|_, _| {
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            mergeon_ui::button("dialog-cancel")
                                .ghost()
                                .sm()
                                .label("Cancel"),
                        )
                        .child(mergeon_ui::button("dialog-submit").sm().label("Submit"))
                })
                .on_confirm(move |_, _| {
                    confirms.set(confirms.get() + 1);
                    true
                })
                .on_close(move |_, _| closed.set(closed.get() + 1))
                .open(window, cx);
        }
    });

    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "close");
        window.press("enter", cx);
    });

    assert_eq!(closed.get(), 1);
    assert_eq!(confirms.get(), 0);
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

#[gpui_kit::test]
async fn dialog_select_enter_picks_option_without_confirming_or_closing(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let confirms = Rc::new(Cell::new(0usize));
    let chosen = Rc::new(RefCell::new(Vec::<SharedString>::new()));
    let current = Rc::new(RefCell::new(SharedString::from("origin")));

    in_window(cx, window, {
        let confirms = confirms.clone();
        let chosen = chosen.clone();
        let current = current.clone();
        move |window, cx| {
            AppDialog::new()
                .title("Fetch")
                .body({
                    let chosen = chosen.clone();
                    let current = current.clone();
                    move |_, _| {
                        let chosen = chosen.clone();
                        let current = current.clone();
                        let value = current.borrow().clone();
                        mergeon_ui::select("dialog-remote")
                            .items(["origin", "upstream", "fork"])
                            .value(value)
                            .on_change(move |value, window, _| {
                                *current.borrow_mut() = value.clone();
                                chosen.borrow_mut().push(value.clone());
                                window.refresh();
                            })
                    }
                })
                .on_confirm(move |_, _| {
                    confirms.set(confirms.get() + 1);
                    true
                })
                .open(window, cx);
        }
    });

    in_window(cx, window, |window, cx| window.click("dialog-remote", cx));
    in_window(cx, window, |window, cx| {
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        window.within("popup-menu").press("down", cx);
        window.within("popup-menu").press("down", cx);
        window.within("popup-menu").press("enter", cx);
    });

    assert_eq!(*chosen.borrow(), ["upstream"]);
    assert_eq!(confirms.get(), 0);
    in_window(cx, window, |window, cx| {
        assert!(window.try_find("popup-menu").is_none());
        assert_eq!(dialog::open_count(window, cx), 1);
    });
}

#[gpui_kit::test]
async fn dialog_select_enter_on_focused_trigger_opens_dropdown_without_confirming(
    cx: &mut TestAppContext,
) {
    let window = open(cx, empty);
    let confirms = Rc::new(Cell::new(0usize));
    let current = Rc::new(RefCell::new(SharedString::from("origin")));

    in_window(cx, window, {
        let confirms = confirms.clone();
        let current = current.clone();
        move |window, cx| {
            AppDialog::new()
                .title("Fetch")
                .body({
                    let current = current.clone();
                    move |_, _| {
                        let current = current.clone();
                        let value = current.borrow().clone();
                        mergeon_ui::select("dialog-remote")
                            .items(["origin", "upstream", "fork"])
                            .value(value)
                            .on_change(move |value, window, _| {
                                *current.borrow_mut() = value.clone();
                                window.refresh();
                            })
                    }
                })
                .on_confirm(move |_, _| {
                    confirms.set(confirms.get() + 1);
                    true
                })
                .open(window, cx);
        }
    });

    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "dialog-remote");
        window.press("enter", cx);
    });

    assert_eq!(confirms.get(), 0);
    in_window(cx, window, |window, cx| {
        assert!(window.try_find("popup-menu").is_some());
        assert_eq!(dialog::open_count(window, cx), 1);
    });
}

#[gpui_kit::test]
async fn dialog_tabs_enter_switches_tab_without_confirming(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let confirms = Rc::new(Cell::new(0usize));
    let active = Rc::new(RefCell::new(SharedString::from("changes")));

    in_window(cx, window, {
        let confirms = confirms.clone();
        let active = active.clone();
        move |window, cx| {
            AppDialog::new()
                .title("New worktree")
                .body({
                    let active = active.clone();
                    move |_, _| {
                        let active = active.clone();
                        let selected = active.borrow().clone();
                        mergeon_ui::tabs("dialog-tabs")
                            .item(TabItem::new("changes", "Changes"))
                            .item(TabItem::new("tree", "Repo tree"))
                            .selected(selected)
                            .on_change(move |key, window, _| {
                                *active.borrow_mut() = key.clone();
                                window.refresh();
                            })
                    }
                })
                .on_confirm(move |_, _| {
                    confirms.set(confirms.get() + 1);
                    true
                })
                .open(window, cx);
        }
    });

    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "tab-tree");
        window.press("enter", cx);
    });

    assert_eq!(*active.borrow(), "tree");
    assert_eq!(confirms.get(), 0);
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 1);
    });
}

#[gpui_kit::test]
async fn destructive_confirm_enter_on_focused_option_toggles_instead_of_confirming(
    cx: &mut TestAppContext,
) {
    let window = open(cx, empty);
    let outcome = Rc::new(Cell::new(None));

    in_window(cx, window, {
        let outcome = outcome.clone();
        move |window, cx| {
            dialog::confirm_with_option(
                window,
                cx,
                ConfirmOptions {
                    variant: ConfirmVariant::Destructive,
                    ..confirm_options("Delete branch")
                },
                ConfirmOption {
                    label: "Also delete on origin".into(),
                    checked: false,
                },
                move |answer, _, _| outcome.set(Some(answer)),
            );
        }
    });

    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "confirm-option");
        window.press("enter", cx);
    });

    assert_eq!(outcome.get(), None);
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 1);
    });

    in_window(cx, window, |window, cx| window.click("confirm-accept", cx));
    assert_eq!(
        outcome.get(),
        Some(ConfirmOutcome {
            confirmed: true,
            checked: true,
        })
    );
}

#[gpui_kit::test]
async fn dialog_opened_from_context_menu_does_not_restore_focus_to_closed_menu(
    cx: &mut TestAppContext,
) {
    use gpui_kit::InteractiveElement as _;
    use gpui_kit::TestSupportExt as _;

    let window = open(cx, |_, _| {
        div()
            .id("branch-row")
            .test_support()
            .w(px(200.))
            .h(px(32.))
            .child("feature/login")
            .context_menu_items(|_, _| {
                vec![MenuItem::new("Delete branch").on_click(|window, cx| {
                    AppDialog::new().title("Delete branch").open(window, cx);
                })]
            })
            .into_any_element()
    });

    in_window(cx, window, |window, cx| {
        window.right_click("branch-row", cx)
    });
    in_window(cx, window, |window, cx| {
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        window.within("popup-menu").press("down", cx);
        window.within("popup-menu").press("enter", cx);
    });

    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_titles(window, cx), ["Delete branch"]);
        window.press("escape", cx);
    });

    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
        assert!(window.try_find("popup-menu").is_none());
        assert!(
            !window
                .context_stack()
                .iter()
                .any(|context| context.contains("PopupMenu")),
            "el foco no debe volver a un PopupMenu ya desmontado"
        );
        assert!(window.focused(cx).is_none());
    });
}

#[gpui_kit::test]
async fn reinit_after_clearing_key_bindings_preserves_button_enter_in_dialogs(
    cx: &mut TestAppContext,
) {
    let window = open(cx, empty);
    let answer = Rc::new(Cell::new(None));

    cx.update(|cx| {
        cx.clear_key_bindings();
        mergeon_ui::init(cx);
    });

    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            dialog::confirm(
                window,
                cx,
                confirm_options("Delete tag"),
                move |ok, _, _| answer.set(Some(ok)),
            );
        }
    });
    in_window(cx, window, |window, cx| {
        focus_by_tab(window, cx, "confirm-cancel");
        window.press("enter", cx);
    });

    assert_eq!(answer.get(), Some(false));
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

// ---------------------------------------------------------------------------
// prompt
// ---------------------------------------------------------------------------

type PromptAnswer = Rc<RefCell<Option<Option<SharedString>>>>;

fn open_prompt(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    options: PromptOptions,
) -> PromptAnswer {
    let answer: PromptAnswer = Rc::new(RefCell::new(None));
    in_window(cx, window, {
        let answer = answer.clone();
        move |window, cx| {
            dialog::prompt(window, cx, options, move |value, _, _| {
                *answer.borrow_mut() = Some(value)
            });
        }
    });
    answer
}

#[gpui_kit::test]
async fn prompt_returns_the_edited_text(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = open_prompt(
        cx,
        window,
        PromptOptions {
            title: "Rename branch".into(),
            default_value: "feature/login".into(),
            ..Default::default()
        },
    );

    // El campo ya tiene el foco y el cursor al final del valor inicial.
    in_window(cx, window, |window, cx| window.input("-v2", cx));
    in_window(cx, window, |window, cx| window.click("prompt-accept", cx));

    assert_eq!(*answer.borrow(), Some(Some("feature/login-v2".into())));
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

#[gpui_kit::test]
async fn prompt_confirms_with_enter_from_the_field(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = open_prompt(
        cx,
        window,
        PromptOptions {
            default_value: "v1.0.0".into(),
            ..Default::default()
        },
    );

    in_window(cx, window, |window, cx| window.press("enter", cx));

    assert_eq!(*answer.borrow(), Some(Some("v1.0.0".into())));
}

#[gpui_kit::test]
async fn prompt_keeps_an_invalid_value_open_until_it_is_fixed(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = open_prompt(
        cx,
        window,
        PromptOptions {
            default_value: "has spaces".into(),
            validate: Some(Rc::new(|value: &str| !value.contains(' '))),
            input_error_message: "Branch names cannot contain spaces".into(),
            ..Default::default()
        },
    );

    in_window(cx, window, |window, cx| window.click("prompt-accept", cx));
    assert_eq!(*answer.borrow(), None, "un valor inválido no responde");
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 1);
    });

    in_window(cx, window, |window, cx| window.click("prompt-cancel", cx));
    assert_eq!(*answer.borrow(), Some(None));
}

#[gpui_kit::test]
async fn dismissing_a_prompt_answers_nothing(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = open_prompt(
        cx,
        window,
        PromptOptions {
            textarea: true,
            ..Default::default()
        },
    );

    in_window(cx, window, |window, cx| window.click("close", cx));

    assert_eq!(*answer.borrow(), Some(None));
}

#[gpui_kit::test]
async fn single_line_prompt_clears_validation_error_on_edit(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let answer = open_prompt(
        cx,
        window,
        PromptOptions {
            default_value: "bad name".into(),
            validate: Some(Rc::new(|value: &str| !value.contains(' '))),
            input_error_message: "No spaces".into(),
            ..Default::default()
        },
    );

    in_window(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(*answer.borrow(), None);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("prompt-error").is_some());
    });

    in_window(cx, window, |window, cx| window.input("x", cx));
    in_window(cx, window, |window, _| {
        assert!(window.try_find("prompt-error").is_none());
    });
}

#[gpui_kit::test]
async fn multiline_prompt_supports_secondary_enter_tab_escape_error_clearing_and_rows(
    cx: &mut TestAppContext,
) {
    let window = open(cx, empty);

    // Comprobar primero que `rows` cambia realmente el alto del área de texto.
    let _ = open_prompt(
        cx,
        window,
        PromptOptions {
            textarea: true,
            ..Default::default()
        },
    );
    let default_height = in_window(cx, window, |window, _| {
        window.find("prompt-textarea").bounds().size.height
    });
    in_window(cx, window, |window, cx| {
        assert!(dialog::close(window, cx));
    });

    let answer = open_prompt(
        cx,
        window,
        PromptOptions {
            title: "Tag message".into(),
            textarea: true,
            rows: 8,
            validate: Some(Rc::new(|value: &str| !value.trim().is_empty())),
            input_error_message: "Message cannot be empty".into(),
            ..Default::default()
        },
    );

    let tall_height = in_window(cx, window, |window, _| {
        window.find("prompt-textarea").bounds().size.height
    });
    assert!(
        tall_height > default_height,
        "rows: 8 ({tall_height:?}) debe medir más que el alto por defecto ({default_height:?})"
    );

    // `secondary-enter` con valor inválido muestra el error sin cerrar.
    in_window(cx, window, |window, cx| window.press("secondary-enter", cx));
    assert_eq!(*answer.borrow(), None);
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 1);
        assert!(window.try_find("prompt-error").is_some());
    });

    // Editar limpia el error de validación.
    in_window(cx, window, |window, cx| window.input("line1", cx));
    in_window(cx, window, |window, _| {
        assert!(window.try_find("prompt-error").is_none());
    });

    // Enter simple añade salto de línea y no confirma el diálogo.
    in_window(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(*answer.borrow(), None);
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 1);
        window.input("line2", cx);
    });

    // Tab saca el foco del área de texto hacia Cancel sin insertar tabulación.
    in_window(cx, window, |window, cx| {
        window.press("tab", cx);
        assert_eq!(window.find("prompt-cancel").focused(), Some(true));
        window.press("shift-tab", cx);
        assert_eq!(window.find("prompt-cancel").focused(), Some(false));
    });

    // `secondary-enter` (Ctrl+Enter / Cmd+Enter) confirma sin añadir `\n` extra.
    in_window(cx, window, |window, cx| window.press("secondary-enter", cx));
    assert_eq!(*answer.borrow(), Some(Some("line1\nline2".into())));
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

// ---------------------------------------------------------------------------
// toasts (notify.test.js)
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn collapses_identical_toasts_fired_within_the_duplicate_window(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    let (first, second) = in_window(cx, window, |window, cx| {
        (
            toast::error(window, cx, "Could not read the repository status"),
            toast::error(window, cx, "Could not read the repository status"),
        )
    });

    assert_eq!(first, second);
    in_window(cx, window, |window, cx| {
        assert_eq!(
            toast::visible_titles(window, cx),
            ["Could not read the repository status"]
        );
    });
}

#[gpui_kit::test]
async fn a_toast_leaves_after_its_duration_and_can_be_shown_again(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    in_window(cx, window, |window, cx| {
        toast::success(window, cx, "Fetched");
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    in_window(cx, window, |window, cx| {
        // Sigue en pantalla: el reintento no apila nada encima.
        toast::success(window, cx, "Fetched");
        assert_eq!(toast::visible_titles(window, cx), ["Fetched"]);
    });

    // Los avisos de éxito duran 4 segundos.
    cx.executor().advance_clock(Duration::from_secs(3));
    in_window(cx, window, |window, cx| {
        assert!(toast::visible_titles(window, cx).is_empty());
        toast::success(window, cx, "Fetched");
        assert_eq!(toast::visible_titles(window, cx), ["Fetched"]);
    });
}

#[gpui_kit::test]
async fn lets_a_caller_update_a_toast_in_place_through_an_explicit_id(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    in_window(cx, window, |window, cx| {
        let options = || ToastOptions::new().id("app-update");
        toast::show(
            window,
            cx,
            ToastKind::Info,
            "Checking for updates…",
            options(),
        );
        toast::show(
            window,
            cx,
            ToastKind::Info,
            "Checking for updates…",
            options(),
        );
        toast::show(window, cx, ToastKind::Success, "Up to date", options());

        assert_eq!(toast::visible_titles(window, cx), ["Up to date"]);
    });
}

/// El avance de un toast de progreso se pinta en su sitio: no vuelve a entrar
/// ni pasa por delante de los avisos que llegaron después.
#[gpui_kit::test]
async fn updates_progress_in_place_without_reordering(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let id = toast::ToastId::from("operation:fetch:/repo");

    in_window(cx, window, |window, cx| {
        let progress = |value| {
            ToastOptions::new()
                .id("operation:fetch:/repo")
                .progress(value)
        };
        toast::show(
            window,
            cx,
            ToastKind::Info,
            "Fetch origin…",
            progress(ToastProgress::Indeterminate),
        );
        toast::info(window, cx, "Copied");
        toast::show(
            window,
            cx,
            ToastKind::Info,
            "Fetch origin…",
            progress(ToastProgress::Fraction(0.45)).description("Receiving objects 450/1,000"),
        );

        assert_eq!(
            toast::visible_titles(window, cx),
            ["Fetch origin…", "Copied"]
        );
        assert_eq!(
            toast::visible_progress(window, cx, &id),
            Some((
                ToastProgress::Fraction(0.45),
                Some("Receiving objects 450/1,000".into())
            ))
        );

        // El resultado (sin barra) sí lo reemplaza.
        toast::show(
            window,
            cx,
            ToastKind::Success,
            "Fetched origin",
            ToastOptions::new().id("operation:fetch:/repo"),
        );
        assert_eq!(
            toast::visible_titles(window, cx),
            ["Copied", "Fetched origin"]
        );
        assert_eq!(toast::visible_progress(window, cx, &id), None);
    });
}

#[gpui_kit::test]
async fn reports_one_error_only_once_even_when_several_layers_catch_it(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    in_window(cx, window, |window, cx| {
        let error = "spawn git ENOENT";
        toast::show(
            window,
            cx,
            ToastKind::Error,
            "Error loading status",
            ToastOptions::new().error(&error),
        );
        toast::show(
            window,
            cx,
            ToastKind::Error,
            "Could not read the repository status",
            ToastOptions::new().error(&error),
        );

        assert_eq!(toast::visible_titles(window, cx), ["Error loading status"]);
    });
}

/// El texto completo queda detrás de "Details", que abre el diálogo de detalle.
#[gpui_kit::test]
async fn details_opens_the_full_text_in_a_dialog(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    in_window(cx, window, |window, cx| {
        toast::show(
            window,
            cx,
            ToastKind::Error,
            "Update failed",
            ToastOptions::new().details("stderr output"),
        );
    });
    in_window(cx, window, |window, cx| window.click("action", cx));

    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_titles(window, cx), ["Notification details"]);
    });
}

/// Con una acción propia, "Details" se mueve a su lado y las dos siguen a mano.
#[gpui_kit::test]
async fn moves_details_next_to_the_caller_action(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let restarted = Rc::new(Cell::new(false));

    in_window(cx, window, {
        let restarted = restarted.clone();
        move |window, cx| {
            toast::show(
                window,
                cx,
                ToastKind::Error,
                "Update failed",
                ToastOptions::new()
                    .details("stderr output")
                    .action("Restart", move |_, _| restarted.set(true)),
            );
        }
    });
    in_window(cx, window, |window, cx| {
        assert!(window.try_find("cancel").is_some(), "Details sigue visible");
        window.click("action", cx);
    });

    assert!(restarted.get());
    in_window(cx, window, |window, cx| {
        assert_eq!(dialog::open_count(window, cx), 0);
    });
}

#[gpui_kit::test]
async fn keeps_short_informative_toasts_free_of_a_details_button(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    in_window(cx, window, |window, cx| {
        toast::success(window, cx, "Pushed.");
    });
    in_window(cx, window, |window, _| {
        assert!(window.try_find("toast").is_some());
        assert!(window.try_find("action").is_none());
    });
}

#[gpui_kit::test]
async fn the_close_button_dismisses_a_toast(cx: &mut TestAppContext) {
    let window = open(cx, empty);

    in_window(cx, window, |window, cx| {
        toast::warning(window, cx, "Slow");
    });
    in_window(cx, window, |window, cx| window.click("close", cx));

    in_window(cx, window, |window, cx| {
        assert!(toast::visible_titles(window, cx).is_empty());
    });
    cx.wait_for(window, Duration::from_secs(2), |window, _| {
        window.try_find("toast").is_none()
    })
    .await;
}

// ---------------------------------------------------------------------------
// menús y select
// ---------------------------------------------------------------------------

fn branch_menu(log: &Rc<RefCell<Vec<&'static str>>>) -> Vec<MenuItem> {
    let entry = |label: &'static str| {
        let log = log.clone();
        MenuItem::new(label).on_click(move |_, _| log.borrow_mut().push(label))
    };
    vec![
        entry("Checkout"),
        entry("Rename…").disabled(true),
        MenuItem::separator(),
        entry("Delete branch").danger().shortcut("Del"),
    ]
}

#[gpui_kit::test]
async fn a_dropdown_runs_the_clicked_entry_and_closes(cx: &mut TestAppContext) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let log = log.clone();
        move |_, _| {
            let log = log.clone();
            mergeon_ui::button("branch-actions")
                .outline()
                .label("Branch actions")
                .dropdown_menu_items(move |_, _| branch_menu(&log))
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| window.click("branch-actions", cx));
    in_window(cx, window, |window, cx| {
        // Una entrada deshabilitada no hace nada y no cierra el menú.
        window.within("popup-menu").click(1usize, cx);
    });
    assert!(log.borrow().is_empty());
    in_window(cx, window, |window, cx| {
        window.within("popup-menu").click(3usize, cx);
    });

    assert_eq!(*log.borrow(), ["Delete branch"]);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("popup-menu").is_none());
    });
}

#[gpui_kit::test]
async fn a_context_menu_opens_on_right_click_and_follows_the_keyboard(cx: &mut TestAppContext) {
    use gpui_kit::InteractiveElement as _;
    use gpui_kit::TestSupportExt as _;

    let log = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let log = log.clone();
        move |_, _| {
            let log = log.clone();
            div()
                .id("branch-row")
                .test_support()
                .w(px(200.))
                .h(px(32.))
                .child("feature/login")
                .context_menu_items(move |_, _| branch_menu(&log))
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| {
        window.right_click("branch-row", cx)
    });
    in_window(cx, window, |window, cx| {
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        // Las flechas saltan la entrada deshabilitada y el separador.
        window.within("popup-menu").press("down", cx);
        window.within("popup-menu").press("down", cx);
        window.within("popup-menu").press("enter", cx);
    });

    assert_eq!(*log.borrow(), ["Delete branch"]);
}

/// Un atajo de una letra ejecuta su entrada con el menú abierto; una entrada
/// deshabilitada se queda quieta y una letra sin entrada no hace nada.
#[gpui_kit::test]
async fn a_single_letter_shortcut_runs_its_entry_while_the_menu_is_open(cx: &mut TestAppContext) {
    use gpui_kit::InteractiveElement as _;
    use gpui_kit::TestSupportExt as _;

    let log = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let log = log.clone();
        move |_, _| {
            let log = log.clone();
            div()
                .id("commit-row")
                .test_support()
                .w(px(200.))
                .h(px(32.))
                .child("fix(cart)")
                .context_menu_items(move |_, _| {
                    let entry = |label: &'static str| {
                        let log = log.clone();
                        MenuItem::new(label).on_click(move |_, _| log.borrow_mut().push(label))
                    };
                    vec![
                        MenuItem::header("c4e1a07 · fix(cart)"),
                        entry("Checkout").shortcut("Ctrl+Enter"),
                        entry("Create branch here…").shortcut("B"),
                        entry("Merge into main").shortcut("M").disabled(true),
                    ]
                })
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| {
        window.right_click("commit-row", cx)
    });
    in_window(cx, window, |window, cx| {
        window.within("popup-menu").press("m", cx);
        window.within("popup-menu").press("x", cx);
    });
    assert!(log.borrow().is_empty());
    in_window(cx, window, |window, cx| {
        assert!(window.try_find("popup-menu").is_some());
        window.within("popup-menu").press("b", cx);
    });

    assert_eq!(*log.borrow(), ["Create branch here…"]);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("popup-menu").is_none());
    });
    // Con el menú cerrado la letra ya no hace nada.
    in_window(cx, window, |window, cx| window.press("b", cx));
    assert_eq!(log.borrow().len(), 1);
}

#[gpui_kit::test]
async fn menu_entries_describe_themselves(cx: &mut TestAppContext) {
    let window = open(cx, empty);
    let ran = Rc::new(Cell::new(0));
    let count = ran.clone();
    let entry = MenuItem::new("Checkout").on_click(move |_, _| count.set(count.get() + 1));

    assert_eq!(entry.text(), "Checkout");
    assert_eq!(entry.access_key(), None);
    assert_eq!(
        entry.clone().shortcut("B").access_key().as_deref(),
        Some("b")
    );
    assert_eq!(entry.clone().shortcut("Ctrl+B").access_key(), None);
    assert_eq!(MenuItem::header("c4e1a07 · fix").text(), "c4e1a07 · fix");
    assert!(MenuItem::separator().is_separator());
    in_window(cx, window, |window, cx| {
        assert!(entry.activate(window, cx));
        assert!(!entry.clone().disabled(true).activate(window, cx));
    });
    assert_eq!(ran.get(), 1);
}

#[gpui_kit::test]
async fn a_select_reports_the_chosen_value(cx: &mut TestAppContext) {
    let chosen = Rc::new(RefCell::new(Vec::<SharedString>::new()));
    let window = open(cx, {
        let chosen = chosen.clone();
        move |_, _| {
            let chosen = chosen.clone();
            div()
                .w(px(240.))
                .child(
                    mergeon_ui::select("remote")
                        .items(["origin", "upstream", "fork"])
                        .value("origin")
                        .on_change(move |value, _, _| chosen.borrow_mut().push(value.clone())),
                )
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| window.click("remote", cx));
    in_window(cx, window, |window, cx| {
        window.within("popup-menu").click(1usize, cx);
    });

    assert_eq!(*chosen.borrow(), ["upstream"]);
    in_window(cx, window, |window, _| {
        assert!(window.try_find("popup-menu").is_none());
    });
}

#[gpui_kit::test]
async fn a_disabled_select_does_not_open(cx: &mut TestAppContext) {
    let window = open(cx, |_, _| {
        div()
            .w(px(240.))
            .child(
                mergeon_ui::select("remote")
                    .items(["origin"])
                    .value("origin")
                    .disabled(true),
            )
            .into_any_element()
    });

    in_window(cx, window, |window, cx| window.click("remote", cx));

    in_window(cx, window, |window, _| {
        assert!(window.try_find("popup-menu").is_none());
    });
}

#[gpui_kit::test]
async fn a_select_shows_the_label_of_its_value_or_the_placeholder(_: &mut TestAppContext) {
    use mergeon_ui::SelectOption;

    let options = [
        SelectOption::new("light", "Light"),
        SelectOption::new("dark", "Dark"),
    ];
    let select = mergeon_ui::select("theme")
        .items(options.clone())
        .value("dark");
    assert_eq!(
        select.selected_label().map(|label| label.as_ref()),
        Some("Dark")
    );

    let unknown = mergeon_ui::select("theme").items(options).value("sepia");
    assert_eq!(unknown.selected_label(), None);
}

// ---------------------------------------------------------------------------
// controles
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn a_button_runs_on_click_and_from_the_keyboard(cx: &mut TestAppContext) {
    let clicks = Rc::new(Cell::new(0));
    let window = open(cx, {
        let clicks = clicks.clone();
        move |_, _| {
            let clicks = clicks.clone();
            mergeon_ui::button("fetch")
                .label("Fetch")
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| window.click("fetch", cx));
    assert_eq!(clicks.get(), 1);

    in_window(cx, window, |window, cx| {
        window.focus_next(cx);
        window.press("enter", cx);
    });
    assert_eq!(clicks.get(), 2);
}

#[gpui_kit::test]
async fn a_disabled_or_loading_button_ignores_clicks(cx: &mut TestAppContext) {
    let clicks = Rc::new(Cell::new(0));
    let window = open(cx, {
        let clicks = clicks.clone();
        move |_, _| {
            let disabled = clicks.clone();
            let loading = clicks.clone();
            div()
                .flex()
                .gap_2()
                .child(
                    mergeon_ui::button("disabled")
                        .label("Push")
                        .disabled(true)
                        .on_click(move |_, _, _| disabled.set(disabled.get() + 1)),
                )
                .child(
                    mergeon_ui::button("loading")
                        .label("Pushing")
                        .loading(true)
                        .on_click(move |_, _, _| loading.set(loading.get() + 1)),
                )
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| {
        window.click("disabled", cx);
        window.click("loading", cx);
    });

    assert_eq!(clicks.get(), 0);
}

#[gpui_kit::test]
async fn a_checkbox_requests_the_opposite_value(cx: &mut TestAppContext) {
    let requested = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let requested = requested.clone();
        move |_, _| {
            let on = requested.clone();
            let off = requested.clone();
            let locked = requested.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    mergeon_ui::checkbox("unchecked")
                        .label("Fetch all remotes")
                        .on_change(move |value, _, _| on.borrow_mut().push(*value)),
                )
                .child(
                    mergeon_ui::checkbox("checked")
                        .label("Prune")
                        .checked(true)
                        .on_change(move |value, _, _| off.borrow_mut().push(*value)),
                )
                .child(
                    mergeon_ui::checkbox("locked")
                        .label("Locked")
                        .disabled(true)
                        .on_change(move |value, _, _| locked.borrow_mut().push(*value)),
                )
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| {
        window.click("unchecked", cx);
        window.click("checked", cx);
        window.click("locked", cx);
    });

    assert_eq!(*requested.borrow(), [true, false]);
}

#[gpui_kit::test]
async fn tabs_report_the_clicked_key_and_skip_disabled_ones(cx: &mut TestAppContext) {
    let selected = Rc::new(RefCell::new(Vec::<SharedString>::new()));
    let window = open(cx, {
        let selected = selected.clone();
        move |_, _| {
            let selected = selected.clone();
            mergeon_ui::tabs("commit-tabs")
                .item(TabItem::new("changes", "Changes"))
                .item(TabItem::new("tree", "Repo tree"))
                .item(TabItem::new("stats", "Stats").disabled(true))
                .selected("changes")
                .on_change(move |key, _, _| selected.borrow_mut().push(key.clone()))
                .into_any_element()
        }
    });

    in_window(cx, window, |window, cx| {
        window.click("tab-stats", cx);
        window.click("tab-tree", cx);
    });

    assert_eq!(*selected.borrow(), ["tree"]);
}

fn radio_harness(
    cx: &mut TestAppContext,
    selected: &'static str,
) -> (AnyWindowHandle, Rc<RefCell<Vec<SharedString>>>) {
    let requested = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let requested = requested.clone();
        move |_, _| {
            let requested = requested.clone();
            mergeon_ui::radio_group("mode")
                .option(RadioOption::new("merge", "Merge"))
                .option(RadioOption::new("rebase", "Rebase").disabled(true))
                .option(RadioOption::new("squash", "Squash"))
                .selected(selected)
                .on_change(move |value, _, _| requested.borrow_mut().push(value.clone()))
                .into_any_element()
        }
    });
    (window, requested)
}

#[gpui_kit::test]
async fn a_radio_group_selects_on_click_and_ignores_disabled_options(cx: &mut TestAppContext) {
    let (window, requested) = radio_harness(cx, "merge");

    in_window(cx, window, |window, cx| {
        window.click("radio-rebase", cx);
        window.click("radio-squash", cx);
    });

    assert_eq!(*requested.borrow(), ["squash"]);
}

/// El grupo es una sola parada de tabulación; dentro se navega con flechas.
#[gpui_kit::test]
async fn arrows_move_a_radio_group_past_disabled_options(cx: &mut TestAppContext) {
    let (window, requested) = radio_harness(cx, "merge");

    in_window(cx, window, |window, cx| {
        window.focus_next(cx);
        window.press("down", cx);
    });
    assert_eq!(*requested.borrow(), ["squash"]);

    // Desde la primera, hacia arriba da la vuelta hasta la última habilitada.
    in_window(cx, window, |window, cx| window.press("up", cx));
    assert_eq!(*requested.borrow(), ["squash", "squash"]);
}
