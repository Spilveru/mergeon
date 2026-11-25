use std::collections::HashMap;
use std::path::PathBuf;

/// Plataforma con los nombres de `process.platform` de Node, que son los que
/// usa el original (`win32`, `linux`, `darwin`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Win32,
    Linux,
    Darwin,
    Other(String),
}

impl Platform {
    /// Plataforma para la que se compiló el binario.
    pub fn current() -> Self {
        match std::env::consts::OS {
            "windows" => Self::Win32,
            "linux" => Self::Linux,
            "macos" => Self::Darwin,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn from_node_name(name: &str) -> Self {
        match name {
            "win32" => Self::Win32,
            "linux" => Self::Linux,
            "darwin" => Self::Darwin,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Win32 => "win32",
            Self::Linux => "linux",
            Self::Darwin => "darwin",
            Self::Other(name) => name,
        }
    }
}

/// Arquitectura con los nombres de `process.arch` de Node (`x64`, `arm64`...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
    Other(String),
}

impl Arch {
    /// Arquitectura para la que se compiló el binario.
    pub fn current() -> Self {
        match std::env::consts::ARCH {
            "x86_64" => Self::X64,
            "aarch64" => Self::Arm64,
            "x86" => Self::Other("ia32".to_string()),
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
            Self::Other(name) => name,
        }
    }
}

/// Variables de entorno (`process.env`), inyectables en los tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: HashMap<String, String>,
}

impl Env {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entorno del proceso. Las variables que no son UTF-8 se ignoran.
    pub fn from_process() -> Self {
        Self {
            vars: std::env::vars_os()
                .filter_map(|(key, value)| {
                    Some((key.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
        }
    }

    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), value.into());
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.vars.remove(key)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// `Boolean(env.X)`: definida y no vacía.
    pub fn is_truthy(&self, key: &str) -> bool {
        self.get(key).is_some_and(|value| !value.is_empty())
    }
}

// Whether the updater can actually download + install an update in place on
// this platform/build. Windows (NSIS) and the Linux AppImage support it. The
// Linux .deb is not auto-updatable (only AppImage is). macOS needs a Developer ID
// signature + notarization (our ad-hoc build can't self-install), so until that
// exists we treat Mac as notify-only.
pub fn resolve_can_auto_install(platform: &Platform, env: &Env) -> bool {
    match platform {
        Platform::Win32 => true,
        Platform::Linux => env.is_truthy("APPIMAGE"),
        _ => false,
    }
}

pub fn resolve_update_feed_url(env: &Env) -> Option<String> {
    let value = env.get("APP_UPDATE_FEED_URL").unwrap_or("").trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Canal de actualizaciones fijado al compilar: el `APP_UPDATE_FEED_URL` del
/// entorno de `cargo build` (los workflows de release lo pasan). Sin él, solo
/// el AppImage tenía canal, porque su `AppRun` exporta la variable; el `.exe`,
/// el `.deb` y el `.app` no tienen envoltorio donde fijarla.
pub const BUILT_IN_UPDATE_FEED_URL: Option<&str> = option_env!("APP_UPDATE_FEED_URL");

/// El canal del entorno de ejecución y, si no lo hay (variable ausente o en
/// blanco, el mismo criterio que `AppRun`), el fijado al compilar.
pub fn resolve_update_feed_url_or(env: &Env, built_in: Option<&str>) -> Option<String> {
    resolve_update_feed_url(env).or_else(|| {
        built_in
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

/// Directorio de caché por defecto de las descargas, el mismo criterio que
/// electron-updater (`<caché del usuario>/mergeon-updater`).
pub fn default_cache_dir(platform: &Platform, env: &Env) -> Option<PathBuf> {
    const DIR_NAME: &str = "mergeon-updater";
    let non_empty = |key: &str| env.get(key).filter(|value| !value.is_empty());

    match platform {
        Platform::Win32 => non_empty("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                non_empty("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            })
            .map(|base| base.join(DIR_NAME)),
        Platform::Darwin => non_empty("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join(DIR_NAME)
        }),
        _ => non_empty("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| non_empty("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .map(|base| base.join(DIR_NAME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_runtime_feed_url_when_present() {
        let env = Env::from_pairs([("APP_UPDATE_FEED_URL", " https://updates.example.com/mac ")]);
        assert_eq!(
            resolve_update_feed_url(&env).as_deref(),
            Some("https://updates.example.com/mac")
        );
        assert_eq!(resolve_update_feed_url(&Env::new()), None);
        assert_eq!(
            resolve_update_feed_url(&Env::from_pairs([("APP_UPDATE_FEED_URL", "   ")])),
            None
        );
    }

    #[test]
    fn falls_back_to_the_feed_fixed_at_build_time() {
        let built_in = Some(" https://updates.example.com/built-in ");
        assert_eq!(
            resolve_update_feed_url_or(&Env::new(), built_in).as_deref(),
            Some("https://updates.example.com/built-in")
        );
        // El del entorno de ejecución manda.
        let env = Env::from_pairs([("APP_UPDATE_FEED_URL", "https://updates.example.com/env")]);
        assert_eq!(
            resolve_update_feed_url_or(&env, built_in).as_deref(),
            Some("https://updates.example.com/env")
        );
        // En blanco cuenta como ausente, igual que en `AppRun`.
        let blank = Env::from_pairs([("APP_UPDATE_FEED_URL", "  ")]);
        assert_eq!(
            resolve_update_feed_url_or(&blank, built_in).as_deref(),
            Some("https://updates.example.com/built-in")
        );
        assert_eq!(resolve_update_feed_url_or(&Env::new(), None), None);
        assert_eq!(resolve_update_feed_url_or(&Env::new(), Some("   ")), None);
    }

    #[test]
    fn resolve_can_auto_install_reflects_platform_support() {
        assert!(resolve_can_auto_install(&Platform::Win32, &Env::new()));
        assert!(resolve_can_auto_install(
            &Platform::Linux,
            &Env::from_pairs([("APPIMAGE", "/tmp/app.AppImage")])
        ));
        assert!(!resolve_can_auto_install(&Platform::Linux, &Env::new()));
        assert!(!resolve_can_auto_install(&Platform::Darwin, &Env::new()));
        // `Boolean('')` es falso: una variable vacía no cuenta.
        assert!(!resolve_can_auto_install(
            &Platform::Linux,
            &Env::from_pairs([("APPIMAGE", "")])
        ));
    }

    #[test]
    fn platform_and_arch_use_node_names() {
        assert_eq!(Platform::from_node_name("win32"), Platform::Win32);
        assert_eq!(Platform::from_node_name("freebsd").as_str(), "freebsd");
        assert_eq!(Platform::Darwin.as_str(), "darwin");
        assert_eq!(Arch::X64.as_str(), "x64");
        assert_eq!(Arch::Arm64.as_str(), "arm64");
    }

    #[test]
    fn default_cache_dir_follows_each_platform_convention() {
        let linux = Env::from_pairs([("HOME", "/home/ana")]);
        assert_eq!(
            default_cache_dir(&Platform::Linux, &linux),
            Some(PathBuf::from("/home/ana/.cache/mergeon-updater"))
        );
        let xdg = Env::from_pairs([("HOME", "/home/ana"), ("XDG_CACHE_HOME", "/var/cache/ana")]);
        assert_eq!(
            default_cache_dir(&Platform::Linux, &xdg),
            Some(PathBuf::from("/var/cache/ana/mergeon-updater"))
        );
        assert_eq!(
            default_cache_dir(&Platform::Darwin, &linux),
            Some(PathBuf::from("/home/ana/Library/Caches/mergeon-updater"))
        );
        let windows = Env::from_pairs([("LOCALAPPDATA", "C:\\Users\\ana\\AppData\\Local")]);
        assert_eq!(
            default_cache_dir(&Platform::Win32, &windows),
            Some(PathBuf::from("C:\\Users\\ana\\AppData\\Local").join("mergeon-updater"))
        );
        assert_eq!(default_cache_dir(&Platform::Linux, &Env::new()), None);
    }
}
