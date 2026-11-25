//! Atajos de teclado globales (`src/composables/useKeyboardShortcuts.js`): cómo
//! se escribe un combo, cuándo lo cumple una pulsación y qué atajo corresponde
//! a un evento.
//!
//! Dos guardas: un atajo no dispara mientras se escribe (inputs, textareas,
//! selects, contenteditable) ni con un diálogo modal abierto — los diálogos ya
//! tienen su propio teclado (Escape, Enter) y un atajo global por debajo sería
//! una sorpresa. Saber si el foco está en un campo editable o si hay un diálogo
//! abierto es cosa de la interfaz, que lo pasa en [`ShortcutContext`]
//! (`isEditableTarget` y `hasOpenDialog` consultaban el DOM).
//!
//! Combos: `mod+1`, `mod+shift+f`... donde `mod` es Cmd en macOS y Ctrl en el
//! resto.
//!
//! Las tablas [`APP_SHORTCUTS`] y [`WORKSPACE_SHORTCUTS`] son las que el
//! original registraba en `App.vue` y `RepositoryWorkspace.vue`, y
//! [`SHORTCUT_SECTIONS`] la chuleta de `ShortcutsDialog.vue`.

use crate::jsstr::utf16_len;

/// `IS_MAC` del original (allí salía de `navigator.platform`).
pub const fn is_mac_platform() -> bool {
    cfg!(target_os = "macos")
}

/// Lo que se mira de una pulsación.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyEvent {
    /// Nombre de la tecla como en `KeyboardEvent.key` (`f`, `Enter`, `,`); no
    /// importan las mayúsculas.
    pub key: String,
    pub meta_key: bool,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    /// Tecla mantenida: no dispara atajos.
    pub repeat: bool,
    /// Otro manejador ya se hizo cargo.
    pub default_prevented: bool,
}

impl KeyEvent {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ..Default::default()
        }
    }

    /// Con el modificador principal de la plataforma (Cmd en macOS, Ctrl en el resto).
    pub fn with_mod(mut self, is_mac: bool) -> Self {
        if is_mac {
            self.meta_key = true;
        } else {
            self.ctrl_key = true;
        }
        self
    }

    pub fn with_shift(mut self) -> Self {
        self.shift_key = true;
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.alt_key = true;
        self
    }
}

pub fn matches_combo(event: &KeyEvent, combo: &str, is_mac: bool) -> bool {
    let combo = combo.to_lowercase();
    let parts: Vec<&str> = combo.split('+').collect();
    let key = parts.last().copied().unwrap_or_default();
    let wants_mod = parts.contains(&"mod");
    // `ctrl` es Control en todas las plataformas (`ctrl+tab` también en macOS,
    // donde Cmd+Tab es del sistema).
    let wants_ctrl = parts.contains(&"ctrl");
    let wants_shift = parts.contains(&"shift");
    let wants_alt = parts.contains(&"alt");

    let (wants_meta, wants_control) = if is_mac {
        (wants_mod, wants_ctrl)
    } else {
        (false, wants_mod || wants_ctrl)
    };

    event.key.to_lowercase() == key
        && event.meta_key == wants_meta
        && event.ctrl_key == wants_control
        && event.shift_key == wants_shift
        && event.alt_key == wants_alt
}

/// Etiqueta visible de un combo según la plataforma ('mod+shift+f' → '⌘⇧F').
pub fn combo_label(combo: &str, is_mac: bool) -> String {
    combo
        .split('+')
        .map(|part| match part {
            "mod" => (if is_mac { "⌘" } else { "Ctrl" }).to_string(),
            "ctrl" => (if is_mac { "⌃" } else { "Ctrl" }).to_string(),
            "pageup" => "PgUp".to_string(),
            "pagedown" => "PgDn".to_string(),
            "shift" => (if is_mac { "⇧" } else { "Shift" }).to_string(),
            "alt" => (if is_mac { "⌥" } else { "Alt" }).to_string(),
            _ if utf16_len(part) == 1 => part.to_uppercase(),
            _ => {
                let mut characters = part.chars();
                match characters.next() {
                    Some(first) => first.to_uppercase().chain(characters).collect(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(if is_mac { "" } else { "+" })
}

/// Un atajo registrado: el combo y qué hace (`A` es la acción, que decide quien
/// lo registra).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutBinding<A = ShortcutAction> {
    pub combo: &'static str,
    pub action: A,
    /// Dispara también con un diálogo modal abierto.
    pub allow_in_dialogs: bool,
    /// Dispara también mientras se escribe en un campo.
    pub allow_in_editable: bool,
}

impl<A> ShortcutBinding<A> {
    pub const fn new(combo: &'static str, action: A) -> Self {
        Self {
            combo,
            action,
            allow_in_dialogs: false,
            allow_in_editable: false,
        }
    }

    /// Atajo que funciona en cualquier contexto (recargar, salir...).
    pub const fn everywhere(combo: &'static str, action: A) -> Self {
        Self {
            combo,
            action,
            allow_in_dialogs: true,
            allow_in_editable: true,
        }
    }

    pub const fn in_editable(mut self) -> Self {
        self.allow_in_editable = true;
        self
    }
}

/// Dónde ocurre la pulsación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutContext {
    /// El foco está en un campo de texto (input, textarea, select, contenteditable).
    pub editable_target: bool,
    /// Hay un diálogo modal abierto.
    pub dialog_open: bool,
    pub is_mac: bool,
}

impl Default for ShortcutContext {
    fn default() -> Self {
        Self {
            editable_target: false,
            dialog_open: false,
            is_mac: is_mac_platform(),
        }
    }
}

/// El primer atajo de `bindings` que corresponde a la pulsación en ese
/// contexto (`handleKeydown`). Quien lo recibe ejecuta su acción y da el
/// evento por consumido.
pub fn resolve_shortcut<'a, A>(
    event: &KeyEvent,
    bindings: &'a [ShortcutBinding<A>],
    context: ShortcutContext,
) -> Option<&'a ShortcutBinding<A>> {
    if event.default_prevented || event.repeat {
        return None;
    }

    bindings.iter().find(|binding| {
        (!context.editable_target || binding.allow_in_editable)
            && (!context.dialog_open || binding.allow_in_dialogs)
            && matches_combo(event, binding.combo, context.is_mac)
    })
}

/// Lo que hacen los atajos de la aplicación.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    OpenRepository,
    CloneFromGitHub,
    /// "Clone from URL": clonar pegando la URL de cualquier remoto.
    CloneFromUrl,
    Fetch,
    Pull,
    Push,
    NewBranch,
    ReloadApplication,
    Quit,
    MinimizeWindow,
    ToggleDevTools,
    ShowCommitsView,
    ShowChangesView,
    /// Va a la vista de commits y enfoca la búsqueda.
    SearchCommits,
    ShowOperationsLog,
    ShowReflog,
    /// Abre en el editor externo el archivo de la pestaña activa o, si no hay
    /// ninguna de archivo, el repositorio entero.
    OpenInEditor,
    OpenPreferences,
    /// La chuleta de atajos.
    OpenShortcutsHelp,
    /// Cierra el documento que se está viendo (o la pestaña de Settings).
    /// En Commits, Local Changes y Home no hace nada: el repositorio se
    /// cierra con [`ShortcutAction::CloseRepository`].
    CloseTab,
    /// Cierra la pestaña del repositorio activo aunque haya un archivo abierto.
    CloseRepository,
    /// Vuelve a abrir el último repositorio cerrado.
    ReopenClosedRepository,
    /// Siguiente pestaña de repositorio (vuelve a la primera tras la última).
    NextRepositoryTab,
    /// Pestaña de repositorio anterior.
    PreviousRepositoryTab,
    /// Siguiente documento abierto del repositorio (vuelve al primero).
    NextDocumentTab,
    /// Documento anterior.
    PreviousDocumentTab,
    /// La pestaña de repositorio N (1-8); 9 es siempre la última, como en los
    /// navegadores.
    SelectRepositoryTab(u8),
    /// La paleta de comandos (comandos, ramas, repositorios y archivos).
    OpenCommandPalette,
    /// Deshace la última operación de git del repositorio activo. Con el
    /// foco en un campo de texto el atajo es del campo.
    Undo,
    /// Rehace lo último deshecho.
    Redo,
}

/// Atajos globales (`App.vue`).
pub const APP_SHORTCUTS: &[ShortcutBinding] = &[
    ShortcutBinding::new("mod+o", ShortcutAction::OpenRepository),
    ShortcutBinding::new("mod+shift+c", ShortcutAction::CloneFromGitHub),
    ShortcutBinding::new("mod+shift+o", ShortcutAction::CloneFromUrl),
    ShortcutBinding::new("mod+shift+f", ShortcutAction::Fetch),
    // `mod+shift+p` es la paleta de comandos (como en VS Code y Zed).
    ShortcutBinding::new("mod+alt+p", ShortcutAction::Pull),
    ShortcutBinding::new("mod+shift+u", ShortcutAction::Push),
    ShortcutBinding::new("mod+shift+n", ShortcutAction::NewBranch),
    ShortcutBinding::everywhere("mod+r", ShortcutAction::ReloadApplication),
    ShortcutBinding::everywhere("mod+q", ShortcutAction::Quit),
    ShortcutBinding::everywhere("mod+m", ShortcutAction::MinimizeWindow),
    ShortcutBinding::everywhere("mod+shift+i", ShortcutAction::ToggleDevTools),
    // `mod+1`..`mod+9` son las pestañas de repositorio; las vistas fijas del
    // repositorio pasan a `alt+1` / `alt+2`.
    ShortcutBinding::new("alt+1", ShortcutAction::ShowCommitsView),
    ShortcutBinding::new("alt+2", ShortcutAction::ShowChangesView),
    ShortcutBinding::new("mod+f", ShortcutAction::SearchCommits),
    // El buscador de la barra (Señal) anuncia `mod+k`: lleva a la misma búsqueda.
    ShortcutBinding::new("mod+k", ShortcutAction::SearchCommits),
    ShortcutBinding::new("mod+shift+l", ShortcutAction::ShowOperationsLog),
    ShortcutBinding::new("mod+shift+r", ShortcutAction::ShowReflog),
    ShortcutBinding::new("mod+shift+e", ShortcutAction::OpenInEditor),
    ShortcutBinding::new("mod+,", ShortcutAction::OpenPreferences),
    ShortcutBinding::new("mod+/", ShortcutAction::OpenShortcutsHelp),
    ShortcutBinding::new("ctrl+tab", ShortcutAction::NextRepositoryTab).in_editable(),
    ShortcutBinding::new("ctrl+shift+tab", ShortcutAction::PreviousRepositoryTab).in_editable(),
    ShortcutBinding::new("mod+pagedown", ShortcutAction::NextRepositoryTab).in_editable(),
    ShortcutBinding::new("mod+pageup", ShortcutAction::PreviousRepositoryTab).in_editable(),
    ShortcutBinding::new("mod+1", ShortcutAction::SelectRepositoryTab(1)).in_editable(),
    ShortcutBinding::new("mod+2", ShortcutAction::SelectRepositoryTab(2)).in_editable(),
    ShortcutBinding::new("mod+3", ShortcutAction::SelectRepositoryTab(3)).in_editable(),
    ShortcutBinding::new("mod+4", ShortcutAction::SelectRepositoryTab(4)).in_editable(),
    ShortcutBinding::new("mod+5", ShortcutAction::SelectRepositoryTab(5)).in_editable(),
    ShortcutBinding::new("mod+6", ShortcutAction::SelectRepositoryTab(6)).in_editable(),
    ShortcutBinding::new("mod+7", ShortcutAction::SelectRepositoryTab(7)).in_editable(),
    ShortcutBinding::new("mod+8", ShortcutAction::SelectRepositoryTab(8)).in_editable(),
    ShortcutBinding::new("mod+9", ShortcutAction::SelectRepositoryTab(9)).in_editable(),
    ShortcutBinding::new("mod+shift+w", ShortcutAction::CloseRepository).in_editable(),
    ShortcutBinding::new("mod+shift+t", ShortcutAction::ReopenClosedRepository),
    ShortcutBinding::new("mod+shift+p", ShortcutAction::OpenCommandPalette).in_editable(),
    // Sin `in_editable`: en un campo de texto, deshacer es del campo (los
    // atajos globales se registran antes que `mergeon_ui::init` para que el
    // `Input` gane en sus propias teclas, y `run_now` además comprueba el foco).
    ShortcutBinding::new("mod+z", ShortcutAction::Undo),
    ShortcutBinding::new("mod+shift+z", ShortcutAction::Redo),
    ShortcutBinding::new("mod+y", ShortcutAction::Redo),
];

/// Atajos del espacio de trabajo de un repositorio, activos solo en la pestaña
/// de repositorio visible (`RepositoryWorkspace.vue`).
///
/// `mod+pageup` / `mod+pagedown` ya recorren los repositorios, así que los
/// documentos van con `alt+←` / `alt+→` (sin `in_editable`: en un campo de
/// texto esas teclas son del campo).
pub const WORKSPACE_SHORTCUTS: &[ShortcutBinding] = &[
    ShortcutBinding::new("mod+w", ShortcutAction::CloseTab).in_editable(),
    ShortcutBinding::new("alt+right", ShortcutAction::NextDocumentTab),
    ShortcutBinding::new("alt+left", ShortcutAction::PreviousDocumentTab),
];

/// Teclas de la lista de archivos de Local Changes, solo con el foco en ella.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangesListAction {
    /// Prepara el archivo elegido, o lo quita del stage si ya estaba.
    ToggleStage,
    /// Descarta los cambios del archivo elegido (tras confirmarlo).
    Discard,
    /// Archivo anterior / siguiente de la lista.
    Previous,
    Next,
}

/// Las de la lista de Local Changes: teclas sueltas, sin modificadores, que no
/// chocan con [`APP_SHORTCUTS`] ni [`WORKSPACE_SHORTCUTS`] (todos llevan un
/// modificador) y que la lista solo atiende mientras tiene el foco.
pub const CHANGES_LIST_SHORTCUTS: &[ShortcutBinding<ChangesListAction>] = &[
    ShortcutBinding::new("space", ChangesListAction::ToggleStage),
    ShortcutBinding::new("d", ChangesListAction::Discard),
    ShortcutBinding::new("up", ChangesListAction::Previous),
    ShortcutBinding::new("down", ChangesListAction::Next),
];

/// Teclas de los visores de código (diff y archivo), solo con el foco dentro
/// del visor (en su lienzo o en su barra de búsqueda).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewerAction {
    /// Abre (o enfoca) la barra de búsqueda del visor.
    Find,
    /// Siguiente / anterior bloque de cambios del diff.
    NextChange,
    PreviousChange,
}

/// Las de los visores. Regla: `mod+f` es "Search commits" en toda la app,
/// salvo con el foco en un visor de diff o de archivo, donde gana la búsqueda
/// dentro del visor (el binding del visor va en un contexto de teclado más
/// profundo, así que GPUI lo prueba antes). Es el único combo que se repite
/// con [`APP_SHORTCUTS`], a propósito.
pub const VIEWER_SHORTCUTS: &[ShortcutBinding<ViewerAction>] = &[
    ShortcutBinding::new("mod+f", ViewerAction::Find).in_editable(),
    ShortcutBinding::new("f7", ViewerAction::NextChange).in_editable(),
    ShortcutBinding::new("shift+f7", ViewerAction::PreviousChange).in_editable(),
    ShortcutBinding::new("alt+down", ViewerAction::NextChange).in_editable(),
    ShortcutBinding::new("alt+up", ViewerAction::PreviousChange).in_editable(),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutItem {
    pub label: &'static str,
    pub combo: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutSection {
    pub title: &'static str,
    pub items: &'static [ShortcutItem],
}

const fn item(label: &'static str, combo: &'static str) -> ShortcutItem {
    ShortcutItem { label, combo }
}

/// La chuleta de atajos (`ShortcutsDialog.vue`). El combo se pinta con [`combo_label`].
pub const SHORTCUT_SECTIONS: &[ShortcutSection] = &[
    ShortcutSection {
        title: "Repository",
        items: &[
            item("Open repository", "mod+o"),
            item("Clone from GitHub", "mod+shift+c"),
            item("Clone from URL", "mod+shift+o"),
            item("Fetch", "mod+shift+f"),
            item("Pull", "mod+alt+p"),
            item("Push", "mod+shift+u"),
            item("New branch", "mod+shift+n"),
            item("Reflog", "mod+shift+r"),
            item("Open in editor", "mod+shift+e"),
        ],
    },
    ShortcutSection {
        title: "Navigation",
        items: &[
            item("Command palette", "mod+shift+p"),
            item("Commits view", "alt+1"),
            item("Changes view", "alt+2"),
            item("Search commits", "mod+f"),
            item("Previous / next commit", "↑ / ↓"),
            item("Operation log", "mod+shift+l"),
            item("Next repository tab", "ctrl+tab"),
            item("Previous repository tab", "ctrl+shift+tab"),
            item("Repository tab N (9 = last)", "mod+1…9"),
            item("Next document", "alt+right"),
            item("Previous document", "alt+left"),
            item("Close document", "mod+w"),
            item("Close repository", "mod+shift+w"),
            item("Reopen closed repository", "mod+shift+t"),
            item("Reload application", "mod+r"),
            item("Minimize window", "mod+m"),
            item("Exit Mergeon", "mod+q"),
        ],
    },
    ShortcutSection {
        title: "Editing",
        items: &[
            item("Undo last Git operation", "mod+z"),
            item("Redo Git operation", "mod+shift+z"),
            item("Redo Git operation (alternative)", "mod+y"),
            item("Commit staged changes", "mod+enter"),
            item("Preferences", "mod+,"),
            item("This cheatsheet", "mod+/"),
        ],
    },
    ShortcutSection {
        title: "Local Changes",
        items: &[
            item("Stage / unstage file", "space"),
            item("Discard file", "d"),
            item("Previous / next file", "↑ / ↓"),
        ],
    },
    ShortcutSection {
        title: "Diff and file viewer",
        items: &[
            item("Find in the diff or file", "mod+f"),
            item("Next change", "f7"),
            item("Previous change", "shift+f7"),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn with_wrong_mod(mut event: KeyEvent, is_mac: bool) -> KeyEvent {
        if is_mac {
            event.ctrl_key = true;
        } else {
            event.meta_key = true;
        }
        event
    }

    // El original solo podía probar la plataforma en la que corría; aquí van las dos.
    #[test]
    fn matches_mod_combos_on_the_current_platform_only() {
        for is_mac in [false, true] {
            assert!(matches_combo(
                &KeyEvent::new("1").with_mod(is_mac),
                "mod+1",
                is_mac
            ));
            assert!(!matches_combo(&KeyEvent::new("1"), "mod+1", is_mac));
            // El modificador de la otra plataforma no cuenta como mod
            assert!(!matches_combo(
                &with_wrong_mod(KeyEvent::new("1"), is_mac),
                "mod+1",
                is_mac
            ));
            // ...y estorba aunque el bueno también esté pulsado.
            assert!(!matches_combo(
                &with_wrong_mod(KeyEvent::new("1").with_mod(is_mac), is_mac),
                "mod+1",
                is_mac
            ));
        }
    }

    #[test]
    fn requires_exact_shift_state() {
        for is_mac in [false, true] {
            let plain = KeyEvent::new("l").with_mod(is_mac);
            let shifted = KeyEvent::new("l").with_mod(is_mac).with_shift();

            assert!(matches_combo(&shifted, "mod+shift+l", is_mac));
            assert!(!matches_combo(&plain, "mod+shift+l", is_mac));
            assert!(!matches_combo(&shifted, "mod+l", is_mac));
        }
    }

    #[test]
    fn compares_keys_without_caring_about_case() {
        // Con Shift pulsado el navegador entrega la letra en mayúscula.
        assert!(matches_combo(
            &KeyEvent::new("F").with_mod(false).with_shift(),
            "mod+shift+f",
            false
        ));
        assert!(matches_combo(
            &KeyEvent::new("Enter").with_mod(false),
            "mod+enter",
            false
        ));
        assert!(matches_combo(
            &KeyEvent::new(",").with_mod(false),
            "mod+,",
            false
        ));
        assert!(!matches_combo(
            &KeyEvent::new("f").with_mod(false).with_alt(),
            "mod+f",
            false
        ));
        assert!(matches_combo(
            &KeyEvent::new("f").with_mod(false).with_alt(),
            "mod+alt+f",
            false
        ));
        assert!(matches_combo(&KeyEvent::new("Escape"), "escape", false));
    }

    // `isEditableTarget` y `hasOpenDialog` miraban el DOM; lo que decidían con
    // esa respuesta es esto.
    #[test]
    fn stays_quiet_while_typing_or_behind_a_dialog_unless_the_shortcut_allows_it() {
        let context = |editable_target, dialog_open| ShortcutContext {
            editable_target,
            dialog_open,
            is_mac: false,
        };
        let action = |event: &KeyEvent, context| {
            resolve_shortcut(event, APP_SHORTCUTS, context).map(|binding| binding.action)
        };
        let fetch = KeyEvent::new("f").with_mod(false).with_shift();
        let reload = KeyEvent::new("r").with_mod(false);

        assert_eq!(
            action(&fetch, context(false, false)),
            Some(ShortcutAction::Fetch)
        );
        assert_eq!(action(&fetch, context(true, false)), None);
        assert_eq!(action(&fetch, context(false, true)), None);
        assert_eq!(
            action(&reload, context(true, true)),
            Some(ShortcutAction::ReloadApplication)
        );

        // Cerrar pestaña funciona escribiendo, pero no por debajo de un diálogo.
        let close = KeyEvent::new("w").with_mod(false);
        let close_action = |context| {
            resolve_shortcut(&close, WORKSPACE_SHORTCUTS, context).map(|binding| binding.action)
        };
        assert_eq!(
            close_action(context(true, false)),
            Some(ShortcutAction::CloseTab)
        );
        assert_eq!(close_action(context(false, true)), None);
    }

    #[test]
    fn ignores_repeats_and_events_someone_else_already_handled() {
        let context = ShortcutContext {
            is_mac: false,
            ..Default::default()
        };
        let held = KeyEvent {
            repeat: true,
            ..KeyEvent::new("o").with_mod(false)
        };
        let handled = KeyEvent {
            default_prevented: true,
            ..KeyEvent::new("o").with_mod(false)
        };

        assert!(
            resolve_shortcut(&KeyEvent::new("o").with_mod(false), APP_SHORTCUTS, context).is_some()
        );
        assert_eq!(resolve_shortcut(&held, APP_SHORTCUTS, context), None);
        assert_eq!(resolve_shortcut(&handled, APP_SHORTCUTS, context), None);
        assert_eq!(
            resolve_shortcut(&KeyEvent::new("o"), APP_SHORTCUTS, context),
            None
        );
    }

    #[test]
    fn labels_combos_for_each_platform() {
        assert_eq!(combo_label("mod+shift+f", true), "⌘⇧F");
        assert_eq!(combo_label("mod+shift+f", false), "Ctrl+Shift+F");
        assert_eq!(combo_label("mod+alt+enter", true), "⌘⌥Enter");
        assert_eq!(combo_label("mod+enter", false), "Ctrl+Enter");
        assert_eq!(combo_label("mod+,", false), "Ctrl+,");
        assert_eq!(combo_label("mod+/", true), "⌘/");
        // Lo que no es un combo se deja como está.
        assert_eq!(combo_label("↑ / ↓", false), "↑ / ↓");
        assert_eq!(combo_label("", false), "");
    }

    #[test]
    fn registers_each_combo_once_and_documents_the_ones_it_lists() {
        let combos: HashSet<&str> = APP_SHORTCUTS
            .iter()
            .chain(WORKSPACE_SHORTCUTS)
            .map(|binding| binding.combo)
            .collect();
        assert_eq!(
            combos.len(),
            APP_SHORTCUTS.len() + WORKSPACE_SHORTCUTS.len()
        );
        assert_eq!(APP_SHORTCUTS.len(), 39);

        // Todo lo que anuncia la chuleta existe, salvo lo que atiende cada vista
        // por su cuenta (flechas del historial y confirmar el commit).
        let local_to_a_view = [
            "↑ / ↓",
            "mod+enter",
            "mod+1…9",
            "space",
            "d",
            "f7",
            "shift+f7",
        ];
        for section in SHORTCUT_SECTIONS {
            for item in section.items {
                assert!(
                    combos.contains(item.combo) || local_to_a_view.contains(&item.combo),
                    "{} ({}) is not registered",
                    item.label,
                    item.combo
                );
            }
        }
        assert_eq!(
            SHORTCUT_SECTIONS
                .iter()
                .map(|section| section.title)
                .collect::<Vec<_>>(),
            [
                "Repository",
                "Navigation",
                "Editing",
                "Local Changes",
                "Diff and file viewer"
            ]
        );
        assert_eq!(
            SHORTCUT_SECTIONS
                .iter()
                .map(|section| section.items.len())
                .sum::<usize>(),
            38
        );
    }

    #[test]
    fn documents_are_walked_with_alt_arrows_and_mod_w_only_closes_a_document() {
        let context = ShortcutContext {
            editable_target: false,
            dialog_open: false,
            is_mac: false,
        };
        let action = |event: &KeyEvent| {
            resolve_shortcut(event, WORKSPACE_SHORTCUTS, context).map(|binding| binding.action)
        };
        assert_eq!(
            action(&KeyEvent::new("right").with_alt()),
            Some(ShortcutAction::NextDocumentTab)
        );
        assert_eq!(
            action(&KeyEvent::new("left").with_alt()),
            Some(ShortcutAction::PreviousDocumentTab)
        );
        assert_eq!(
            action(&KeyEvent::new("w").with_mod(false)),
            Some(ShortcutAction::CloseTab)
        );
        // Escribiendo, las flechas son del campo.
        let typing = ShortcutContext {
            editable_target: true,
            ..context
        };
        assert_eq!(
            resolve_shortcut(
                &KeyEvent::new("right").with_alt(),
                WORKSPACE_SHORTCUTS,
                typing
            ),
            None
        );
        assert_eq!(combo_label("alt+right", false), "Alt+Right");
    }

    #[test]
    fn the_changes_list_keys_do_not_collide_with_any_global_shortcut() {
        let global: HashSet<String> = APP_SHORTCUTS
            .iter()
            .chain(WORKSPACE_SHORTCUTS)
            .map(|binding| binding.combo.to_lowercase())
            .collect();
        let mut seen = HashSet::new();
        for binding in CHANGES_LIST_SHORTCUTS {
            assert!(!global.contains(binding.combo), "{}", binding.combo);
            assert!(seen.insert(binding.combo), "{} twice", binding.combo);
            // Teclas sueltas: con el foco en un campo de texto son texto.
            assert!(!binding.allow_in_editable && !binding.combo.contains('+'));
        }

        let context = ShortcutContext {
            editable_target: false,
            dialog_open: false,
            is_mac: false,
        };
        let action = |event: KeyEvent| {
            resolve_shortcut(&event, CHANGES_LIST_SHORTCUTS, context).map(|binding| binding.action)
        };
        assert_eq!(
            action(KeyEvent::new("space")),
            Some(ChangesListAction::ToggleStage)
        );
        assert_eq!(action(KeyEvent::new("D")), Some(ChangesListAction::Discard));
        assert_eq!(
            action(KeyEvent::new("up")),
            Some(ChangesListAction::Previous)
        );
        assert_eq!(action(KeyEvent::new("down")), Some(ChangesListAction::Next));
        // Con modificador ya no es la tecla de la lista.
        assert_eq!(action(KeyEvent::new("d").with_mod(false)), None);
        assert_eq!(action(KeyEvent::new("d").with_shift()), None);
        assert_eq!(combo_label("space", false), "Space");
        assert_eq!(combo_label("d", true), "D");
    }

    #[test]
    fn the_command_palette_takes_mod_shift_p_and_pull_moves_to_mod_alt_p() {
        for is_mac in [false, true] {
            let context = ShortcutContext {
                is_mac,
                ..Default::default()
            };
            let action = |event: &KeyEvent| {
                resolve_shortcut(event, APP_SHORTCUTS, context).map(|binding| binding.action)
            };
            assert_eq!(
                action(&KeyEvent::new("p").with_mod(is_mac).with_shift()),
                Some(ShortcutAction::OpenCommandPalette)
            );
            assert_eq!(
                action(&KeyEvent::new("p").with_mod(is_mac).with_alt()),
                Some(ShortcutAction::Pull)
            );
        }
        assert_eq!(combo_label("mod+shift+p", true), "⌘⇧P");
        assert_eq!(combo_label("mod+alt+p", false), "Ctrl+Alt+P");
    }

    /// Los visores solo comparten `mod+f` con los globales (la búsqueda
    /// local gana con el foco en el visor); el resto no choca con nada.
    #[test]
    fn the_viewer_keys_only_shadow_ctrl_f() {
        let global: HashSet<&str> = APP_SHORTCUTS
            .iter()
            .chain(WORKSPACE_SHORTCUTS)
            .map(|binding| binding.combo)
            .chain(CHANGES_LIST_SHORTCUTS.iter().map(|binding| binding.combo))
            .collect();
        let mut seen = HashSet::new();
        for binding in VIEWER_SHORTCUTS {
            assert!(seen.insert(binding.combo), "{} twice", binding.combo);
            if binding.action == ViewerAction::Find {
                assert_eq!(binding.combo, "mod+f");
                assert!(global.contains(binding.combo));
            } else {
                assert!(!global.contains(binding.combo), "{}", binding.combo);
            }
        }
        assert_eq!(combo_label("shift+f7", false), "Shift+F7");
        assert_eq!(combo_label("alt+down", true), "⌥Down");
    }

    #[test]
    fn navigates_repository_tabs_from_the_keyboard_even_while_typing() {
        for is_mac in [false, true] {
            let context = ShortcutContext {
                editable_target: true,
                dialog_open: false,
                is_mac,
            };
            let action = |event: &KeyEvent| {
                resolve_shortcut(event, APP_SHORTCUTS, context).map(|binding| binding.action)
            };
            let mut control_tab = KeyEvent::new("tab");
            control_tab.ctrl_key = true;

            assert_eq!(
                action(&control_tab),
                Some(ShortcutAction::NextRepositoryTab)
            );
            assert_eq!(
                action(&control_tab.clone().with_shift()),
                Some(ShortcutAction::PreviousRepositoryTab)
            );
            assert_eq!(
                action(&KeyEvent::new("9").with_mod(is_mac)),
                Some(ShortcutAction::SelectRepositoryTab(9))
            );
            assert_eq!(
                action(&KeyEvent::new("w").with_mod(is_mac).with_shift()),
                Some(ShortcutAction::CloseRepository)
            );
        }

        let not_typing = ShortcutContext {
            is_mac: false,
            ..Default::default()
        };
        assert_eq!(
            resolve_shortcut(&KeyEvent::new("1").with_alt(), APP_SHORTCUTS, not_typing)
                .map(|binding| binding.action),
            Some(ShortcutAction::ShowCommitsView)
        );
        assert_eq!(combo_label("ctrl+tab", true), "⌃Tab");
        assert_eq!(combo_label("ctrl+shift+tab", false), "Ctrl+Shift+Tab");
        assert_eq!(combo_label("mod+pagedown", false), "Ctrl+PgDn");
        assert_eq!(combo_label("mod+1…9", false), "Ctrl+1…9");
    }

    #[test]
    fn lets_a_view_register_its_own_actions() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Local {
            Save,
        }
        let bindings = [ShortcutBinding::new("mod+s", Local::Save).in_editable()];
        let context = ShortcutContext {
            editable_target: true,
            dialog_open: false,
            is_mac: true,
        };

        let found = resolve_shortcut(&KeyEvent::new("s").with_mod(true), &bindings, context);

        assert_eq!(found.map(|binding| binding.action), Some(Local::Save));
        assert_eq!(ShortcutContext::default().is_mac, is_mac_platform());
    }
}
