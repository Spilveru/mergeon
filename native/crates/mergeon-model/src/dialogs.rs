//! Estado de los diálogos globales de confirmación y de entrada de texto
//! (`src/lib/ui/dialogs.js`), sin la parte que pinta.
//!
//! El original guardaba en el estado el `resolve` de una promesa. Aquí las
//! funciones que cierran un diálogo devuelven la respuesta, y es la interfaz la
//! que se la entrega a quien preguntó. `pending` dice si hay alguien esperando.
//!
//! La validación del prompt (`pattern` + `inputErrorMessage`), que vivía en
//! `GlobalDialogs.vue`, está en [`DialogState::submit_prompt`].

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Respuesta de un `confirm` sencillo: `Some(true)` confirma, `Some(false)`
/// cancela y `None` es "sin respuesta" (el `null`/`undefined` que el original
/// admite como `dismissValue` para distinguir un cierre de una cancelación).
pub type ConfirmValue = Option<bool>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmState {
    pub open: bool,
    pub title: String,
    pub description: String,
    pub confirm_text: String,
    pub cancel_text: String,
    /// Lo que se responde si el diálogo se cierra sin elegir (Escape, clic fuera).
    pub dismiss_value: ConfirmValue,
    /// `default`, `destructive`, `secondary`.
    pub variant: String,
    // Una decisión secundaria que viaja con la confirmación: por ejemplo, si
    // borrar también en el remoto.
    pub option_label: String,
    pub option_checked: bool,
    /// Hay una pregunta esperando respuesta (el `resolve` del original).
    #[serde(skip)]
    pub pending: bool,
}

impl Default for ConfirmState {
    fn default() -> Self {
        Self {
            open: false,
            title: String::new(),
            description: String::new(),
            confirm_text: "Confirm".to_string(),
            cancel_text: "Cancel".to_string(),
            dismiss_value: Some(false),
            variant: "default".to_string(),
            option_label: String::new(),
            option_checked: false,
            pending: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptState {
    pub open: bool,
    pub title: String,
    pub description: String,
    pub confirm_text: String,
    pub cancel_text: String,
    pub placeholder: String,
    pub default_value: String,
    /// Expresión regular que debe cumplir el valor (`None`: cualquiera vale).
    pub pattern: Option<String>,
    pub input_error_message: String,
    pub textarea: bool,
    pub rows: u32,
    #[serde(skip)]
    pub pending: bool,
}

impl Default for PromptState {
    fn default() -> Self {
        Self {
            open: false,
            title: String::new(),
            description: String::new(),
            confirm_text: "Confirm".to_string(),
            cancel_text: "Cancel".to_string(),
            placeholder: String::new(),
            default_value: String::new(),
            pattern: None,
            input_error_message: String::new(),
            textarea: false,
            rows: 3,
            pending: false,
        }
    }
}

/// Opciones de `confirm` y `confirmWithOption`. Lo que falte (o venga vacío)
/// toma el texto por defecto.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfirmOptions {
    pub title: Option<String>,
    pub description: Option<String>,
    pub confirm_text: Option<String>,
    pub cancel_text: Option<String>,
    /// `None`: no se indicó, y cerrar equivale a cancelar (`Some(false)`).
    #[serde(skip)]
    pub dismiss_value: Option<ConfirmValue>,
    pub variant: Option<String>,
    /// Solo `confirm_with_option`.
    pub option_label: Option<String>,
    pub option_checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptOptions {
    pub title: Option<String>,
    pub description: Option<String>,
    pub confirm_text: Option<String>,
    pub cancel_text: Option<String>,
    pub placeholder: Option<String>,
    pub default_value: Option<String>,
    pub pattern: Option<String>,
    pub input_error_message: Option<String>,
    pub textarea: bool,
    pub rows: Option<u32>,
}

/// Lo que recibe quien preguntó con `confirm` o `confirmWithOption`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfirmAnswer {
    /// Confirmación con decisión secundaria. Va primero para que la forma con
    /// campos no se confunda con la simple al deserializar.
    WithOption { confirmed: bool, checked: bool },
    /// `confirm` sencillo: se devuelve el valor tal cual.
    Plain(ConfirmValue),
}

impl ConfirmAnswer {
    pub fn confirmed(self) -> bool {
        match self {
            ConfirmAnswer::Plain(value) => value == Some(true),
            ConfirmAnswer::WithOption { confirmed, .. } => confirmed,
        }
    }
}

/// Lo que recibe quien preguntó con `prompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAnswer {
    pub confirmed: bool,
    pub value: Option<String>,
}

fn text_or(value: &Option<String>, fallback: &str) -> String {
    value
        .as_deref()
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Estado de los dos diálogos globales (un módulo con estado en el original).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogState {
    pub confirm_state: ConfirmState,
    pub prompt_state: PromptState,
}

impl DialogState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Abre una confirmación sencilla; la respuesta será [`ConfirmAnswer::Plain`].
    pub fn confirm(&mut self, options: &ConfirmOptions) {
        let state = &mut self.confirm_state;
        state.title = text_or(&options.title, "Are you sure?");
        state.description = text_or(&options.description, "");
        state.confirm_text = text_or(&options.confirm_text, "Confirm");
        state.cancel_text = text_or(&options.cancel_text, "Cancel");
        state.dismiss_value = options.dismiss_value.unwrap_or(Some(false));
        state.variant = text_or(&options.variant, "default");
        state.pending = true;
        state.open = true;
    }

    /// Confirmar y, de paso, decidir algo secundario. Se separa de `confirm` porque
    /// devuelve un objeto en vez de un booleano: mezclar las dos formas en la misma
    /// función se presta a que alguien lea el objeto como si fuera verdadero.
    pub fn confirm_with_option(&mut self, options: &ConfirmOptions) {
        let state = &mut self.confirm_state;
        state.title = text_or(&options.title, "Are you sure?");
        state.description = text_or(&options.description, "");
        state.confirm_text = text_or(&options.confirm_text, "Confirm");
        state.cancel_text = text_or(&options.cancel_text, "Cancel");
        state.dismiss_value = Some(false);
        state.variant = text_or(&options.variant, "default");
        state.option_label = text_or(&options.option_label, "");
        state.option_checked = options.option_checked;
        state.pending = true;
        state.open = true;
    }

    pub fn set_confirm_option(&mut self, checked: bool) {
        self.confirm_state.option_checked = checked;
    }

    /// Cierra la confirmación con `confirmed` y devuelve la respuesta para quien
    /// preguntó (`None` si no había nadie esperando).
    pub fn resolve_confirm(&mut self, confirmed: ConfirmValue) -> Option<ConfirmAnswer> {
        let state = std::mem::take(&mut self.confirm_state);
        if !state.pending {
            return None;
        }

        Some(if state.option_label.is_empty() {
            ConfirmAnswer::Plain(confirmed)
        } else {
            ConfirmAnswer::WithOption {
                confirmed: confirmed == Some(true),
                checked: state.option_checked,
            }
        })
    }

    /// El diálogo se cerró sin elegir (Escape, clic fuera): responde `dismissValue`.
    pub fn dismiss_confirm(&mut self) -> Option<ConfirmAnswer> {
        let dismiss_value = self.confirm_state.dismiss_value;
        self.resolve_confirm(dismiss_value)
    }

    pub fn prompt(&mut self, options: &PromptOptions) {
        let state = &mut self.prompt_state;
        state.title = text_or(&options.title, "Enter a value");
        state.description = text_or(&options.description, "");
        state.confirm_text = text_or(&options.confirm_text, "Confirm");
        state.cancel_text = text_or(&options.cancel_text, "Cancel");
        state.placeholder = text_or(&options.placeholder, "");
        state.default_value = text_or(&options.default_value, "");
        state.pattern = options
            .pattern
            .clone()
            .filter(|pattern| !pattern.is_empty());
        state.input_error_message = text_or(&options.input_error_message, "Invalid value");
        state.textarea = options.textarea;
        state.rows = options.rows.filter(|rows| *rows != 0).unwrap_or(3);
        state.pending = true;
        state.open = true;
    }

    /// Cierra el prompt entregando `payload` a quien preguntó.
    pub fn resolve_prompt(&mut self, payload: PromptAnswer) -> Option<PromptAnswer> {
        let state = std::mem::take(&mut self.prompt_state);
        state.pending.then_some(payload)
    }

    /// Botón de confirmar del prompt. Si el valor no cumple `pattern` devuelve
    /// el mensaje de error y el diálogo sigue abierto; si lo cumple, lo cierra.
    pub fn submit_prompt(&mut self, value: &str) -> Result<Option<PromptAnswer>, String> {
        if let Some(pattern) = &self.prompt_state.pattern {
            // Un patrón que no compila no puede dar nada por bueno.
            let matches = Regex::new(pattern).is_ok_and(|regex| regex.is_match(value));
            if !matches {
                return Err(text_or(
                    &Some(self.prompt_state.input_error_message.clone()),
                    "Invalid value",
                ));
            }
        }

        Ok(self.resolve_prompt(PromptAnswer {
            confirmed: true,
            value: Some(value.to_string()),
        }))
    }

    /// El prompt se cerró sin confirmar.
    pub fn dismiss_prompt(&mut self) -> Option<PromptAnswer> {
        self.resolve_prompt(PromptAnswer {
            confirmed: false,
            value: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delete_branch() -> ConfirmOptions {
        ConfirmOptions {
            title: Some("Delete branch".to_string()),
            option_label: Some("Also on origin".to_string()),
            ..Default::default()
        }
    }

    // --- confirm with a secondary decision ---

    // Lo irreversible para los demás no debería venir marcado de fábrica.
    #[test]
    fn starts_with_the_option_off() {
        let mut dialogs = DialogState::new();
        dialogs.confirm_with_option(&delete_branch());
        assert!(!dialogs.confirm_state.option_checked);

        assert_eq!(
            dialogs.resolve_confirm(Some(true)),
            Some(ConfirmAnswer::WithOption {
                confirmed: true,
                checked: false
            })
        );
    }

    #[test]
    fn carries_the_option_back_with_the_answer() {
        let mut dialogs = DialogState::new();
        dialogs.confirm_with_option(&delete_branch());
        dialogs.set_confirm_option(true);

        assert_eq!(
            dialogs.resolve_confirm(Some(true)),
            Some(ConfirmAnswer::WithOption {
                confirmed: true,
                checked: true
            })
        );
    }

    // Cancelar cancela: la opción marcada no debe colarse como un sí.
    #[test]
    fn does_nothing_when_the_answer_is_no_whatever_the_option_says() {
        let mut dialogs = DialogState::new();
        dialogs.confirm_with_option(&delete_branch());
        dialogs.set_confirm_option(true);

        let answer = dialogs.resolve_confirm(Some(false)).unwrap();

        assert_eq!(
            answer,
            ConfirmAnswer::WithOption {
                confirmed: false,
                checked: true
            }
        );
        assert!(!answer.confirmed());
    }

    // `confirm` sigue devolviendo un booleano: mezclar las dos formas haría que un
    // objeto se leyera como verdadero y se borrara sin preguntar.
    #[test]
    fn leaves_the_plain_confirm_returning_a_boolean() {
        let mut dialogs = DialogState::new();
        dialogs.confirm(&ConfirmOptions {
            title: Some("Delete tag".to_string()),
            ..Default::default()
        });

        let answer = dialogs.resolve_confirm(Some(true));

        assert_eq!(answer, Some(ConfirmAnswer::Plain(Some(true))));
        assert_eq!(
            serde_json::to_value(answer).unwrap(),
            serde_json::json!(true)
        );
    }

    #[test]
    fn forgets_the_option_once_the_dialog_closes() {
        let mut dialogs = DialogState::new();
        dialogs.confirm_with_option(&delete_branch());
        dialogs.set_confirm_option(true);
        dialogs.resolve_confirm(Some(true));

        assert_eq!(dialogs.confirm_state.option_label, "");
        assert!(!dialogs.confirm_state.option_checked);
        assert_eq!(dialogs.confirm_state, ConfirmState::default());
    }

    // --- lo que el original repartía entre dialogs.js y GlobalDialogs.vue ---

    #[test]
    fn fills_in_the_default_texts() {
        let mut dialogs = DialogState::new();
        dialogs.confirm(&ConfirmOptions::default());

        let state = &dialogs.confirm_state;
        assert!(state.open && state.pending);
        assert_eq!(state.title, "Are you sure?");
        assert_eq!(state.confirm_text, "Confirm");
        assert_eq!(state.cancel_text, "Cancel");
        assert_eq!(state.variant, "default");
        assert_eq!(state.dismiss_value, Some(false));

        dialogs.confirm(&ConfirmOptions {
            title: Some("Discard changes".to_string()),
            description: Some("This cannot be undone.".to_string()),
            confirm_text: Some("Discard".to_string()),
            variant: Some("destructive".to_string()),
            ..Default::default()
        });
        assert_eq!(dialogs.confirm_state.title, "Discard changes");
        assert_eq!(dialogs.confirm_state.description, "This cannot be undone.");
        assert_eq!(dialogs.confirm_state.confirm_text, "Discard");
        assert_eq!(dialogs.confirm_state.variant, "destructive");
    }

    #[test]
    fn answers_with_the_dismiss_value_when_closed_without_choosing() {
        let mut dialogs = DialogState::new();
        dialogs.confirm(&ConfirmOptions::default());
        assert_eq!(
            dialogs.dismiss_confirm(),
            Some(ConfirmAnswer::Plain(Some(false)))
        );

        // Quien necesita distinguir "cerró" de "canceló" pide otro valor.
        dialogs.confirm(&ConfirmOptions {
            dismiss_value: Some(None),
            ..Default::default()
        });
        assert_eq!(dialogs.dismiss_confirm(), Some(ConfirmAnswer::Plain(None)));
        assert!(!dialogs.confirm_state.open);
    }

    #[test]
    fn has_nobody_to_answer_when_no_dialog_is_open() {
        let mut dialogs = DialogState::new();

        assert_eq!(dialogs.resolve_confirm(Some(true)), None);
        assert_eq!(dialogs.dismiss_prompt(), None);
    }

    #[test]
    fn validates_the_prompt_value_before_closing() {
        let mut dialogs = DialogState::new();
        dialogs.prompt(&PromptOptions {
            title: Some("Remote URL".to_string()),
            pattern: Some("^https?://.+".to_string()),
            input_error_message: Some("Please enter a valid URL (https://...)".to_string()),
            ..Default::default()
        });

        assert_eq!(
            dialogs.submit_prompt("ftp://nope"),
            Err("Please enter a valid URL (https://...)".to_string())
        );
        assert!(dialogs.prompt_state.open);

        assert_eq!(
            dialogs.submit_prompt("https://example.com/repo.git"),
            Ok(Some(PromptAnswer {
                confirmed: true,
                value: Some("https://example.com/repo.git".to_string())
            }))
        );
        assert_eq!(dialogs.prompt_state, PromptState::default());
    }

    #[test]
    fn prompts_with_defaults_and_can_be_dismissed() {
        let mut dialogs = DialogState::new();
        dialogs.prompt(&PromptOptions::default());

        let state = &dialogs.prompt_state;
        assert_eq!(state.title, "Enter a value");
        assert_eq!(state.input_error_message, "Invalid value");
        assert_eq!(state.rows, 3);
        assert!(!state.textarea);
        assert_eq!(state.pattern, None);

        assert_eq!(
            dialogs.dismiss_prompt(),
            Some(PromptAnswer {
                confirmed: false,
                value: None
            })
        );
        assert!(!dialogs.prompt_state.open);

        // Sin patrón cualquier valor vale, incluso vacío.
        dialogs.prompt(&PromptOptions {
            textarea: true,
            rows: Some(6),
            default_value: Some("WIP".to_string()),
            ..Default::default()
        });
        assert_eq!(dialogs.prompt_state.rows, 6);
        assert_eq!(dialogs.prompt_state.default_value, "WIP");
        assert_eq!(
            dialogs.submit_prompt(""),
            Ok(Some(PromptAnswer {
                confirmed: true,
                value: Some(String::new())
            }))
        );
    }

    #[test]
    fn serializes_the_state_with_the_field_names_the_dialog_reads() {
        let json = serde_json::to_value(DialogState::default()).unwrap();

        assert_eq!(
            json["confirmState"],
            serde_json::json!({
                "open": false,
                "title": "",
                "description": "",
                "confirmText": "Confirm",
                "cancelText": "Cancel",
                "dismissValue": false,
                "variant": "default",
                "optionLabel": "",
                "optionChecked": false
            })
        );
        assert_eq!(json["promptState"]["inputErrorMessage"], "");
        assert_eq!(json["promptState"]["rows"], 3);
        assert_eq!(json["promptState"]["pattern"], serde_json::Value::Null);
    }
}
