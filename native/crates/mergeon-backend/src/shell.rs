//! Canales `shell:*` (port de `electron/ipc/shell-handlers.js`).
//!
//! Sin Electron no hay `shell.openExternal` / `openPath` / `showItemInFolder`:
//! se lanzan los programas del sistema que hacen lo mismo. Todo proceso pasa
//! por [`ProcessLauncher`], de modo que los tests comprueban qué se habría
//! lanzado sin abrir nada.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use mergeon_store::ExternalEditorSettings;

use crate::editor::{
    DetectedEditor, EDITORS, EditorDefinition, EditorLaunch, EditorOpened, custom_arguments,
    editor_arguments, editor_definition,
};
use crate::git::SuccessResult;

/// Plataforma para la que se eligen los programas (`os.platform()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellPlatform {
    /// Linux y el resto de Unix con escritorio XDG.
    Linux,
    MacOs,
    Windows,
}

impl ShellPlatform {
    /// La plataforma para la que se compiló el binario.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            ShellPlatform::Windows
        } else if cfg!(target_os = "macos") {
            ShellPlatform::MacOs
        } else {
            ShellPlatform::Linux
        }
    }
}

/// Por qué no arrancó un proceso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchErrorKind {
    /// El ejecutable no existe (`ENOENT`).
    NotFound,
    Other,
}

/// Fallo al lanzar un proceso (el evento `error` de `spawn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchError {
    pub kind: LaunchErrorKind,
    pub message: String,
}

impl LaunchError {
    pub fn not_found(command: &str) -> Self {
        Self {
            kind: LaunchErrorKind::NotFound,
            message: format!("spawn {command} ENOENT"),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: LaunchErrorKind::Other,
            message: message.into(),
        }
    }

    fn from_io(command: &str, error: &std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::not_found(command),
            std::io::ErrorKind::PermissionDenied => Self::other(format!("spawn {command} EACCES")),
            _ => Self::other(format!("spawn {command}: {error}")),
        }
    }
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LaunchError {}

/// Lanzador de procesos del sistema.
pub trait ProcessLauncher: Send + Sync {
    /// `spawnDetached`: lanza el proceso separado de la app (sobrevive a su
    /// cierre), sin entrada ni salida, y vuelve en cuanto arranca.
    fn spawn_detached(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<(), LaunchError>;

    /// Lanza el proceso, sin entrada ni salida, y espera a que termine.
    /// Devuelve su código de salida (`None` si lo mató una señal).
    fn run(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<Option<i32>, LaunchError>;

    /// Como [`ProcessLauncher::run`], pero sin esperar más de `limit`: si el
    /// proceso sigue vivo entonces, se le deja seguir (separado, como
    /// `spawn_detached`) y devuelve `Ok(Running)`. Para lanzadores como
    /// `xdg-open`, que sin escritorio ejecutan el programa en primer plano y
    /// no vuelven hasta que se cierra.
    fn run_for(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
        limit: std::time::Duration,
    ) -> Result<RunOutcome, LaunchError> {
        let _ = limit;
        self.run(command, args, cwd).map(RunOutcome::Exited)
    }
}

/// Lo que da [`ProcessLauncher::run_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Terminó con este código (`None` si lo mató una señal).
    Exited(Option<i32>),
    /// Seguía en marcha al acabar el plazo.
    Running,
}

/// Lanzador real, sobre `std::process`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLauncher;

impl SystemLauncher {
    fn command(command: &str, args: &[String], cwd: Option<&Path>) -> std::process::Command {
        use std::process::Stdio;

        let mut process = std::process::Command::new(command);
        process
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        process
    }
}

impl ProcessLauncher for SystemLauncher {
    fn spawn_detached(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<(), LaunchError> {
        let mut process = Self::command(command, args, cwd);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Grupo de procesos propio: las señales que reciba la app (Ctrl+C
            // en la terminal que la lanzó) no llegan al hijo.
            process.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP, lo mismo que
            // `detached: true` en Node.
            process.creation_flags(0x0000_0008 | 0x0000_0200);
        }

        let child = process
            .spawn()
            .map_err(|error| LaunchError::from_io(command, &error))?;
        // `child.unref()`: nadie espera su resultado, pero hay que recogerlo
        // al terminar para no dejar un zombi mientras viva la app.
        reap(child, command);
        Ok(())
    }

    fn run_for(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
        limit: std::time::Duration,
    ) -> Result<RunOutcome, LaunchError> {
        let mut process = Self::command(command, args, cwd);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Si sigue vivo al acabar el plazo se queda separado de la app.
            process.process_group(0);
        }
        let mut child = process
            .spawn()
            .map_err(|error| LaunchError::from_io(command, &error))?;
        let deadline = std::time::Instant::now() + limit;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(RunOutcome::Exited(status.code())),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Ok(None) => {
                    reap(child, command);
                    return Ok(RunOutcome::Running);
                }
                Err(error) => return Err(LaunchError::from_io(command, &error)),
            }
        }
    }

    fn run(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<Option<i32>, LaunchError> {
        let mut process = Self::command(command, args, cwd);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: sin esto cada comando abre una consola un instante.
            process.creation_flags(0x0800_0000);
        }
        process
            .status()
            .map(|status| status.code())
            .map_err(|error| LaunchError::from_io(command, &error))
    }
}

/// Recoge `child` en un hilo cuando termine, para no dejar un zombi.
fn reap(mut child: std::process::Child, command: &str) {
    let reaper = std::thread::Builder::new()
        .name("mergeon-shell-child".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(error) = reaper {
        log::warn!("Failed to start the thread that reaps {command}: {error}");
    }
}

/// Lo que se espera a `xdg-open` antes de dar por abierto lo que lanzó: sin
/// escritorio (o con uno que no tenga `gio`/`kde-open`) ejecuta el programa
/// en primer plano y no vuelve hasta que se cierra, y la app se quedaba sin
/// respuesta (ni aviso de éxito ni de fallo).
const XDG_OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Error de un canal `shell:*`, con el mensaje que lanzaba el handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellError {
    message: String,
}

impl ShellError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ShellError {}

impl From<LaunchError> for ShellError {
    fn from(error: LaunchError) -> Self {
        Self::new(error.message)
    }
}

/// Un terminal y sus argumentos; `{path}` se sustituye por el directorio.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Terminal {
    cmd: String,
    args: Vec<String>,
}

const PATH_PLACEHOLDER: &str = "{path}";

// Terminales con su flag de directorio inicial. `x-terminal-emulator` no va
// aquí: es un alias de alternatives que puede apuntar a xterm o urxvt, que no
// aceptan ese flag; se lanza sin argumentos y hereda `cwd`.
const LINUX_TERMINALS: [(&str, &[&str]); 12] = [
    // Modern GNOME terminal (used in Fedora/Bazzite)
    ("ptyxis", &["--working-directory", PATH_PLACEHOLDER]),
    ("kgx", &["--working-directory", PATH_PLACEHOLDER]),
    ("gnome-console", &["--working-directory", PATH_PLACEHOLDER]),
    // Traditional terminals
    ("gnome-terminal", &["--working-directory", PATH_PLACEHOLDER]),
    ("konsole", &["--workdir", PATH_PLACEHOLDER]),
    ("xfce4-terminal", &["--working-directory", PATH_PLACEHOLDER]),
    ("mate-terminal", &["--working-directory", PATH_PLACEHOLDER]),
    ("tilix", &["--working-directory", PATH_PLACEHOLDER]),
    ("kitty", &["--directory", PATH_PLACEHOLDER]),
    ("alacritty", &["--working-directory", PATH_PLACEHOLDER]),
    ("xterm", &[]),
    ("x-terminal-emulator", &[]),
];

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn linux_terminals() -> Vec<Terminal> {
    LINUX_TERMINALS
        .iter()
        .map(|(cmd, args)| Terminal {
            cmd: cmd.to_string(),
            args: strings(args),
        })
        .collect()
}

/// Lectura de variables de entorno (`process.env`).
type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// ¿Existe este archivo? (para buscar ejecutables sin lanzar nada).
type FileProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `window.api.shell`: abrir URLs, rutas y terminales con los programas del
/// sistema.
pub struct Shell {
    launcher: Arc<dyn ProcessLauncher>,
    platform: ShellPlatform,
    env: EnvLookup,
    file_exists: FileProbe,
    /// Cache for detected terminal
    detected_terminal: Mutex<Option<Terminal>>,
    /// El editor externo elegido en los ajustes.
    external_editor: Mutex<ExternalEditorSettings>,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell")
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    /// Shell de la plataforma actual, con el lanzador real y el entorno del
    /// proceso.
    pub fn new() -> Self {
        Self::with_launcher(Arc::new(SystemLauncher), ShellPlatform::current())
    }

    /// Shell con un lanzador propio y la plataforma indicada; los tests
    /// inyectan uno que solo registra lo que se lanzaría.
    pub fn with_launcher(launcher: Arc<dyn ProcessLauncher>, platform: ShellPlatform) -> Self {
        Self {
            launcher,
            platform,
            env: Arc::new(|name| std::env::var(name).ok()),
            file_exists: Arc::new(|path| Path::new(path).is_file()),
            detected_terminal: Mutex::new(None),
            external_editor: Mutex::new(ExternalEditorSettings::default()),
        }
    }

    /// Sustituye la comprobación de que un archivo existe (la búsqueda de
    /// editores en el `PATH` y en sus rutas de instalación).
    pub fn with_file_probe(
        mut self,
        exists: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.file_exists = Arc::new(exists);
        self
    }

    /// Sustituye la lectura de variables de entorno (`$TERMINAL`).
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env = Arc::new(env);
        self
    }

    pub fn platform(&self) -> ShellPlatform {
        self.platform
    }

    /// Canal `shell:openExternal`: abre una URL en el navegador del sistema.
    ///
    /// Solo http/https: una interfaz comprometida no debe poder abrir file://
    /// ni esquemas propios del sistema a través de este canal.
    pub fn open_external(&self, url: &str) -> Result<(), ShellError> {
        let parsed = url::Url::parse(url).map_err(|_| ShellError::new("Invalid URL."))?;

        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(ShellError::new(format!(
                "Blocked URL scheme '{}:'. Only http and https are allowed.",
                parsed.scheme()
            )));
        }

        let url = parsed.to_string();
        let (command, args): (&str, Vec<String>) = match self.platform {
            ShellPlatform::Linux => ("xdg-open", vec![url]),
            ShellPlatform::MacOs => ("open", vec![url]),
            // No `cmd /c start`: cmd interpretaría los `&` de la URL como
            // separador de comandos. Este manejador recibe la URL intacta.
            ShellPlatform::Windows => (
                "rundll32.exe",
                vec!["url.dll,FileProtocolHandler".to_string(), url],
            ),
        };
        self.launcher.spawn_detached(command, &args, None)?;
        Ok(())
    }

    /// Canal `shell:openPath`: abre una carpeta en el explorador de archivos
    /// (o un archivo con su programa por defecto).
    pub fn open_path(&self, path: &str) -> Result<SuccessResult, ShellError> {
        let error = self.open_path_error(path);
        if !error.is_empty() {
            return Err(ShellError::new(format!("Failed to open path: {error}")));
        }
        Ok(SuccessResult::ok())
    }

    /// `shell.openPath(path)`: vacío si se abrió; si no, el motivo.
    fn open_path_error(&self, path: &str) -> String {
        const GENERIC_FAILURE: &str = "Failed to open path";

        match self.platform {
            ShellPlatform::Linux => {
                // Como Electron: `xdg-open` desde la carpeta que contiene la
                // ruta, esperando su código de salida.
                let cwd = Path::new(path)
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty());
                // Sin volver en `XDG_OPEN_WAIT`, está ejecutando el programa
                // que abre la ruta: se abrió.
                match self
                    .launcher
                    .run_for("xdg-open", &[path.to_string()], cwd, XDG_OPEN_WAIT)
                {
                    Ok(RunOutcome::Running | RunOutcome::Exited(Some(0))) => String::new(),
                    Ok(RunOutcome::Exited(Some(1))) => "Error in command line syntax".to_string(),
                    Ok(RunOutcome::Exited(Some(2))) => "The item does not exist".to_string(),
                    Ok(RunOutcome::Exited(Some(3))) => {
                        "A required tool could not be found".to_string()
                    }
                    Ok(RunOutcome::Exited(Some(4))) => "The action failed".to_string(),
                    Ok(_) | Err(_) => GENERIC_FAILURE.to_string(),
                }
            }
            ShellPlatform::MacOs => match self.launcher.run("open", &[path.to_string()], None) {
                Ok(Some(0)) => String::new(),
                Ok(_) | Err(_) => GENERIC_FAILURE.to_string(),
            },
            ShellPlatform::Windows => {
                // `explorer` abre "Documentos" cuando la ruta no existe y su
                // código de salida no dice nada: se comprueba antes.
                if !Path::new(path).exists() {
                    return GENERIC_FAILURE.to_string();
                }
                match self
                    .launcher
                    .spawn_detached("explorer.exe", &[explorer_path(path)], None)
                {
                    Ok(()) => String::new(),
                    Err(_) => GENERIC_FAILURE.to_string(),
                }
            }
        }
    }

    /// Canal `shell:showItemInFolder`: abre el explorador de archivos con el
    /// elemento seleccionado. Como en el original, no informa de fallos.
    pub fn show_item_in_folder(&self, path: &str) -> SuccessResult {
        let launched = match self.platform {
            ShellPlatform::Linux => self.show_item_in_linux_file_manager(path),
            ShellPlatform::MacOs => {
                self.launcher
                    .spawn_detached("open", &["-R".to_string(), path.to_string()], None)
            }
            ShellPlatform::Windows => self.launcher.spawn_detached(
                "explorer.exe",
                &["/select,".to_string(), explorer_path(path)],
                None,
            ),
        };
        if let Err(error) = launched {
            log::warn!("Failed to show the item in its folder: {error}");
        }
        SuccessResult::ok()
    }

    /// Como Electron en Linux: se pide al gestor de archivos que seleccione el
    /// elemento (`org.freedesktop.FileManager1`) y, si ninguno atiende, se
    /// abre la carpeta que lo contiene.
    fn show_item_in_linux_file_manager(&self, path: &str) -> Result<(), LaunchError> {
        if let Ok(uri) = url::Url::from_file_path(path) {
            // `array:string:` separa los elementos por comas.
            let uri = uri.as_str().replace(',', "%2C");
            let shown = self.launcher.run(
                "dbus-send",
                &[
                    "--session".to_string(),
                    "--print-reply".to_string(),
                    "--dest=org.freedesktop.FileManager1".to_string(),
                    "--type=method_call".to_string(),
                    "/org/freedesktop/FileManager1".to_string(),
                    "org.freedesktop.FileManager1.ShowItems".to_string(),
                    format!("array:string:{uri}"),
                    "string:".to_string(),
                ],
                None,
            );
            if matches!(shown, Ok(Some(0))) {
                return Ok(());
            }
        }

        let folder = Path::new(path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(
                || path.to_string(),
                |parent| parent.to_string_lossy().into_owned(),
            );
        self.launcher.spawn_detached("xdg-open", &[folder], None)
    }

    /// Canal `shell:openTerminal`: abre un terminal en `path`.
    pub fn open_terminal(&self, path: &str) -> Result<SuccessResult, ShellError> {
        match self.platform {
            ShellPlatform::Windows => self.open_windows_terminal(path),
            ShellPlatform::MacOs => self.spawn_detached(
                "open",
                &["-a".to_string(), "Terminal".to_string(), path.to_string()],
                None,
            ),
            // Linux - detect available terminal
            ShellPlatform::Linux => {
                let Some(terminal) = self.detect_linux_terminal() else {
                    return Err(ShellError::new(
                        "No terminal emulator found. Set $TERMINAL or install ptyxis, gnome-terminal, konsole, xfce4-terminal, or x-terminal-emulator.",
                    ));
                };

                let terminal_args: Vec<String> = terminal
                    .args
                    .iter()
                    .map(|value| {
                        if value == PATH_PLACEHOLDER {
                            path.to_string()
                        } else {
                            value.clone()
                        }
                    })
                    .collect();

                // `cwd` cubre a los terminales sin flag de directorio; para flatpak-spawn
                // el cwd es el de la sandbox, así que ahí solo valen los flags.
                self.spawn_detached(&terminal.cmd, &terminal_args, Some(Path::new(path)))
            }
        }
    }

    fn spawn_detached(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<SuccessResult, ShellError> {
        self.launcher.spawn_detached(command, args, cwd)?;
        Ok(SuccessResult::ok())
    }

    /*
     * En Windows un proceso separado nace sin consola (DETACHED_PROCESS), así
     * que un cmd.exe lanzado directo era invisible y el botón parecía no hacer
     * nada. Sin separarlo, el hijo muere al cerrar Mergeon. Por eso primero se
     * prueba Windows Terminal (app gráfica, no necesita consola) y, si no
     * está, un cmd.exe intermedio ejecuta `start`, que abre una consola nueva
     * y visible. `cwd` fija la carpeta sin citar la ruta.
     */
    fn open_windows_terminal(&self, path: &str) -> Result<SuccessResult, ShellError> {
        match self
            .launcher
            .spawn_detached("wt.exe", &["-d".to_string(), path.to_string()], None)
        {
            Ok(()) => return Ok(SuccessResult::ok()),
            // Solo un ejecutable ausente pasa al plan B; lo demás se informa.
            Err(error) if error.kind != LaunchErrorKind::NotFound => return Err(error.into()),
            Err(_) => {}
        }
        self.spawn_detached(
            "cmd.exe",
            &strings(&["/c", "start", "cmd.exe"]),
            Some(Path::new(path)),
        )
    }

    /// Check if a command exists
    fn command_exists(&self, cmd: &str) -> bool {
        matches!(
            self.launcher.run("which", &[cmd.to_string()], None),
            Ok(Some(0))
        )
    }

    // El terminal que el usuario eligió en $TERMINAL va antes que la lista fija.
    // Si no está en la tabla, se lanza sin flags y arranca en `cwd`.
    fn preferred_linux_terminal(&self) -> Option<Terminal> {
        let configured = (self.env)("TERMINAL").unwrap_or_default();
        let configured = configured.trim();
        if configured.is_empty() {
            return None;
        }
        let name = configured.rsplit('/').next().unwrap_or(configured);
        let args = LINUX_TERMINALS
            .iter()
            .find(|(cmd, _)| *cmd == name)
            .map(|(_, args)| strings(args))
            .unwrap_or_default();
        Some(Terminal {
            cmd: configured.to_string(),
            args,
        })
    }

    /// Detect available terminal emulator on Linux
    fn detect_linux_terminal(&self) -> Option<Terminal> {
        if let Some(terminal) = lock(&self.detected_terminal).clone() {
            return Some(terminal);
        }

        // Check if we're in a flatpak/sandbox
        let in_flatpak = self.command_exists("flatpak-spawn");

        let mut terminals = linux_terminals();
        if let Some(preferred) = self.preferred_linux_terminal() {
            terminals.insert(0, preferred);
        }

        let detected = if in_flatpak {
            // If in flatpak, check host system
            terminals.into_iter().find_map(|terminal| {
                let exists = matches!(
                    self.launcher.run(
                        "flatpak-spawn",
                        &[
                            "--host".to_string(),
                            "which".to_string(),
                            terminal.cmd.clone()
                        ],
                        None,
                    ),
                    Ok(Some(0))
                );
                exists.then(|| {
                    let mut args = vec!["--host".to_string(), terminal.cmd];
                    args.extend(terminal.args);
                    Terminal {
                        cmd: "flatpak-spawn".to_string(),
                        args,
                    }
                })
            })
        } else {
            // Check local system
            terminals
                .into_iter()
                .find(|terminal| self.command_exists(&terminal.cmd))
        };

        if let Some(terminal) = &detected {
            *lock(&self.detected_terminal) = Some(terminal.clone());
        }
        detected
    }
}

// ---------------------------------------------------------------------------
// Editor externo
// ---------------------------------------------------------------------------

impl Shell {
    /// El editor elegido en los ajustes (`settings.externalEditor`).
    pub fn external_editor(&self) -> ExternalEditorSettings {
        lock(&self.external_editor).clone()
    }

    /// Fija el editor que usará [`Shell::open_in_editor`].
    pub fn set_external_editor(&self, editor: ExternalEditorSettings) {
        *lock(&self.external_editor) = editor;
    }

    /// Los editores del catálogo instalados en este sistema, en el orden del
    /// catálogo.
    pub fn available_editors(&self) -> Vec<DetectedEditor> {
        EDITORS
            .iter()
            .filter_map(|editor| {
                self.find_editor(editor).map(|executable| DetectedEditor {
                    id: editor.id.to_string(),
                    name: editor.name.to_string(),
                    executable,
                })
            })
            .collect()
    }

    /// Qué se lanzaría para abrir `path` (en `line`) con `editor`.
    pub fn editor_launch(
        &self,
        editor: &ExternalEditorSettings,
        path: &str,
        line: Option<u32>,
    ) -> Result<EditorLaunch, ShellError> {
        let path = self.native_path(path);
        let known = |definition: &EditorDefinition, program: String| EditorLaunch::Command {
            name: definition.name.to_string(),
            program,
            args: editor_arguments(definition.line_style, &path, line),
        };
        match editor.editor.as_str() {
            ExternalEditorSettings::SYSTEM => Ok(EditorLaunch::SystemDefault),
            ExternalEditorSettings::AUTO => Ok(EDITORS
                .iter()
                .find_map(|definition| {
                    self.find_editor(definition)
                        .map(|program| known(definition, program))
                })
                .unwrap_or(EditorLaunch::SystemDefault)),
            ExternalEditorSettings::CUSTOM => {
                let command = editor.custom_command.trim();
                if command.is_empty() {
                    return Err(ShellError::new(
                        "No custom editor command is set. Choose one in Settings → Git → External editor.",
                    ));
                }
                Ok(EditorLaunch::Command {
                    name: command.to_string(),
                    program: command.to_string(),
                    args: custom_arguments(&editor.custom_args, &path, line),
                })
            }
            other => Ok(match editor_definition(other) {
                // Si no se encuentra se lanza por su nombre: el fallo dirá
                // claramente que no está instalado.
                Some(definition) => {
                    let program = self
                        .find_editor(definition)
                        .unwrap_or_else(|| definition.commands[0].to_string());
                    known(definition, program)
                }
                // Un id que no es del catálogo: se toma por un ejecutable.
                None => EditorLaunch::Command {
                    name: other.to_string(),
                    program: other.to_string(),
                    args: vec![path.clone()],
                },
            }),
        }
    }

    /// Abre `path` (un archivo, o una carpeta si `line` es `None`) en el
    /// editor elegido, en la línea `line`. Sin editor elegido ni detectado
    /// lo abre el programa por defecto del sistema (y lo dice el resultado,
    /// para que la interfaz pueda ofrecer elegir uno); si eso también falla,
    /// el error lo explica.
    pub fn open_in_editor(
        &self,
        path: &str,
        line: Option<u32>,
    ) -> Result<EditorOpened, ShellError> {
        let editor = self.external_editor();
        match self.editor_launch(&editor, path, line)? {
            EditorLaunch::SystemDefault => {
                if is_potentially_executable_file(path) {
                    return Err(ShellError::new(
                        "Refusing to open a potentially executable file with the system default handler. Configure an external editor in Settings → Git → External editor.",
                    ));
                }
                let fallback = editor.editor != ExternalEditorSettings::SYSTEM;
                match self.open_path(path) {
                    Ok(_) => Ok(EditorOpened::SystemDefault { fallback }),
                    Err(error) if fallback => Err(ShellError::new(format!(
                        "No external editor was found, and the system couldn't open the file either ({}).",
                        error
                            .message()
                            .trim_start_matches("Failed to open path: ")
                            .trim_end_matches('.')
                    ))),
                    Err(error) => Err(error),
                }
            }
            EditorLaunch::Command {
                name,
                program,
                args,
            } => match self.launcher.spawn_detached(&program, &args, None) {
                Ok(()) => Ok(EditorOpened::Editor { name }),
                Err(error) if error.kind == LaunchErrorKind::NotFound => {
                    Err(ShellError::new(format!(
                        "{name} was not found ({program}). Install it or choose another editor in Settings → Git → External editor."
                    )))
                }
                Err(error) => Err(ShellError::new(format!("Could not open {name}: {error}"))),
            },
        }
    }

    /// La ruta con los separadores de la plataforma: en Windows, `\` (las de
    /// git llegan con `/`).
    fn native_path(&self, path: &str) -> String {
        match self.platform {
            ShellPlatform::Windows => explorer_path(path),
            ShellPlatform::Linux | ShellPlatform::MacOs => path.to_string(),
        }
    }

    /// El ejecutable de un editor del catálogo, si está instalado: en Windows
    /// primero sus rutas típicas (el `.exe`, mejor que el `.cmd` del PATH);
    /// en macOS, el PATH y luego el lanzador dentro del `.app`.
    fn find_editor(&self, editor: &EditorDefinition) -> Option<String> {
        let windows = self.platform == ShellPlatform::Windows;
        if editor.windows_only && !windows {
            return None;
        }
        let installed = || -> Vec<String> {
            match self.platform {
                ShellPlatform::Windows => editor
                    .windows_paths
                    .iter()
                    .filter_map(|(variable, rest)| {
                        let base = (self.env)(variable)?;
                        let base = base.trim_end_matches(['\\', '/']);
                        (!base.is_empty()).then(|| format!("{base}\\{rest}"))
                    })
                    .collect(),
                ShellPlatform::MacOs => editor.mac_paths.iter().map(|p| p.to_string()).collect(),
                ShellPlatform::Linux => Vec::new(),
            }
        };
        let in_path = || {
            editor
                .commands
                .iter()
                .find_map(|command| self.find_in_path(command))
        };
        if windows {
            installed()
                .into_iter()
                .find(|candidate| (self.file_exists)(candidate))
                .or_else(in_path)
        } else {
            in_path().or_else(|| {
                installed()
                    .into_iter()
                    .find(|candidate| (self.file_exists)(candidate))
            })
        }
    }

    /// `which`, sin lanzar nada: el primer directorio del `PATH` que tiene
    /// `command` (en Windows, con su extensión de ejecutable).
    fn find_in_path(&self, command: &str) -> Option<String> {
        let windows = self.platform == ShellPlatform::Windows;
        let (list_separator, separator) = if windows { (';', '\\') } else { (':', '/') };
        let extensions: &[&str] = if windows {
            &[".exe", ".cmd", ".bat"]
        } else {
            &[""]
        };
        let path = (self.env)("PATH")?;
        path.split(list_separator)
            .map(|dir| dir.trim().trim_matches('"'))
            .filter(|dir| !dir.is_empty())
            .flat_map(|dir| {
                let dir = dir.trim_end_matches(['/', separator]).to_string();
                extensions
                    .iter()
                    .map(move |extension| format!("{dir}{separator}{command}{extension}"))
            })
            .find(|candidate| (self.file_exists)(candidate))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// La ruta como la entiende el Explorador de Windows: solo con `\`. Las rutas
/// de git llegan con `/` (`C:/Users/…`) y las de archivos de un repo, mezcladas
/// (`C:\repo/src/x`); con `/select,` el Explorador no las reconoce y abre
/// otra carpeta (Documentos o Este equipo) en vez de la del elemento.
fn explorer_path(path: &str) -> String {
    path.replace('/', "\\")
}

/// Extensiones que el lanzador por defecto del sistema operativo puede ejecutar
/// en lugar de abrir en un editor de texto.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "bat", "cmd", "com", "msi", "ps1", "vbs", "vbe", "jse", "wsf", "wsh", "scr", "pif",
    "lnk", "app", "command", "action", "workflow", "desktop", "sh", "csh", "zsh", "fish",
];

/// `true` si `path` tiene una extensión ejecutable peligrosa para el lanzador
/// por defecto del SO (sin editor explícito). Las carpetas se permiten siempre.
pub(crate) fn is_potentially_executable_file(path: &str) -> bool {
    let candidate = Path::new(path);
    if candidate.is_dir() {
        return false;
    }
    let Some(ext) = candidate.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    EXECUTABLE_EXTENSIONS
        .iter()
        .any(|blocked| ext.eq_ignore_ascii_case(blocked))
}

#[cfg(test)]
mod executable_guard_tests {
    use super::*;

    #[test]
    fn blocks_potentially_executable_extensions_but_allows_source_files() {
        for blocked in [
            "setup.exe",
            "SCRIPT.BAT",
            "evil.lnk",
            "launch.desktop",
            "run.command",
            "install.sh",
            "payload.ps1",
        ] {
            assert!(is_potentially_executable_file(blocked), "{blocked}");
        }
        for allowed in [
            "src/main.rs",
            "README.md",
            "package.json",
            "Makefile",
            "notes.txt",
        ] {
            assert!(!is_potentially_executable_file(allowed), "{allowed}");
        }
    }
}
