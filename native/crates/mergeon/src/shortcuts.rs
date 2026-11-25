//! Atajos de teclado globales (`useKeyboardShortcuts` de `App.vue` y
//! `RepositoryWorkspace.vue`). Las tablas viven en `mergeon_model`; aquí se
//! traducen a bindings de GPUI y se aplican las mismas guardas del original
//! (diálogo abierto, foco en un campo de texto).

use std::collections::BTreeMap;

use gpui_kit::{App, KeyBinding, Keystroke, Window, actions};
use mergeon_app::AppModel;
use mergeon_model::keyboard_shortcuts::{
    APP_SHORTCUTS, ShortcutAction, ShortcutBinding, WORKSPACE_SHORTCUTS, effective_combos_for,
};

actions!(
    mergeon,
    [
        OpenRepository,
        CloneFromGitHub,
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
        SearchCommits,
        ShowOperationsLog,
        ShowReflog,
        OpenInEditor,
        OpenPreferences,
        OpenShortcutsHelp,
        CloseTab,
        CloseRepository,
        ReopenClosedRepository,
        NextRepositoryTab,
        PreviousRepositoryTab,
        NextDocumentTab,
        PreviousDocumentTab,
        SelectRepositoryTab1,
        SelectRepositoryTab2,
        SelectRepositoryTab3,
        SelectRepositoryTab4,
        SelectRepositoryTab5,
        SelectRepositoryTab6,
        SelectRepositoryTab7,
        SelectRepositoryTab8,
        SelectRepositoryTab9,
        OpenCommandPalette,
        UndoGitOperation,
        RedoGitOperation,
    ]
);

/// `mod+shift+c` → `secondary-shift-c` (`secondary` es Cmd en macOS y Ctrl en el resto).
fn keystroke(combo: &str) -> String {
    combo
        .split('+')
        .map(|part| if part == "mod" { "secondary" } else { part })
        .collect::<Vec<_>>()
        .join("-")
}

/// Todos los combos de una acción (`ctrl+tab` y `mod+pagedown`, por ejemplo).
fn bindings_for(action: ShortcutAction) -> impl Iterator<Item = &'static ShortcutBinding> {
    APP_SHORTCUTS
        .iter()
        .chain(WORKSPACE_SHORTCUTS.iter())
        .filter(move |binding| binding.action == action)
}

fn binding_for(action: ShortcutAction) -> Option<&'static ShortcutBinding> {
    bindings_for(action).next()
}

fn focus_is_editable(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|context| context.contains("Input"))
}

fn focus_is_shortcut_recorder(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|context| context.contains(mergeon_view_home::SHORTCUT_RECORDER_CONTEXT))
}

fn run(action: ShortcutAction, cx: &mut App) {
    log::debug!("atajo {action:?}");
    // La acción llega desde dentro del despacho de la pulsación, con la
    // ventana prestada: GPUI la saca de `cx.windows` mientras la actualiza
    // (`App::update_window_erased`), así que `window.update` aquí devolvía
    // "window not found", el `unwrap_or(false)` lo tomaba por "no permitido"
    // y ningún atajo hacía nada, en cualquier plataforma. Las guardas se
    // miran justo después, cuando la ventana ya está de vuelta.
    cx.defer(move |cx| run_now(action, cx));
}

fn run_now(action: ShortcutAction, cx: &mut App) {
    let Some(binding) = binding_for(action) else {
        return;
    };
    let Some(window) = cx
        .active_window()
        .or_else(|| cx.windows().into_iter().next())
    else {
        log::debug!("atajo {action:?} sin ventana");
        return;
    };
    let allowed = window.update(cx, |_, window, cx| {
        if focus_is_shortcut_recorder(window) {
            return false;
        }
        let dialog_open = mergeon_ui::dialog::open_count(window, cx) > 0;
        let editable = focus_is_editable(window);
        log::debug!("atajo {action:?}: diálogo={dialog_open} editable={editable}");
        (binding.allow_in_dialogs || !dialog_open) && (binding.allow_in_editable || !editable)
    });
    match allowed {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            log::warn!("atajo {action:?} descartado: {error}");
            return;
        }
    }
    // La paleta es de la cáscara, no del modelo.
    if action == ShortcutAction::OpenCommandPalette {
        let shell = cx
            .try_global::<crate::shell::ShellHandle>()
            .and_then(|handle| handle.0.upgrade());
        if let Some(shell) = shell {
            let opened = window.update(cx, |_, window, cx| {
                shell.update(cx, |shell, cx| shell.open_command_palette(window, cx));
            });
            if let Err(error) = opened {
                log::warn!("paleta de comandos: {error}");
            }
        }
        return;
    }
    if let Some(model) = AppModel::try_global(cx) {
        model.update(cx, |model, cx| model.handle_shortcut(action, cx));
    }
}

macro_rules! for_all_shortcuts {
    ($macro:ident, $cx:expr $(, $extra:expr)*) => {
        $macro!($cx $(, $extra)*;
            OpenRepository => ShortcutAction::OpenRepository,
            CloneFromGitHub => ShortcutAction::CloneFromGitHub,
            CloneFromUrl => ShortcutAction::CloneFromUrl,
            Fetch => ShortcutAction::Fetch,
            Pull => ShortcutAction::Pull,
            Push => ShortcutAction::Push,
            NewBranch => ShortcutAction::NewBranch,
            ReloadApplication => ShortcutAction::ReloadApplication,
            Quit => ShortcutAction::Quit,
            MinimizeWindow => ShortcutAction::MinimizeWindow,
            ToggleDevTools => ShortcutAction::ToggleDevTools,
            ShowCommitsView => ShortcutAction::ShowCommitsView,
            ShowChangesView => ShortcutAction::ShowChangesView,
            SearchCommits => ShortcutAction::SearchCommits,
            ShowOperationsLog => ShortcutAction::ShowOperationsLog,
            ShowReflog => ShortcutAction::ShowReflog,
            OpenInEditor => ShortcutAction::OpenInEditor,
            OpenPreferences => ShortcutAction::OpenPreferences,
            OpenShortcutsHelp => ShortcutAction::OpenShortcutsHelp,
            CloseTab => ShortcutAction::CloseTab,
            CloseRepository => ShortcutAction::CloseRepository,
            ReopenClosedRepository => ShortcutAction::ReopenClosedRepository,
            NextRepositoryTab => ShortcutAction::NextRepositoryTab,
            PreviousRepositoryTab => ShortcutAction::PreviousRepositoryTab,
            NextDocumentTab => ShortcutAction::NextDocumentTab,
            PreviousDocumentTab => ShortcutAction::PreviousDocumentTab,
            SelectRepositoryTab1 => ShortcutAction::SelectRepositoryTab(1),
            SelectRepositoryTab2 => ShortcutAction::SelectRepositoryTab(2),
            SelectRepositoryTab3 => ShortcutAction::SelectRepositoryTab(3),
            SelectRepositoryTab4 => ShortcutAction::SelectRepositoryTab(4),
            SelectRepositoryTab5 => ShortcutAction::SelectRepositoryTab(5),
            SelectRepositoryTab6 => ShortcutAction::SelectRepositoryTab(6),
            SelectRepositoryTab7 => ShortcutAction::SelectRepositoryTab(7),
            SelectRepositoryTab8 => ShortcutAction::SelectRepositoryTab(8),
            SelectRepositoryTab9 => ShortcutAction::SelectRepositoryTab(9),
            OpenCommandPalette => ShortcutAction::OpenCommandPalette,
            UndoGitOperation => ShortcutAction::Undo,
            RedoGitOperation => ShortcutAction::Redo,
        )
    };
}

macro_rules! register_handlers {
    ($cx:expr; $( $gpui_action:ident => $action:expr ),+ $(,)?) => {{
        $(
            $cx.on_action(|_: &$gpui_action, cx| run($action, cx));
        )+
    }};
}

macro_rules! bind_effective_keys {
    ($cx:expr, $overrides:expr; $( $gpui_action:ident => $action:expr ),+ $(,)?) => {{
        let mut bindings = Vec::new();
        $(
            for combo in effective_combos_for($action, $overrides) {
                let ks = keystroke(&combo);
                if Keystroke::parse(&ks).is_ok() {
                    // Sin contexto (`None`) para que funcionen incluso sin foco en
                    // la ventana; `init_bindings` / `rebind_shortcuts` llaman a
                    // `shortcuts::bind_keys` antes que `mergeon_ui::init` y las
                    // vistas para que los contextos específicos (`Input`,
                    // `DiffViewer`, `CommitMessage`…) ganen en el desempate.
                    bindings.push(KeyBinding::new(&ks, $gpui_action, None));
                } else {
                    log::warn!("atajo inválido ignorado para {:?}: {:?}", $action, combo);
                }
            }
        )+
        $cx.bind_keys(bindings);
    }};
}

/// Registra los manejadores de acciones globales una vez y aplica los bindings
/// por defecto.
pub fn init(cx: &mut App) {
    // `RUST_LOG=mergeon=trace`: cada pulsación, con la acción que resolvió y
    // el contexto de foco (para depurar atajos que "no hacen nada").
    if log::log_enabled!(log::Level::Trace) {
        cx.observe_keystrokes(|event, _, _| {
            log::trace!(
                "tecla {:?} → {:?} en {:?}",
                event.keystroke,
                event.action.as_ref().map(|action| action.name()),
                event.context_stack
            );
        })
        .detach();
    }
    for_all_shortcuts!(register_handlers, cx);
    bind_keys(cx, &BTreeMap::new());
}

/// Registra únicamente los `KeyBinding` globales aplicando los overrides del
/// usuario (tras `cx.clear_key_bindings()` en caliente).
pub fn bind_keys(cx: &mut App, overrides: &BTreeMap<String, String>) {
    for_all_shortcuts!(bind_effective_keys, cx, overrides);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_combos_to_gpui_keystrokes() {
        assert_eq!(keystroke("mod+o"), "secondary-o");
        assert_eq!(keystroke("mod+shift+c"), "secondary-shift-c");
        assert_eq!(keystroke("mod+,"), "secondary-,");
        assert_eq!(keystroke("ctrl+shift+tab"), "ctrl-shift-tab");
        assert_eq!(keystroke("alt+1"), "alt-1");
        assert_eq!(keystroke("alt+right"), "alt-right");
    }

    /// Una ventana sin pantalla con los atajos registrados y el modelo sobre
    /// un backend de prueba.
    fn window_with_shortcuts(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (gpui_kit::Entity<AppModel>, gpui_kit::AnyWindowHandle) {
        use gpui_kit::{AppContext as _, WindowOptions, div};
        struct Empty;
        impl gpui_kit::Render for Empty {
            fn render(
                &mut self,
                _: &mut Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                div()
            }
        }

        cx.executor().allow_parking();
        let (backend, events) = mergeon_app::testing::TestBackend::new().build();
        let model = cx.update(|cx| {
            crate::init_bindings(cx);
            AppModel::init(backend, events, cx)
        });
        let window = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Empty))
                .expect("ventana de prueba")
                .0
        });
        (model, window)
    }

    // Antes: la acción llegaba con la ventana prestada por GPUI, `window.update`
    // fallaba ("window not found") y el atajo se descartaba sin decir nada:
    // ningún atajo funcionaba, tampoco fuera de Xvfb.
    #[gpui_kit::test]
    async fn a_shortcut_typed_in_the_window_reaches_the_model(cx: &mut gpui_kit::TestAppContext) {
        let (model, window) = window_with_shortcuts(cx);
        assert!(!model.read_with(cx, |model, _| model.show_preferences()));

        cx.simulate_keystrokes(window, "secondary-,");
        cx.run_until_parked();

        assert!(model.read_with(cx, |model, _| model.show_preferences()));
        assert_eq!(
            model.read_with(cx, |model, _| model.settings_section().to_string()),
            "appearance"
        );
    }

    /// El buscador de la barra anuncia Ctrl+K (⌘K): abre la misma búsqueda
    /// de commits que Ctrl+F.
    #[gpui_kit::test]
    async fn ctrl_k_opens_the_commit_search(cx: &mut gpui_kit::TestAppContext) {
        let (model, window) = window_with_shortcuts(cx);
        let focused = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            let focused = focused.clone();
            cx.subscribe(&model, move |_, event: &mergeon_app::AppEvent, _| {
                if matches!(event, mergeon_app::AppEvent::FocusCommitSearch) {
                    focused.set(focused.get() + 1);
                }
            })
            .detach();
        });

        cx.simulate_keystrokes(window, "secondary-k");
        cx.run_until_parked();

        assert_eq!(focused.get(), 1);
    }

    /// Un repositorio con un commit, abierto en el modelo, y un commit más
    /// hecho por la app (en la pila de deshacer). Devuelve (dir, ruta, HEAD
    /// antes, HEAD después).
    async fn repository_with_an_undoable_commit(
        model: &gpui_kit::Entity<AppModel>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (tempfile::TempDir, String, String, String) {
        let dir = tempfile::Builder::new()
            .prefix("undo-keys")
            .tempdir()
            .unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
                .args([
                    "-c",
                    "init.defaultBranch=main",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(root)
                .output()
                .expect("git");
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("README.md"), "hola\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        let before = git(&["rev-parse", "HEAD"]);
        let path = root.to_string_lossy().into_owned();
        model
            .update(cx, |model, cx| {
                model.add_repository(&path, mergeon_app::AddRepository::default(), cx)
            })
            .await
            .expect("add repository");
        cx.run_until_parked();
        let path = model.read_with(cx, |model, _| {
            model.active_repository().unwrap().path.clone()
        });
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&["add", "a.txt"]);
        let result = model
            .update(cx, |model, cx| {
                model.commit(&path, "Undo me", Default::default(), true, cx)
            })
            .await;
        assert!(result.success(), "{}", result.message());
        cx.run_until_parked();
        let after = git(&["rev-parse", "HEAD"]);
        (dir, path, before, after)
    }

    fn head_of(dir: &tempfile::TempDir) -> String {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir.path())
            .output()
            .expect("git");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn wait(cx: &mut gpui_kit::TestAppContext, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            if done() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Ctrl+Z / Ctrl+Shift+Z (⌘Z / ⌘⇧Z) deshacen y rehacen la última
    /// operación de git del repositorio activo.
    #[gpui_kit::test]
    async fn ctrl_z_undoes_and_ctrl_shift_z_redoes_the_last_git_operation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (model, window) = window_with_shortcuts(cx);
        let (dir, _path, before, after) = repository_with_an_undoable_commit(&model, cx).await;

        cx.simulate_keystrokes(window, "secondary-z");
        wait(cx, "el deshacer", || head_of(&dir) == before);
        cx.simulate_keystrokes(window, "secondary-shift-z");
        wait(cx, "el rehacer", || head_of(&dir) == after);
        cx.simulate_keystrokes(window, "secondary-z");
        wait(cx, "el deshacer", || head_of(&dir) == before);
        // Ctrl+Y también rehace (⌘Y en macOS).
        cx.simulate_keystrokes(window, "secondary-y");
        wait(cx, "el rehacer", || head_of(&dir) == after);
    }

    /// Con el foco en un campo de texto el atajo es del campo: no deshace
    /// nada de git.
    #[gpui_kit::test]
    async fn ctrl_z_in_a_text_field_belongs_to_the_field(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, WindowOptions};
        use mergeon_ui::InputState;

        struct WithInput {
            input: gpui_kit::Entity<InputState>,
        }
        impl gpui_kit::Render for WithInput {
            fn render(
                &mut self,
                _: &mut Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                mergeon_ui::input(&self.input)
            }
        }

        cx.executor().allow_parking();
        let (backend, events) = mergeon_app::testing::TestBackend::new().build();
        let model = cx.update(|cx| {
            crate::init_bindings(cx);
            AppModel::init(backend, events, cx)
        });
        let (window, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx));
                cx.new(|_| WithInput { input })
            })
            .expect("ventana de prueba")
        });
        let (dir, path, _before, after) = repository_with_an_undoable_commit(&model, cx).await;
        cx.update_window(window, |_, window, cx| {
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| input.focus(window, cx));
        })
        .unwrap();
        cx.run_until_parked();

        // Escribir en el campo y pulsar Ctrl+Z / Ctrl+Y (⌘Z / ⌘⇧Z en macOS):
        // el texto del campo se deshace y rehace, mientras git no se toca.
        cx.simulate_keystrokes(window, "a b c");
        cx.run_until_parked();
        let value = |cx: &mut gpui_kit::TestAppContext| {
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string())
        };
        assert_eq!(value(cx), "abc");

        cx.simulate_keystrokes(window, "secondary-z");
        cx.run_until_parked();
        assert_eq!(value(cx), "");

        let redo_keys = if cfg!(target_os = "macos") {
            "secondary-shift-z"
        } else {
            "secondary-y"
        };
        cx.simulate_keystrokes(window, redo_keys);
        cx.run_until_parked();
        assert_eq!(value(cx), "abc");

        // `secondary-shift-z` en Linux/Windows tampoco dispara el rehacer de git.
        cx.simulate_keystrokes(window, "secondary-shift-z");
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(head_of(&dir), after);
        assert!(model.read_with(cx, |model, _| {
            model.undo_action_label(&path, false).is_some()
        }));
        assert!(model.read_with(cx, |model, _| !model.is_undo_running(&path)));
    }

    #[test]
    fn search_commits_has_ctrl_f_and_ctrl_k() {
        let combos: Vec<&str> = bindings_for(ShortcutAction::SearchCommits)
            .map(|binding| binding.combo)
            .collect();
        assert_eq!(combos, ["mod+f", "mod+k"]);
    }

    #[test]
    fn binds_every_combo_of_an_action() {
        let combos: Vec<&str> = bindings_for(ShortcutAction::NextRepositoryTab)
            .map(|binding| binding.combo)
            .collect();
        assert_eq!(combos, ["ctrl+tab", "mod+pagedown"]);
    }

    #[test]
    fn every_shortcut_action_has_a_binding() {
        for binding in APP_SHORTCUTS.iter().chain(WORKSPACE_SHORTCUTS.iter()) {
            assert!(binding_for(binding.action).is_some());
        }
    }

    #[gpui_kit::test]
    async fn live_rebinding_applies_custom_shortcuts_and_preserves_input_precedence(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext as _, WindowOptions};
        use mergeon_ui::InputState;

        struct WithInput {
            input: gpui_kit::Entity<InputState>,
        }
        impl gpui_kit::Render for WithInput {
            fn render(
                &mut self,
                _: &mut Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                mergeon_ui::input(&self.input)
            }
        }

        cx.executor().allow_parking();
        let (backend, events) = mergeon_app::testing::TestBackend::new().build();
        let model = cx.update(|cx| {
            crate::init_bindings(cx);
            AppModel::init(backend, events, cx)
        });
        let (window, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx));
                cx.new(|_| WithInput { input })
            })
            .expect("ventana de prueba")
        });

        // Rebind OpenPreferences from `mod+,` to `f9` live at runtime.
        let mut overrides = BTreeMap::new();
        overrides.insert("openPreferences".to_string(), "f9".to_string());
        cx.update(|cx| crate::rebind_shortcuts(&overrides, cx));

        // Old shortcut (`secondary-,`) no longer triggers OpenPreferences.
        cx.simulate_keystrokes(window, "secondary-,");
        cx.run_until_parked();
        assert!(!model.read_with(cx, |model, _| model.show_preferences()));

        // New shortcut (`f9`) opens Preferences immediately.
        cx.simulate_keystrokes(window, "f9");
        cx.run_until_parked();
        assert!(model.read_with(cx, |model, _| model.show_preferences()));

        // Input bindings (Ctrl+Z) still win inside an Input after rebinding.
        cx.update_window(window, |_, window, cx| {
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| input.focus(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "x y");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "xy"
        );
        cx.simulate_keystrokes(window, "secondary-z");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            ""
        );
    }
}
