//! Instalación del artefacto descargado, una implementación por formato.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::LazyLock;

use regex::Regex;

use crate::download::DownloadedUpdate;
use crate::error::UpdaterError;
use crate::manifest::ArtifactKind;
use crate::platform::Env;

/// Equivalente a los argumentos de `quitAndInstall(isSilent, isForceRunAfter)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallOptions {
    /// Instalar sin interfaz.
    pub silent: bool,
    /// Volver a abrir la app al terminar.
    pub relaunch: bool,
}

/// Cómo volver a abrir la app ya actualizada. Lo ejecuta la app en su paso de
/// "salir y relanzar" (o `spawn` directamente).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Variables que se añaden al entorno heredado.
    pub env: Vec<(String, String)>,
}

impl RelaunchCommand {
    /// Lanza el proceso desligado del actual, que está a punto de salir.
    pub fn spawn(&self) -> io::Result<()> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach(&mut command);
        command.spawn().map(|_| ())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallOutcome {
    /// `None` cuando el propio instalador reabre la app (NSIS) o no se pidió.
    pub relaunch: Option<RelaunchCommand>,
}

/// Aplica una actualización ya descargada y verificada.
///
/// No cierra la app: eso lo hace el callback de salida que provee la app.
pub trait Installer: Send + Sync {
    fn install(
        &self,
        update: &DownloadedUpdate,
        options: &InstallOptions,
    ) -> Result<InstallOutcome, UpdaterError>;
}

/// Desliga el proceso hijo para que sobreviva a la salida de la app.
fn detach(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = command;
    }
}

// ---------------------------------------------------------------------------
// Windows: instalador NSIS
// ---------------------------------------------------------------------------

type InstallerLauncher = Box<dyn Fn(&Path, &[String]) -> io::Result<()> + Send + Sync>;

/// Ejecuta el instalador NSIS descargado. La app tiene que salir enseguida
/// para que el instalador pueda reemplazar sus archivos; con `--force-run` es
/// el instalador quien la vuelve a abrir.
pub struct NsisInstaller {
    launcher: InstallerLauncher,
}

impl NsisInstaller {
    pub fn new() -> Self {
        Self::with_launcher(Box::new(launch_installer))
    }

    /// Con un lanzador propio (los tests no ejecutan nada).
    pub fn with_launcher(launcher: InstallerLauncher) -> Self {
        Self { launcher }
    }
}

impl Default for NsisInstaller {
    fn default() -> Self {
        Self::new()
    }
}

/// Argumentos que `NsisUpdater` de electron-updater pasa al instalador.
pub fn nsis_install_args(options: &InstallOptions) -> Vec<String> {
    let mut args = vec!["--updated".to_string()];
    if options.silent {
        args.push("/S".to_string());
    }
    if options.relaunch {
        args.push("--force-run".to_string());
    }
    args
}

impl Installer for NsisInstaller {
    fn install(
        &self,
        update: &DownloadedUpdate,
        options: &InstallOptions,
    ) -> Result<InstallOutcome, UpdaterError> {
        if !update.path.is_file() {
            return Err(UpdaterError::install(
                "el instalador descargado ya no existe",
            ));
        }
        (self.launcher)(&update.path, &nsis_install_args(options))
            .map_err(|error| UpdaterError::install(error.to_string()))?;
        Ok(InstallOutcome { relaunch: None })
    }
}

/// `ERROR_ELEVATION_REQUIRED`: el instalador necesita permisos de administrador.
const ERROR_ELEVATION_REQUIRED: i32 = 740;

fn launch_installer(path: &Path, args: &[String]) -> io::Result<()> {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);

    match command.spawn() {
        Ok(_) => Ok(()),
        Err(error) if cfg!(windows) && error.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => {
            // Instalación para todos los usuarios: se pide elevación (UAC).
            let mut elevated = Command::new("powershell.exe");
            elevated
                .args(elevated_launch_args(path, args))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            detach(&mut elevated);
            elevated.spawn().map(|_| ())
        }
        Err(error) => Err(error),
    }
}

fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Argumentos de PowerShell para lanzar el instalador elevado.
pub(crate) fn elevated_launch_args(path: &Path, args: &[String]) -> Vec<String> {
    let mut script = format!(
        "Start-Process -FilePath {} -Verb RunAs",
        powershell_quote(&path.to_string_lossy())
    );
    if !args.is_empty() {
        let list: Vec<String> = args.iter().map(|arg| powershell_quote(arg)).collect();
        script.push_str(" -ArgumentList ");
        script.push_str(&list.join(","));
    }
    vec![
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-WindowStyle".to_string(),
        "Hidden".to_string(),
        "-Command".to_string(),
        script,
    ]
}

// ---------------------------------------------------------------------------
// Linux: AppImage
// ---------------------------------------------------------------------------

static VERSION_IN_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+\.[0-9]+\.[0-9]+").expect("regex constante"));

/// Reemplaza el AppImage en ejecución (el que apunta la variable `APPIMAGE`).
pub struct AppImageInstaller {
    appimage_path: PathBuf,
}

impl AppImageInstaller {
    pub fn new(appimage_path: impl Into<PathBuf>) -> Self {
        Self {
            appimage_path: appimage_path.into(),
        }
    }

    pub fn from_env(env: &Env) -> Option<Self> {
        env.get("APPIMAGE")
            .filter(|value| !value.is_empty())
            .map(Self::new)
    }
}

/// Dónde queda el AppImage nuevo, con el criterio de electron-updater:
// https://github.com/electron-userland/electron-builder/issues/2964
// if no version in existing file name, it means that user wants to preserve current custom name
pub fn appimage_destination(current: &Path, downloaded_file_name: &str) -> PathBuf {
    let existing_name = current
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    if downloaded_file_name == existing_name || !VERSION_IN_NAME_RE.is_match(&existing_name) {
        // no version in the file name, overwrite existing
        current.to_path_buf()
    } else {
        current.with_file_name(downloaded_file_name)
    }
}

impl Installer for AppImageInstaller {
    fn install(
        &self,
        update: &DownloadedUpdate,
        options: &InstallOptions,
    ) -> Result<InstallOutcome, UpdaterError> {
        let fail =
            |step: &str, error: &io::Error| UpdaterError::install(format!("{step}: {error}"));

        let current = &self.appimage_path;
        let destination = appimage_destination(current, &update.file_name);
        let directory = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let destination_name = destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| update.file_name.clone());

        // La descarga está en la caché, que puede ser otro sistema de archivos:
        // se crea un temporal nuevo con `create_new` (nunca sigue symlinks ni
        // usa un nombre predecible) junto al destino y se renombra encima.
        let (staged, mut staged_file) = create_unique_staged_file(&directory, &destination_name)
            .map_err(|error| fail("no se pudo crear el temporal del AppImage nuevo", &error))?;

        let result = (|| {
            let mut source = fs::File::open(&update.path)
                .map_err(|error| fail("no se pudo leer el AppImage descargado", &error))?;
            io::copy(&mut source, &mut staged_file)
                .and_then(|_| staged_file.sync_all())
                .map_err(|error| fail("no se pudo copiar el AppImage nuevo", &error))?;
            drop(staged_file);
            apply_executable_permissions(current, &staged)
                .map_err(|error| fail("no se pudieron ajustar los permisos", &error))?;
            fs::rename(&staged, &destination)
                .map_err(|error| fail("no se pudo reemplazar el AppImage", &error))
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }

        // El nombre llevaba la versión vieja: el archivo anterior ya no pinta nada.
        if destination != *current
            && let Err(error) = fs::remove_file(current)
            && error.kind() != io::ErrorKind::NotFound
        {
            log::warn!(
                "[updater] could not remove the previous AppImage {}: {error}",
                current.display()
            );
        }

        let relaunch = options.relaunch.then(|| RelaunchCommand {
            program: destination,
            args: Vec::new(),
            env: vec![("APPIMAGE_SILENT_INSTALL".to_string(), "true".to_string())],
        });
        Ok(InstallOutcome { relaunch })
    }
}

/// Crea un archivo temporal nuevo junto al AppImage destino con `create_new`
/// (nunca sobrescribe ni sigue un enlace simbólico preexistente).
fn create_unique_staged_file(
    directory: &Path,
    destination_name: &str,
) -> io::Result<(PathBuf, fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());

    let mut last_error = None;
    for _ in 0..32 {
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        let staged = directory.join(format!(".{destination_name}.update-{pid}-{nanos:x}-{seq}"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&staged) {
            Ok(file) => return Ok((staged, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("could not create staged AppImage file")))
}

/// El AppImage nuevo conserva los permisos del anterior y siempre queda ejecutable.
#[cfg(unix)]
fn apply_executable_permissions(current: &Path, staged: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = match fs::metadata(current) {
        Ok(meta) => (meta.permissions().mode() & 0o7777) | 0o100,
        Err(_) => 0o755,
    };
    fs::set_permissions(staged, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_executable_permissions(_current: &Path, _staged: &Path) -> io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// macOS: zip con el bundle .app
// ---------------------------------------------------------------------------

type ArchiveExtractor = Box<dyn Fn(&Path, &Path) -> Result<(), String> + Send + Sync>;

/// Extrae el zip y cambia el bundle `.app` en ejecución por el nuevo.
pub struct MacAppInstaller {
    bundle_path: PathBuf,
    extractor: ArchiveExtractor,
}

impl MacAppInstaller {
    pub fn new(bundle_path: impl Into<PathBuf>) -> Self {
        Self::with_extractor(bundle_path, Box::new(extract_with_ditto))
    }

    /// Con un extractor propio (los tests no tienen `ditto`).
    pub fn with_extractor(bundle_path: impl Into<PathBuf>, extractor: ArchiveExtractor) -> Self {
        Self {
            bundle_path: bundle_path.into(),
            extractor,
        }
    }

    /// Para el bundle que contiene al ejecutable actual, si corre desde uno.
    pub fn for_current_executable() -> Option<Self> {
        let executable = std::env::current_exe().ok()?;
        bundle_path_for_executable(&executable).map(Self::new)
    }
}

/// `/Applications/Mergeon.app/Contents/MacOS/mergeon` → `/Applications/Mergeon.app`.
pub fn bundle_path_for_executable(executable: &Path) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    let contents_dir = macos_dir.parent()?;
    let bundle = contents_dir.parent()?;

    let is_bundle = macos_dir.file_name().is_some_and(|name| name == "MacOS")
        && contents_dir
            .file_name()
            .is_some_and(|name| name == "Contents")
        && bundle
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"));
    is_bundle.then(|| bundle.to_path_buf())
}

/// `ditto` es lo que usa el sistema para estos zip: conserva enlaces
/// simbólicos, permisos y la firma del bundle.
fn extract_with_ditto(archive: &Path, destination: &Path) -> Result<(), String> {
    let output = Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(archive)
        .arg(destination)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("no se pudo ejecutar ditto: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ditto falló: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Busca el `.app` extraído: el que se llama como el bundle actual o, si no, el único que haya.
fn find_extracted_bundle(staging: &Path, bundle_name: &std::ffi::OsStr) -> Option<PathBuf> {
    let same_name = staging.join(bundle_name);
    if same_name.is_dir() {
        return Some(same_name);
    }
    let mut bundles: Vec<PathBuf> = fs::read_dir(staging)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        })
        .collect();
    bundles.sort();
    bundles.into_iter().next()
}

impl Installer for MacAppInstaller {
    fn install(
        &self,
        update: &DownloadedUpdate,
        options: &InstallOptions,
    ) -> Result<InstallOutcome, UpdaterError> {
        let bundle = &self.bundle_path;
        let bundle_text = bundle.to_string_lossy();
        // Gatekeeper ejecuta las apps en cuarentena desde una copia de solo lectura.
        if bundle_text.contains("/AppTranslocation/") {
            return Err(UpdaterError::install(
                "la app se está ejecutando desde una ubicación temporal; muévela a Aplicaciones",
            ));
        }
        let (Some(parent), Some(bundle_name)) = (bundle.parent(), bundle.file_name()) else {
            return Err(UpdaterError::install("no se encontró el bundle de la app"));
        };
        if !bundle.is_dir() {
            return Err(UpdaterError::install("no se encontró el bundle de la app"));
        }

        let name = bundle_name.to_string_lossy();
        let pid = std::process::id();
        // Junto al bundle (mismo volumen) para que los renombrados sean atómicos.
        let staging = parent.join(format!(".{name}.update-{pid}"));
        let backup = parent.join(format!(".{name}.previous-{pid}"));
        let cleanup = |path: &Path| {
            if path.exists()
                && let Err(error) = fs::remove_dir_all(path)
            {
                log::warn!("[updater] could not remove {}: {error}", path.display());
            }
        };

        cleanup(&staging);
        cleanup(&backup);
        fs::create_dir_all(&staging).map_err(|error| {
            UpdaterError::install(format!("no se pudo preparar la actualización: {error}"))
        })?;

        if let Err(detail) = (self.extractor)(&update.path, &staging) {
            cleanup(&staging);
            return Err(UpdaterError::install(detail));
        }
        let Some(extracted) = find_extracted_bundle(&staging, bundle_name) else {
            cleanup(&staging);
            return Err(UpdaterError::install("el zip no contiene un bundle .app"));
        };

        // El bundle actual se aparta en vez de borrarse: si el nuevo no entra,
        // se devuelve a su sitio y la app sigue como estaba.
        if let Err(error) = fs::rename(bundle, &backup) {
            cleanup(&staging);
            return Err(UpdaterError::install(format!(
                "no se pudo apartar la versión actual: {error}"
            )));
        }
        if let Err(error) = fs::rename(&extracted, bundle) {
            let restored = fs::rename(&backup, bundle);
            cleanup(&staging);
            return Err(UpdaterError::install(match restored {
                Ok(()) => format!("no se pudo colocar la versión nueva: {error}"),
                Err(restore_error) => format!(
                    "no se pudo colocar la versión nueva ({error}) ni restaurar la anterior ({restore_error})"
                ),
            }));
        }

        cleanup(&backup);
        cleanup(&staging);

        let relaunch = options.relaunch.then(|| RelaunchCommand {
            program: PathBuf::from("/usr/bin/open"),
            args: vec!["-n".to_string(), bundle.to_string_lossy().into_owned()],
            env: Vec::new(),
        });
        Ok(InstallOutcome { relaunch })
    }
}

/// Instalador real para el formato dado, con los datos del proceso actual.
pub fn default_installer(kind: ArtifactKind, env: &Env) -> Option<Arc<dyn Installer>> {
    match kind {
        ArtifactKind::Nsis => Some(Arc::new(NsisInstaller::new())),
        ArtifactKind::AppImage => AppImageInstaller::from_env(env)
            .map(|installer| Arc::new(installer) as Arc<dyn Installer>),
        ArtifactKind::MacZip => MacAppInstaller::for_current_executable()
            .map(|installer| Arc::new(installer) as Arc<dyn Installer>),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const RELAUNCH: InstallOptions = InstallOptions {
        silent: true,
        relaunch: true,
    };
    const ON_QUIT: InstallOptions = InstallOptions {
        silent: true,
        relaunch: false,
    };

    fn downloaded(dir: &Path, file_name: &str, contents: &[u8]) -> DownloadedUpdate {
        let path = dir.join(file_name);
        fs::write(&path, contents).unwrap();
        DownloadedUpdate {
            version: "0.9.1".to_string(),
            path,
            file_name: file_name.to_string(),
            sha512: String::new(),
            size: contents.len() as u64,
        }
    }

    #[test]
    fn builds_the_nsis_arguments_electron_updater_used() {
        assert_eq!(
            nsis_install_args(&RELAUNCH),
            ["--updated", "/S", "--force-run"]
        );
        assert_eq!(nsis_install_args(&ON_QUIT), ["--updated", "/S"]);
        assert_eq!(
            nsis_install_args(&InstallOptions {
                silent: false,
                relaunch: true
            }),
            ["--updated", "--force-run"]
        );
    }

    #[test]
    fn nsis_installer_launches_the_downloaded_installer_silently() {
        let cache = tempfile::tempdir().unwrap();
        let update = downloaded(cache.path(), "Mergeon-Setup-0.9.1.exe", b"MZ");
        type Launches = Vec<(PathBuf, Vec<String>)>;
        let launched: Arc<Mutex<Launches>> = Arc::default();
        let recorder = Arc::clone(&launched);
        let installer = NsisInstaller::with_launcher(Box::new(move |path, args| {
            recorder
                .lock()
                .unwrap()
                .push((path.to_path_buf(), args.to_vec()));
            Ok(())
        }));

        let outcome = installer.install(&update, &RELAUNCH).unwrap();

        // El instalador reabre la app: no hay nada que relanzar desde aquí.
        assert_eq!(outcome.relaunch, None);
        assert_eq!(
            *launched.lock().unwrap(),
            vec![(
                update.path.clone(),
                vec![
                    "--updated".to_string(),
                    "/S".to_string(),
                    "--force-run".to_string()
                ]
            )]
        );
    }

    #[test]
    fn nsis_installer_reports_a_missing_or_unlaunchable_installer() {
        let cache = tempfile::tempdir().unwrap();
        let mut update = downloaded(cache.path(), "Mergeon-Setup-0.9.1.exe", b"MZ");

        let failing = NsisInstaller::with_launcher(Box::new(|_, _| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "access denied",
            ))
        }));
        assert_eq!(
            failing.install(&update, &RELAUNCH),
            Err(UpdaterError::install("access denied"))
        );

        update.path = cache.path().join("gone.exe");
        let never = NsisInstaller::with_launcher(Box::new(|_, _| panic!("no debe lanzarse")));
        assert!(matches!(
            never.install(&update, &RELAUNCH),
            Err(UpdaterError::Install { .. })
        ));
    }

    #[test]
    fn quotes_the_elevated_powershell_command() {
        let args = elevated_launch_args(
            Path::new("C:\\Users\\O'Brien\\Mergeon Setup.exe"),
            &["--updated".to_string(), "/S".to_string()],
        );
        assert_eq!(
            args[..5],
            [
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command"
            ]
        );
        assert_eq!(
            args[5],
            "Start-Process -FilePath 'C:\\Users\\O''Brien\\Mergeon Setup.exe' -Verb RunAs -ArgumentList '--updated','/S'"
        );
        assert_eq!(
            elevated_launch_args(Path::new("a.exe"), &[])[5],
            "Start-Process -FilePath 'a.exe' -Verb RunAs"
        );
    }

    #[test]
    fn keeps_a_custom_appimage_name_and_follows_versioned_ones() {
        // Sin versión en el nombre: el usuario lo renombró, se conserva.
        assert_eq!(
            appimage_destination(
                Path::new("/opt/apps/Mergeon.AppImage"),
                "Mergeon-0.9.1.AppImage"
            ),
            PathBuf::from("/opt/apps/Mergeon.AppImage")
        );
        // Con versión: el archivo pasa a llamarse como el nuevo.
        assert_eq!(
            appimage_destination(
                Path::new("/opt/apps/Mergeon-0.9.0.AppImage"),
                "Mergeon-0.9.1.AppImage"
            ),
            PathBuf::from("/opt/apps/Mergeon-0.9.1.AppImage")
        );
        assert_eq!(
            appimage_destination(
                Path::new("/opt/apps/Mergeon-0.9.1.AppImage"),
                "Mergeon-0.9.1.AppImage"
            ),
            PathBuf::from("/opt/apps/Mergeon-0.9.1.AppImage")
        );
    }

    #[cfg(unix)]
    #[test]
    fn replaces_the_appimage_in_place_preserving_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let cache = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let current = apps.path().join("Mergeon.AppImage");
        fs::write(&current, b"old").unwrap();
        fs::set_permissions(&current, fs::Permissions::from_mode(0o750)).unwrap();
        let update = downloaded(cache.path(), "Mergeon-0.9.1.AppImage", b"new build");
        // La descarga llega sin bit de ejecución.
        fs::set_permissions(&update.path, fs::Permissions::from_mode(0o600)).unwrap();

        let outcome = AppImageInstaller::new(&current)
            .install(&update, &RELAUNCH)
            .unwrap();

        assert_eq!(fs::read(&current).unwrap(), b"new build");
        assert_eq!(
            fs::metadata(&current).unwrap().permissions().mode() & 0o7777,
            0o750
        );
        assert_eq!(
            outcome.relaunch,
            Some(RelaunchCommand {
                program: current.clone(),
                args: Vec::new(),
                env: vec![("APPIMAGE_SILENT_INSTALL".to_string(), "true".to_string())],
            })
        );
        // No queda ningún temporal junto al AppImage y la caché sigue intacta.
        let entries: Vec<_> = fs::read_dir(apps.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [std::ffi::OsString::from("Mergeon.AppImage")]);
        assert!(update.path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn renames_a_versioned_appimage_and_makes_it_executable() {
        use std::os::unix::fs::PermissionsExt;

        let cache = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let current = apps.path().join("Mergeon-0.9.0.AppImage");
        fs::write(&current, b"old").unwrap();
        // Aunque el anterior hubiera perdido el bit de ejecución, el nuevo lo lleva.
        fs::set_permissions(&current, fs::Permissions::from_mode(0o644)).unwrap();
        let update = downloaded(cache.path(), "Mergeon-0.9.1.AppImage", b"new build");

        let outcome = AppImageInstaller::new(&current)
            .install(&update, &ON_QUIT)
            .unwrap();

        let destination = apps.path().join("Mergeon-0.9.1.AppImage");
        assert_eq!(fs::read(&destination).unwrap(), b"new build");
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o7777,
            0o744
        );
        assert!(!current.exists());
        // Instalación al salir: no se relanza.
        assert_eq!(outcome.relaunch, None);
    }

    #[test]
    fn appimage_install_failure_leaves_the_current_file_untouched() {
        let cache = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let current = apps.path().join("Mergeon.AppImage");
        fs::write(&current, b"old").unwrap();
        let mut update = downloaded(cache.path(), "Mergeon-0.9.1.AppImage", b"new build");
        update.path = cache.path().join("missing.AppImage");

        let result = AppImageInstaller::new(&current).install(&update, &RELAUNCH);

        assert!(matches!(result, Err(UpdaterError::Install { .. })));
        assert_eq!(fs::read(&current).unwrap(), b"old");
        assert_eq!(fs::read_dir(apps.path()).unwrap().count(), 1);
    }

    #[test]
    fn appimage_installer_needs_the_appimage_variable() {
        assert!(AppImageInstaller::from_env(&Env::new()).is_none());
        assert!(AppImageInstaller::from_env(&Env::from_pairs([("APPIMAGE", "")])).is_none());
        assert!(
            AppImageInstaller::from_env(&Env::from_pairs([("APPIMAGE", "/opt/Mergeon.AppImage")]))
                .is_some()
        );
    }

    #[test]
    fn finds_the_bundle_that_contains_the_executable() {
        assert_eq!(
            bundle_path_for_executable(Path::new(
                "/Applications/Mergeon.app/Contents/MacOS/mergeon"
            )),
            Some(PathBuf::from("/Applications/Mergeon.app"))
        );
        assert_eq!(
            bundle_path_for_executable(Path::new("/usr/local/bin/mergeon")),
            None
        );
        assert_eq!(
            bundle_path_for_executable(Path::new("/tmp/build/Contents/MacOS/mergeon")),
            None
        );
    }

    /// Bundle mínimo con un archivo que identifica la versión.
    fn write_bundle(path: &Path, marker: &str) {
        let macos = path.join("Contents").join("MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(macos.join("mergeon"), marker).unwrap();
    }

    fn bundle_marker(path: &Path) -> String {
        fs::read_to_string(path.join("Contents").join("MacOS").join("mergeon")).unwrap()
    }

    /// Extractor falso: "extrae" un bundle con el contenido del zip como marca.
    fn fake_extractor(bundle_name: &'static str) -> ArchiveExtractor {
        Box::new(move |archive, destination| {
            let marker = fs::read_to_string(archive).map_err(|error| error.to_string())?;
            write_bundle(&destination.join(bundle_name), &marker);
            Ok(())
        })
    }

    #[test]
    fn swaps_the_mac_bundle_for_the_extracted_one() {
        let cache = tempfile::tempdir().unwrap();
        let applications = tempfile::tempdir().unwrap();
        let bundle = applications.path().join("Mergeon.app");
        write_bundle(&bundle, "old");
        let update = downloaded(cache.path(), "Mergeon-0.9.1-mac.zip", b"new");

        let outcome = MacAppInstaller::with_extractor(&bundle, fake_extractor("Mergeon.app"))
            .install(&update, &RELAUNCH)
            .unwrap();

        assert_eq!(bundle_marker(&bundle), "new");
        assert_eq!(
            outcome.relaunch,
            Some(RelaunchCommand {
                program: PathBuf::from("/usr/bin/open"),
                args: vec!["-n".to_string(), bundle.to_string_lossy().into_owned()],
                env: Vec::new(),
            })
        );
        // Ni copia de respaldo ni directorio de extracción quedan atrás.
        assert_eq!(fs::read_dir(applications.path()).unwrap().count(), 1);
    }

    #[test]
    fn accepts_a_zip_whose_bundle_has_another_name() {
        let cache = tempfile::tempdir().unwrap();
        let applications = tempfile::tempdir().unwrap();
        let bundle = applications.path().join("Mergeon Beta.app");
        write_bundle(&bundle, "old");
        let update = downloaded(cache.path(), "Mergeon-0.9.1-mac.zip", b"new");

        MacAppInstaller::with_extractor(&bundle, fake_extractor("Mergeon.app"))
            .install(&update, &ON_QUIT)
            .unwrap();

        assert_eq!(bundle_marker(&bundle), "new");
        assert_eq!(fs::read_dir(applications.path()).unwrap().count(), 1);
    }

    #[test]
    fn keeps_the_current_bundle_when_the_archive_is_unusable() {
        let cache = tempfile::tempdir().unwrap();
        let applications = tempfile::tempdir().unwrap();
        let bundle = applications.path().join("Mergeon.app");
        write_bundle(&bundle, "old");
        let update = downloaded(cache.path(), "Mergeon-0.9.1-mac.zip", b"new");

        let broken = MacAppInstaller::with_extractor(
            &bundle,
            Box::new(|_, _| Err("ditto falló: not a zip".to_string())),
        );
        assert_eq!(
            broken.install(&update, &RELAUNCH),
            Err(UpdaterError::install("ditto falló: not a zip"))
        );

        // Un zip sin bundle dentro tampoco toca nada.
        let empty = MacAppInstaller::with_extractor(&bundle, Box::new(|_, _| Ok(())));
        assert_eq!(
            empty.install(&update, &RELAUNCH),
            Err(UpdaterError::install("el zip no contiene un bundle .app"))
        );

        assert_eq!(bundle_marker(&bundle), "old");
        assert_eq!(fs::read_dir(applications.path()).unwrap().count(), 1);
    }

    #[test]
    fn refuses_to_update_a_translocated_or_missing_bundle() {
        let cache = tempfile::tempdir().unwrap();
        let update = downloaded(cache.path(), "Mergeon-0.9.1-mac.zip", b"new");
        let never: fn() -> ArchiveExtractor = || Box::new(|_, _| panic!("no debe extraerse nada"));

        let translocated = MacAppInstaller::with_extractor(
            "/private/var/folders/ab/T/AppTranslocation/1234/d/Mergeon.app",
            never(),
        );
        assert!(matches!(
            translocated.install(&update, &RELAUNCH),
            Err(UpdaterError::Install { .. })
        ));

        let missing = MacAppInstaller::with_extractor(cache.path().join("Nope.app"), never());
        assert_eq!(
            missing.install(&update, &RELAUNCH),
            Err(UpdaterError::install("no se encontró el bundle de la app"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn relaunch_command_spawns_a_detached_process_with_the_extra_env() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.txt");
        let command = RelaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".to_string(),
                format!(
                    "printf %s \"$APPIMAGE_SILENT_INSTALL\" > '{}'",
                    output.display()
                ),
            ],
            env: vec![("APPIMAGE_SILENT_INSTALL".to_string(), "true".to_string())],
        };

        command.spawn().unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while fs::read_to_string(&output)
            .map(|text| text != "true")
            .unwrap_or(true)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "el proceso no escribió nada"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn default_installer_follows_the_environment() {
        assert!(default_installer(ArtifactKind::Nsis, &Env::new()).is_some());
        assert!(default_installer(ArtifactKind::AppImage, &Env::new()).is_none());
        assert!(
            default_installer(
                ArtifactKind::AppImage,
                &Env::from_pairs([("APPIMAGE", "/opt/Mergeon.AppImage")])
            )
            .is_some()
        );
        // Los tests no corren dentro de un bundle .app.
        assert!(default_installer(ArtifactKind::MacZip, &Env::new()).is_none());
    }
}
