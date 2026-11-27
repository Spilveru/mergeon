//! Mergeon: cliente Git de escritorio (versión nativa en GPUI).

mod commands;
mod file_index;
mod logging;
mod palette;
#[cfg(test)]
mod palette_tests;
mod shell;
mod shortcuts;
mod test_mode;
mod ui_host;
mod views;
mod workspace;
mod workspace_switcher;

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::TitleBar;
use gpui_kit::prelude::*;
use gpui_kit::{Bounds, WindowBounds, WindowDecorations, WindowOptions, px, size};
use mergeon_app::{AppModel, AppUpdaterBridge, backend_with_events};
use mergeon_backend::BackendOptions;
use mergeon_updater::{AppUpdater, UpdaterConfig};

use crate::shell::AppShell;
use crate::ui_host::ShellUiHost;

/// Un binario instalado se actualiza solo; uno de desarrollo, no.
fn is_packaged() -> bool {
    !cfg!(debug_assertions) && std::env::var_os("MERGEON_DEV").is_none()
}

/// Registra los atajos y componentes de toda la interfaz en el orden correcto.
///
/// Los atajos globales de `shortcuts::init` van sin contexto (`None`) para
/// funcionar también cuando nada tiene el foco; en GPUI un binding sin contexto
/// empata en profundidad con el contexto más interno y gana el registrado
/// después, así que `shortcuts::init` debe ir ANTES que `mergeon_ui::init`
/// (`Input`: Ctrl+Z / Ctrl+Y) y las vistas (`DiffViewer`: Ctrl+F).
pub(crate) fn init_bindings(cx: &mut gpui_kit::App) {
    shortcuts::init(cx);
    bind_ui_and_view_keys(cx);
}

/// Vuelve a registrar los atajos en caliente tras un cambio del usuario,
/// manteniendo exactamente la misma precedencia (`shortcuts` antes que
/// `mergeon_ui` y las vistas para que `Input`, `DiffViewer` y `CommitPanel`
/// sigan ganando en sus contextos).
pub(crate) fn rebind_shortcuts(
    overrides: &std::collections::BTreeMap<String, String>,
    cx: &mut gpui_kit::App,
) {
    cx.clear_key_bindings();
    shortcuts::bind_keys(cx, overrides);
    bind_ui_and_view_keys(cx);
}

fn bind_ui_and_view_keys(cx: &mut gpui_kit::App) {
    mergeon_ui::init(cx);
    mergeon_view_diff::init(cx);
    mergeon_view_toolbar::init(cx);
    mergeon_view_sidebar::init(cx);
    mergeon_view_home::init(cx);
    mergeon_view_commits::init(cx);
    mergeon_view_changes::init(cx);
}

fn main() {
    mergeon_app::startup::mark_origin();
    logging::init();

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(|cx| {
            // Identidad para las notificaciones del sistema (AppUserModelID en
            // Windows; sin ella GPUI no publica ninguna).
            cx.set_app_identity("io.github.spilveru.Mergeon", "Mergeon");
            init_bindings(cx);
            test_mode::install_ui_doubles(cx);
            mergeon_app::startup::trace("ui initialized");

            let updater = Arc::new(AppUpdater::new(UpdaterConfig::for_current_process(
                env!("CARGO_PKG_VERSION"),
                is_packaged(),
            )));
            let mut options =
                BackendOptions::new().updater(Arc::new(AppUpdaterBridge::new(updater.clone())));
            // Perfil alternativo (desarrollo, capturas, pruebas manuales) sin tocar
            // la configuración real del usuario.
            if let Some(dir) = std::env::var_os("MERGEON_USER_DATA_DIR") {
                let dir = std::path::PathBuf::from(dir);
                if let Err(error) = std::fs::create_dir_all(&dir) {
                    log::error!("no se pudo crear {}: {error}", dir.display());
                }
                options = options
                    .store_path(dir.join("settings.json"))
                    .without_electron_import();
            }
            let options = test_mode::configure_backend(options);
            let (backend, events) = match backend_with_events(options) {
                Ok(backend) => backend,
                Err(error) => {
                    log::error!("no se pudo iniciar el backend: {error}");
                    cx.quit();
                    return;
                }
            };
            mergeon_app::startup::trace("backend ready");
            let model = AppModel::init(backend, events, cx);

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(600.))),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("io.github.spilveru.Mergeon".into()),
                ..TitleBar::window_options()
            };
            let (window, _shell) = match gpui_kit::open_window(window_options, cx, |window, cx| {
                window.set_window_title("Mergeon");
                cx.new(|cx| AppShell::new(window, cx))
            }) {
                Ok(opened) => opened,
                Err(error) => {
                    log::error!("no se pudo abrir la ventana: {error}");
                    cx.quit();
                    return;
                }
            };

            mergeon_app::startup::trace("window opened");
            model.update(cx, |model, cx| {
                let pick_directory = test_mode::PickDirectoryAnswer::default();
                test_mode::start_control(window, pick_directory.clone(), cx);
                model.set_ui_host(Rc::new(ShellUiHost::new(window, pick_directory)), cx);
                let started = model.start(cx);
                // `mergeon <ruta>...`: abre esos repositorios al arrancar.
                let paths: Vec<String> = std::env::args().skip(1).collect();
                cx.spawn(async move |this, cx| {
                    started.await;
                    for path in paths {
                        let opened = this.update(cx, |model, cx| {
                            model.open_repository_path(
                                &path,
                                "The selected directory is not a valid Git repository",
                                cx,
                            )
                        });
                        if let Ok(opened) = opened {
                            let _ = opened.await;
                        }
                    }
                })
                .detach();
            });
            updater.start();
            cx.activate(true);
        });
}
