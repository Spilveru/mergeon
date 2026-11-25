//! `UiHost` real: lo que los flujos de `AppModel` piden a la interfaz global
//! (avisos, confirmaciones, prompts y el selector de carpeta), pintado con los
//! componentes de `mergeon_ui` sobre la ventana principal.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui_kit::{AnyWindowHandle, App, PathPromptOptions};
use mergeon_app::{Answer, AppModel, CredentialsAnswer, CredentialsRequest, UiHost};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::notify::{NotifyType, ProgressToast, Toast, ToastActionKind, ToastId};
use mergeon_ui::Lucide;
use mergeon_ui::dialog::{self, ConfirmOption, ConfirmVariant};
use mergeon_ui::toast::{self, ToastKind, ToastOptions, ToastProgress};

pub struct ShellUiHost {
    window: AnyWindowHandle,
    /// Respuesta preparada por el modo de prueba (`MERGEON_TEST_CONTROL`).
    pick_directory: crate::test_mode::PickDirectoryAnswer,
    /// Avisos mostrados y hasta cuándo siguen en pantalla.
    shown: Rc<RefCell<Vec<(ToastId, Instant)>>>,
}

impl ShellUiHost {
    pub fn new(
        window: AnyWindowHandle,
        pick_directory: crate::test_mode::PickDirectoryAnswer,
    ) -> Self {
        Self {
            window,
            pick_directory,
            shown: Rc::default(),
        }
    }
}

fn toast_key(id: &ToastId) -> String {
    match id {
        ToastId::Auto(number) => format!("auto-{number}"),
        ToastId::Named(name) => name.clone(),
    }
}

fn text(value: Option<String>) -> gpui_kit::SharedString {
    value.unwrap_or_default().into()
}

fn confirm_options(options: &ConfirmOptions) -> dialog::ConfirmOptions {
    dialog::ConfirmOptions {
        title: text(options.title.clone()),
        description: text(options.description.clone()),
        confirm_text: text(options.confirm_text.clone()),
        cancel_text: text(options.cancel_text.clone()),
        dismiss_value: matches!(options.dismiss_value, Some(Some(true))),
        variant: match options.variant.as_deref() {
            Some("destructive") | Some("danger") => ConfirmVariant::Destructive,
            _ => ConfirmVariant::Default,
        },
    }
}

impl UiHost for ShellUiHost {
    fn notify(&self, toast: Toast, cx: &mut App) {
        let window = self.window;
        let shown = self.shown.clone();
        // El modelo está tomado durante la llamada: se pinta al terminar.
        cx.defer(move |cx| {
            let duration = toast.duration();
            let kind = match toast.kind {
                NotifyType::Success => ToastKind::Success,
                NotifyType::Info => ToastKind::Info,
                NotifyType::Warning => ToastKind::Warning,
                NotifyType::Error => ToastKind::Error,
            };
            let mut options = ToastOptions::new()
                .id(toast_key(&toast.id))
                .duration(duration);
            if toast.full_message != toast.title || toast.kind == NotifyType::Error {
                options = options.details(toast.full_message.clone());
            }
            if let Some(description) = toast.description.clone() {
                options = options.description(description);
            }
            for (slot, action) in [(0, &toast.action), (1, &toast.cancel)] {
                let Some(action) = action else { continue };
                let ToastActionKind::Custom(key) = &action.kind else {
                    continue;
                };
                let key = key.clone();
                let run = move |_: &mut gpui_kit::Window, cx: &mut App| {
                    AppModel::global(cx).update(cx, |model, cx| {
                        model.run_toast_action(&key, cx);
                    });
                };
                options = if slot == 0 {
                    options.action(action.label.clone(), run)
                } else {
                    options.cancel(action.label.clone(), run)
                };
            }

            let title = toast.title.clone();
            let result = window.update(cx, |_, window, cx| {
                toast::show(window, cx, kind, &title, options);
            });
            if result.is_ok() {
                let mut shown = shown.borrow_mut();
                let now = Instant::now();
                shown.retain(|(id, until)| *until > now && *id != toast.id);
                shown.push((toast.id, now + duration.min(Duration::from_secs(8))));
            }
        });
    }

    fn show_progress(&self, progress: ProgressToast, cx: &mut App) {
        let window = self.window;
        let shown = self.shown.clone();
        cx.defer(move |cx| {
            // Las flechas de la barra de herramientas.
            let icon = match progress.operation.as_str() {
                "push" => Lucide::Upload,
                "pull" | "clone" => Lucide::Download,
                _ => Lucide::ArrowDownToLine,
            };
            let mut options = ToastOptions::new()
                .id(toast_key(&progress.id))
                .icon(icon)
                .progress(match progress.fraction {
                    Some(fraction) => ToastProgress::Fraction(fraction),
                    None => ToastProgress::Indeterminate,
                });
            if let Some(description) = progress.description.clone() {
                options = options.description(description);
            }
            if let Some(action) = &progress.cancel
                && let ToastActionKind::Custom(key) = &action.kind
            {
                let key = key.clone();
                options = options.cancel(action.label.clone(), move |_, cx| {
                    AppModel::global(cx).update(cx, |model, cx| {
                        model.run_toast_action(&key, cx);
                    });
                });
            }
            let result = window.update(cx, |_, window, cx| {
                toast::show(window, cx, ToastKind::Info, &progress.title, options);
            });
            if result.is_ok() {
                // Mientras dure la operación cuenta como visible.
                let mut shown = shown.borrow_mut();
                shown.retain(|(id, _)| *id != progress.id);
                shown.push((progress.id, Instant::now() + Duration::from_secs(3600)));
            }
        });
    }

    fn dismiss_toast(&self, id: &ToastId, cx: &mut App) {
        let window = self.window;
        let shown = self.shown.clone();
        let id = id.clone();
        cx.defer(move |cx| {
            shown.borrow_mut().retain(|(shown_id, _)| *shown_id != id);
            let _ = window.update(cx, |_, window, cx| {
                toast::dismiss(window, cx, &toast::ToastId::from(toast_key(&id)));
            });
        });
    }

    fn visible_toast_ids(&self, _cx: &App) -> Vec<ToastId> {
        let now = Instant::now();
        self.shown
            .borrow()
            .iter()
            .filter(|(_, until)| *until > now)
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn confirm(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmValue> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                dialog::confirm(window, cx, confirm_options(&options), move |ok, _, _| {
                    let _ = sender.send(Some(ok));
                });
            });
        });
        receiver
    }

    fn confirm_with_option(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmAnswer> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let option = ConfirmOption {
                label: text(options.option_label.clone()),
                checked: options.option_checked,
            };
            let _ = window.update(cx, |_, window, cx| {
                dialog::confirm_with_option(
                    window,
                    cx,
                    confirm_options(&options),
                    option,
                    move |outcome, _, _| {
                        let _ = sender.send(ConfirmAnswer::WithOption {
                            confirmed: outcome.confirmed,
                            checked: outcome.checked,
                        });
                    },
                );
            });
        });
        receiver
    }

    fn prompt(&self, options: PromptOptions, cx: &mut App) -> Answer<PromptAnswer> {
        show_prompt(self.window, options, cx)
    }

    fn pick_directory(&self, cx: &mut App) -> Answer<Option<PathBuf>> {
        let (sender, receiver) = oneshot::channel();
        if let Some(prepared) = self.pick_directory.borrow_mut().take() {
            let _ = sender.send(prepared);
            return receiver;
        }
        let window = self.window;
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        let race = Rc::new(RefCell::new(PickerRace::new(sender)));

        // En Linux, sin `xdg-desktop-portal` la petición se queda esperando
        // para siempre (GPUI no da error): pasado un rato se ofrece escribir
        // la ruta, sin cancelar el selector por si al final aparece.
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            let race = race.clone();
            let timer = cx.background_executor().timer(PICKER_PATIENCE);
            cx.spawn(async move |cx| {
                timer.await;
                cx.update(|cx| {
                    let step = race.borrow_mut().timed_out();
                    apply_picker_step(
                        step,
                        &race,
                        window,
                        "The system folder picker hasn't opened. Type or paste the repository's path instead.",
                        cx,
                    );
                });
            })
            .detach();
        }

        cx.spawn(async move |cx| {
            let (step, description) = match picked.await {
                Ok(Ok(paths)) => (
                    race.borrow_mut()
                        .picker_answered(paths.and_then(|paths| paths.into_iter().next())),
                    String::new(),
                ),
                Ok(Err(error)) => {
                    // Sin portal de escritorio en Linux el selector puede fallar.
                    log::error!("el selector de carpetas falló: {error}");
                    (
                        race.borrow_mut().picker_failed(),
                        format!(
                            "The system folder picker isn't available ({error}). Type or paste the repository's path instead."
                        ),
                    )
                }
                Err(_) => (race.borrow_mut().picker_answered(None), String::new()),
            };
            cx.update(|cx| apply_picker_step(step, &race, window, &description, cx));
        })
        .detach();
        receiver
    }

    fn ask_credentials(
        &self,
        request: CredentialsRequest,
        cx: &mut App,
    ) -> Answer<Option<CredentialsAnswer>> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        // El modelo está tomado durante la llamada: se abre al terminar.
        cx.defer(move |cx| {
            let opened = window.update(cx, |_, window, cx| {
                mergeon_view_home::open_credentials_dialog(request, window, cx, move |answer| {
                    let _ = sender.send(answer);
                });
            });
            if opened.is_err() {
                log::error!("no se pudo abrir el diálogo de credenciales");
            }
        });
        receiver
    }
}

/// Cuánto se espera al selector de carpetas antes de ofrecer escribir la ruta.
const PICKER_PATIENCE: Duration = Duration::from_secs(2);

/// Qué hacer tras un paso de la carrera entre el selector del sistema y el
/// campo de ruta.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickerStep {
    /// Nada (ya se respondió, o el paso no cambia nada).
    Nothing,
    /// Responder con esto.
    Answer(Option<PathBuf>),
    /// Ofrecer el campo de ruta.
    OfferPathField,
    /// Cerrar el campo de ruta y responder con lo que eligió el selector.
    CloseFieldAndAnswer(PathBuf),
}

/// La carrera entre el selector de carpetas del sistema (que en Linux sin
/// portal nunca contesta) y el campo de ruta que se ofrece a los 2 s.
/// Gana el primero que da una ruta; el selector no se cancela.
struct PickerRace {
    sender: Option<oneshot::Sender<Option<PathBuf>>>,
    field_open: bool,
    /// El diálogo del campo de ruta, para cerrarlo si gana el selector.
    field_dialog: Option<mergeon_ui::dialog::DialogId>,
}

impl PickerRace {
    fn new(sender: oneshot::Sender<Option<PathBuf>>) -> Self {
        Self {
            sender: Some(sender),
            field_open: false,
            field_dialog: None,
        }
    }

    fn answered(&self) -> bool {
        self.sender.is_none()
    }

    fn timed_out(&mut self) -> PickerStep {
        if self.answered() || self.field_open {
            return PickerStep::Nothing;
        }
        self.field_open = true;
        PickerStep::OfferPathField
    }

    fn picker_failed(&mut self) -> PickerStep {
        self.timed_out()
    }

    fn picker_answered(&mut self, path: Option<PathBuf>) -> PickerStep {
        if self.answered() {
            return PickerStep::Nothing;
        }
        match (path, self.field_open) {
            (Some(path), true) => PickerStep::CloseFieldAndAnswer(path),
            (path, false) => PickerStep::Answer(path),
            // Cancelar el selector tarde no cierra el campo que ya se ve.
            (None, true) => PickerStep::Nothing,
        }
    }

    fn typed(&mut self, path: PathBuf) -> PickerStep {
        if self.answered() {
            return PickerStep::Nothing;
        }
        PickerStep::Answer(Some(path))
    }

    fn field_cancelled(&mut self) -> PickerStep {
        if self.answered() {
            return PickerStep::Nothing;
        }
        PickerStep::Answer(None)
    }

    fn send(&mut self, answer: Option<PathBuf>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(answer);
        }
    }
}

fn apply_picker_step(
    step: PickerStep,
    race: &Rc<RefCell<PickerRace>>,
    window: AnyWindowHandle,
    description: &str,
    cx: &mut App,
) {
    match step {
        PickerStep::Nothing => {}
        PickerStep::Answer(answer) => race.borrow_mut().send(answer),
        PickerStep::CloseFieldAndAnswer(path) => {
            let dialog = race.borrow_mut().field_dialog.take();
            race.borrow_mut().send(Some(path));
            if let Some(id) = dialog {
                let _ = window.update(cx, |_, window, cx| {
                    mergeon_ui::dialog::close_id(window, cx, id);
                });
            }
        }
        PickerStep::OfferPathField => {
            let description = if description.is_empty() {
                "The system folder picker hasn't opened. Type or paste the repository's path instead."
                    .to_string()
            } else {
                description.to_string()
            };
            let (on_open, on_cancel) = (race.clone(), race.clone());
            let opened = window.update(cx, |_, window, cx| {
                mergeon_view_home::open_repository_path_dialog(
                    description,
                    window,
                    cx,
                    move |path, window, cx| {
                        let step = on_open.borrow_mut().typed(path);
                        apply_picker_step(step, &on_open, window.window_handle(), "", cx);
                    },
                    move |window, cx| {
                        let step = on_cancel.borrow_mut().field_cancelled();
                        apply_picker_step(step, &on_cancel, window.window_handle(), "", cx);
                    },
                )
            });
            match opened {
                Ok(id) => race.borrow_mut().field_dialog = Some(id),
                Err(_) => race.borrow_mut().send(None),
            }
        }
    }
}

/// El diálogo de texto de `prompt`, también para pedir una carpeta a mano
/// cuando el selector del sistema no está disponible.
fn show_prompt(
    window: AnyWindowHandle,
    options: PromptOptions,
    cx: &mut App,
) -> Answer<PromptAnswer> {
    let (sender, receiver) = oneshot::channel();
    cx.defer(move |cx| {
        let validate = options
            .pattern
            .as_deref()
            .and_then(|pattern| regex::Regex::new(pattern).ok())
            .map(|pattern| {
                Rc::new(move |value: &str| pattern.is_match(value)) as dialog::PromptValidator
            });
        let ui_options = dialog::PromptOptions {
            title: text(options.title.clone()),
            description: text(options.description.clone()),
            confirm_text: text(options.confirm_text.clone()),
            cancel_text: text(options.cancel_text.clone()),
            placeholder: text(options.placeholder.clone()),
            default_value: text(options.default_value.clone()),
            validate,
            input_error_message: text(options.input_error_message.clone()),
            textarea: options.textarea,
            rows: options.rows.map(|rows| rows as usize).unwrap_or_default(),
        };
        let _ = window.update(cx, |_, window, cx| {
            dialog::prompt(window, cx, ui_options, move |value, _, _| {
                let _ = sender.send(PromptAnswer {
                    confirmed: value.is_some(),
                    value: value.map(|value| value.to_string()),
                });
            });
        });
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_race() -> (PickerRace, oneshot::Receiver<Option<PathBuf>>) {
        let (sender, receiver) = oneshot::channel();
        (PickerRace::new(sender), receiver)
    }

    #[test]
    fn the_picker_answering_in_time_wins() {
        let (mut race, _receiver) = new_race();
        assert_eq!(
            race.picker_answered(Some(PathBuf::from("/srv/web"))),
            PickerStep::Answer(Some(PathBuf::from("/srv/web")))
        );
        let (mut race, _receiver) = new_race();
        assert_eq!(race.picker_answered(None), PickerStep::Answer(None));
    }

    #[test]
    fn a_silent_picker_offers_the_path_field_once() {
        let (mut race, _receiver) = new_race();
        assert_eq!(race.timed_out(), PickerStep::OfferPathField);
        assert_eq!(race.timed_out(), PickerStep::Nothing);
        // Un fallo después de ofrecerlo no abre otro.
        assert_eq!(race.picker_failed(), PickerStep::Nothing);
        assert_eq!(
            race.typed(PathBuf::from("/srv/web")),
            PickerStep::Answer(Some(PathBuf::from("/srv/web")))
        );
    }

    #[test]
    fn a_late_picker_still_counts_unless_the_path_was_already_typed() {
        let (mut race, _receiver) = new_race();
        race.timed_out();
        // Cancelar el selector tarde deja el campo abierto.
        assert_eq!(race.picker_answered(None), PickerStep::Nothing);
        assert_eq!(
            race.picker_answered(Some(PathBuf::from("/srv/web"))),
            PickerStep::CloseFieldAndAnswer(PathBuf::from("/srv/web"))
        );

        let (mut race, _receiver) = new_race();
        race.timed_out();
        race.send(Some(PathBuf::from("/typed")));
        assert_eq!(
            race.picker_answered(Some(PathBuf::from("/srv/web"))),
            PickerStep::Nothing
        );
        assert_eq!(race.field_cancelled(), PickerStep::Nothing);
    }

    #[test]
    fn a_failing_picker_offers_the_field_right_away() {
        let (mut race, _receiver) = new_race();
        assert_eq!(race.picker_failed(), PickerStep::OfferPathField);
        assert_eq!(race.field_cancelled(), PickerStep::Answer(None));
    }

    #[test]
    fn only_one_answer_is_ever_sent() {
        let (mut race, mut receiver) = new_race();
        race.send(Some(PathBuf::from("/first")));
        race.send(Some(PathBuf::from("/second")));
        assert_eq!(
            receiver.try_recv().unwrap(),
            Some(Some(PathBuf::from("/first")))
        );
    }
}
