//! Rutas de datos de usuario (las de Electron) y migración de la config antigua.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// `productName` de la app: nombre de su carpeta de datos de usuario.
pub const APP_NAME: &str = "Mergeon";

/// Archivo de configuración de la app nativa.
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Archivo de `electron-store` de la app Electron (`name: 'config'`).
pub const ELECTRON_CONFIG_FILE_NAME: &str = "config.json";

// Al renombrar la app a Mergeon, Electron pasó a usar otra carpeta de datos, así
// que las cuentas y los repos recientes quedarían "perdidos". Se copia una sola
// vez la configuración anterior si la nueva todavía no existe.
pub const LEGACY_APP_DIRECTORIES: [&str; 2] = ["git-gui", "GitGUI"];

/// `app.getPath('appData')` de Electron: `$XDG_CONFIG_HOME` o `~/.config` en
/// Linux, `~/Library/Application Support` en macOS y `%APPDATA%` en Windows.
pub fn default_app_data_dir() -> Option<PathBuf> {
    app_data_dir_from(&|name| std::env::var_os(name))
}

/// `app.getPath('userData')` de Electron para `productName` "Mergeon".
pub fn default_user_data_dir() -> Option<PathBuf> {
    default_app_data_dir().map(|app_data| app_data.join(APP_NAME))
}

fn env_path(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    env(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn app_data_dir_from(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    env_path(env, "APPDATA")
}

#[cfg(target_os = "macos")]
fn app_data_dir_from(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    env_path(env, "HOME").map(|home| home.join("Library").join("Application Support"))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn app_data_dir_from(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    env_path(env, "XDG_CONFIG_HOME")
        .or_else(|| env_path(env, "HOME").map(|home| home.join(".config")))
}

/// Qué hizo [`migrate_legacy_config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyMigration {
    /// Todavía no hay rutas que consultar.
    MissingPaths,
    /// Ya existe una configuración: nunca se pisa.
    TargetExists,
    /// No hay configuración anterior que recuperar.
    NothingToMigrate,
    /// Se copió la configuración anterior.
    Copied { source: PathBuf },
    /// La copia falló; el arranque sigue.
    Failed { source: PathBuf, message: String },
}

/// `migrateLegacyConfig` de `store.js`: copia `<appData>/git-gui/config.json`
/// (o `GitGUI`) a `<userData>/config.json` si este todavía no existe.
///
/// Es un camino que corre una sola vez, en el arranque, y que se traga sus
/// errores: nunca debe impedir que la app arranque.
pub fn migrate_legacy_config(
    user_data_dir: Option<&Path>,
    app_data_dir: Option<&Path>,
) -> LegacyMigration {
    let (Some(user_data_dir), Some(app_data_dir)) = (user_data_dir, app_data_dir) else {
        return LegacyMigration::MissingPaths;
    };

    let target = user_data_dir.join(ELECTRON_CONFIG_FILE_NAME);
    if target.exists() {
        return LegacyMigration::TargetExists;
    }

    for directory in LEGACY_APP_DIRECTORIES {
        let source = app_data_dir.join(directory).join(ELECTRON_CONFIG_FILE_NAME);
        if source.exists() {
            let copied =
                fs::create_dir_all(user_data_dir).and_then(|()| fs::copy(&source, &target));
            return match copied {
                Ok(_) => LegacyMigration::Copied { source },
                Err(error) => {
                    log::error!("No se pudo migrar la configuración anterior: {error}");
                    LegacyMigration::Failed {
                        source,
                        message: error.to_string(),
                    }
                }
            };
        }
    }

    LegacyMigration::NothingToMigrate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn linux_uses_xdg_config_home_or_dot_config() {
        assert_eq!(
            app_data_dir_from(&env_of(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.config"))
        );
        assert_eq!(
            app_data_dir_from(&env_of(&[
                ("HOME", "/home/u"),
                ("XDG_CONFIG_HOME", "/custom/config")
            ])),
            Some(PathBuf::from("/custom/config"))
        );
        // Vacía cuenta como no definida.
        assert_eq!(
            app_data_dir_from(&env_of(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "")])),
            Some(PathBuf::from("/home/u/.config"))
        );
        assert_eq!(app_data_dir_from(&env_of(&[])), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_application_support() {
        assert_eq!(
            app_data_dir_from(&env_of(&[("HOME", "/Users/u")])),
            Some(PathBuf::from("/Users/u/Library/Application Support"))
        );
        assert_eq!(app_data_dir_from(&env_of(&[])), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_uses_appdata() {
        assert_eq!(
            app_data_dir_from(&env_of(&[("APPDATA", r"C:\Users\u\AppData\Roaming")])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming"))
        );
        assert_eq!(app_data_dir_from(&env_of(&[])), None);
    }

    #[test]
    fn user_data_dir_is_the_app_folder_inside_app_data() {
        if let (Some(app_data), Some(user_data)) = (default_app_data_dir(), default_user_data_dir())
        {
            assert_eq!(user_data, app_data.join("Mergeon"));
        }
    }
}
