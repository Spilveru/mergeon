//! `AppUpdater` de punta a punta con transporte e instalador falsos: el port
//! de `electron/services/app-updater.test.js` más el ciclo completo
//! check → download → install que antes hacía electron-updater.

mod common;

use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mergeon_updater::testing::{FakeResponse, FakeTransport, RecordingInstaller};
use mergeon_updater::{
    AppImageInstaller, AppUpdater, Arch, ArtifactKind, BUILT_IN_UPDATE_FEED_URL, CheckOptions,
    DEFAULT_UPDATE_CHANNEL, Env, ErrorPhase, HttpRequest, HttpResponse, HttpTransport,
    InstallFailureReason, InstallOptions, InstallOutcome, InstallResult, Platform, QuitRequest,
    RELEASES_URL, RelaunchCommand, TransportError, UpdateFeed, UpdateStatus, UpdaterConfig,
    UpdaterError, UpdaterState, sha512_base64,
};

use common::{
    EventLog, FEED_URL, WAIT, config, manifest, payload, settle, test_private_key, test_public_key,
};

const EXE: &str = "Mergeon-Setup-0.2.0.exe";
const APPIMAGE: &str = "Mergeon-0.2.0.AppImage";

/// Lo que la app recibe en su paso "salir y relanzar".
#[derive(Clone, Default)]
struct QuitLog(Arc<Mutex<Vec<QuitRequest>>>);

impl QuitLog {
    fn requests(&self) -> Vec<QuitRequest> {
        self.0.lock().unwrap().clone()
    }
}

struct Harness {
    updater: AppUpdater,
    transport: Arc<FakeTransport>,
    installer: Arc<RecordingInstaller>,
    events: EventLog,
    quits: QuitLog,
}

fn harness_with(
    config: UpdaterConfig,
    transport: Arc<FakeTransport>,
    installer: RecordingInstaller,
) -> Harness {
    let installer = Arc::new(installer);
    let quits = QuitLog::default();
    let sink = quits.clone();
    let updater = AppUpdater::builder(config)
        .transport(transport.clone())
        .installer(installer.clone())
        .on_quit(move |request| sink.0.lock().unwrap().push(request.clone()))
        .build();
    let events = EventLog::attach(&updater);
    Harness {
        updater,
        transport,
        installer,
        events,
        quits,
    }
}

fn new_harness(config: UpdaterConfig) -> Harness {
    harness_with(
        config,
        Arc::new(FakeTransport::new().with_signing_key(test_private_key())),
        RecordingInstaller::new(),
    )
}

/// Windows con la versión 0.2.0 publicada en el feed.
fn windows_with_update(cache: &tempfile::TempDir) -> (Harness, Vec<u8>) {
    let data = payload(150_000);
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    harness.transport.serve_file(EXE, data.clone());
    (harness, data)
}

fn version(text: &str) -> Option<String> {
    Some(text.to_string())
}

// --- app-updater.test.js -----------------------------------------------------

#[test]
fn configures_the_generic_updater_feed_for_packaged_builds() {
    let cache = tempfile::tempdir().unwrap();
    let config = UpdaterConfig::from_env(
        version("0.1.6"),
        true,
        Platform::Darwin,
        Arch::Arm64,
        &Env::from_pairs([("APP_UPDATE_FEED_URL", "https://updates.example.com/mac")]),
        cache.path(),
    );

    assert_eq!(
        config.feed,
        Some(UpdateFeed::generic("https://updates.example.com/mac"))
    );
    assert_eq!(config.channel, "latest");
    assert_eq!(config.channel, DEFAULT_UPDATE_CHANNEL);
    assert!(config.updates_configured());
    // `autoInstallOnAppQuit = true` y comprobación al arrancar + cada 4 horas.
    assert!(config.auto_install_on_app_quit);
    assert!(config.check_on_start);
    assert_eq!(
        config.check_interval,
        Some(Duration::from_secs(4 * 60 * 60))
    );
    assert_eq!(config.releases_url, RELEASES_URL);

    let harness = new_harness(config);
    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Idle);
    assert_eq!(state.current_version, version("0.1.6"));
    assert!(state.updates_configured);
    assert!(state.is_packaged);

    // El manifiesto se pide al feed genérico, con el nombre del canal para macOS.
    harness.updater.check(CheckOptions::default());
    let requests = harness.transport.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .url
            .starts_with("https://updates.example.com/mac/latest-mac.yml?noCache="),
        "{}",
        requests[0].url
    );
    assert_eq!(requests[0].header("cache-control"), Some("no-cache"));
}

#[test]
fn preserves_the_latest_version_from_update_not_available_for_manual_checks() {
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Darwin, "0.1.6", &[], cache.path()));
    harness.transport.serve(
        "latest-mac.yml",
        manifest("0.1.6", &[("Mergeon-0.1.6-mac.zip", b"zip")]),
    );

    let returned = harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    let state = harness.updater.state();
    assert_eq!(returned, state);
    assert_eq!(state.status, UpdateStatus::NotAvailable);
    assert_eq!(state.current_version, version("0.1.6"));
    assert_eq!(state.latest_version, version("0.1.6"));
    assert!(!state.manual_check);

    // Los eventos sí llevan la marca: con ella la interfaz muestra el aviso.
    let events = harness.events.events();
    assert_eq!(
        harness.events.statuses(),
        [UpdateStatus::Checking, UpdateStatus::NotAvailable]
    );
    assert!(events.iter().all(|event| event.manual_check));
    assert!(harness.installer.calls().is_empty());
}

/*
 * El feed fijado al compilar (`option_env!("APP_UPDATE_FEED_URL")`) es el
 * respaldo cuando el entorno de ejecución no trae ninguno. Antes solo el
 * AppImage tenía canal (su `AppRun` exporta la variable): el `.exe`, el `.deb`
 * y el `.app` se quedaban sin actualizaciones.
 */
#[test]
fn falls_back_to_the_feed_fixed_at_build_time() {
    let cache = tempfile::tempdir().unwrap();
    let built_in = Some("https://updates.example.com/built-in");
    let build = |env: &Env| {
        UpdaterConfig::from_env_with_built_in_feed(
            version("0.2.1"),
            true,
            Platform::Win32,
            Arch::X64,
            env,
            built_in,
            cache.path(),
        )
    };

    let config = build(&Env::new());
    assert_eq!(
        config.feed,
        Some(UpdateFeed::generic("https://updates.example.com/built-in"))
    );
    assert!(config.updates_configured());

    let config = build(&Env::from_pairs([(
        "APP_UPDATE_FEED_URL",
        "https://updates.example.com/runtime",
    )]));
    assert_eq!(
        config.feed,
        Some(UpdateFeed::generic("https://updates.example.com/runtime"))
    );

    // `from_env` usa el que se fijó al compilar este binario, si se fijó alguno.
    assert_eq!(BUILT_IN_UPDATE_FEED_URL, option_env!("APP_UPDATE_FEED_URL"));
    let compiled = UpdaterConfig::from_env(
        version("0.2.1"),
        true,
        Platform::Win32,
        Arch::X64,
        &Env::new(),
        cache.path(),
    );
    assert_eq!(
        compiled.feed,
        BUILT_IN_UPDATE_FEED_URL
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(UpdateFeed::generic)
    );
}

#[test]
fn reports_no_update_channel_instead_of_querying_the_private_github_feed() {
    let cache = tempfile::tempdir().unwrap();
    // Sin feed de respaldo: el test no depende de cómo se compiló.
    let config = UpdaterConfig::from_env_with_built_in_feed(
        version("0.2.1"),
        true,
        Platform::Win32,
        Arch::X64,
        &Env::new(),
        None,
        cache.path(),
    );
    assert_eq!(config.feed, None);
    let harness = new_harness(config);

    let state = harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    assert_eq!(state.status, UpdateStatus::NotConfigured);
    assert!(!state.updates_configured);
    assert_eq!(state.error, None);
    // Sin canal no se consulta nada: el 404 del repo privado ni se provoca.
    assert!(harness.transport.requests().is_empty());

    let events = harness.events.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, UpdateStatus::NotConfigured);
    assert!(events[0].manual_check);
    assert!(!harness.updater.state().manual_check);
}

#[test]
fn emits_the_condensed_message_to_the_renderer() {
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Darwin, "0.2.1", &[], cache.path()));
    // El feed no tiene manifiesto: responde 404, como el repo privado.

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    let expected = "No se encontró el canal de actualizaciones (404). Revisa que las releases del repositorio sean públicas.";
    assert_eq!(harness.updater.state().error.as_deref(), Some(expected));
    let last = harness.events.events().pop().unwrap();
    assert_eq!(last.status, UpdateStatus::Error);
    assert_eq!(last.error.as_deref(), Some(expected));
    assert_eq!(last.error_phase, Some(ErrorPhase::Check));
}

#[test]
fn classifies_a_failed_check_separately_from_a_failed_download() {
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Win32, "0.1.9", &[], cache.path()));
    let data = payload(4_000);

    // Al día, y luego el feed deja de responder: es un fallo de comprobación.
    harness.transport.serve(
        "latest.yml",
        manifest("0.1.9", &[("Mergeon-Setup-0.1.9.exe", b"old")]),
    );
    harness.updater.check(CheckOptions::default());
    harness.transport.serve_status("latest.yml", 500);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.latest_version, version("0.1.9"));
    assert_eq!(state.error_phase, Some(ErrorPhase::Check));
    assert_eq!(
        state.error.as_deref(),
        Some("El servidor de actualizaciones respondió 500.")
    );

    // Hay versión nueva pero el instalador no baja: es un fallo de descarga.
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    harness.transport.serve_status(EXE, 403);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.latest_version, version("0.2.0"));
    assert_eq!(state.error_phase, Some(ErrorPhase::Download));
    assert_eq!(
        state.error.as_deref(),
        Some("El servidor de actualizaciones rechazó la petición (403).")
    );
}

#[test]
fn leaves_restart_decisions_to_the_in_app_updater_indicator() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);

    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    // Descargada, pero nadie instala ni cierra la app hasta que el usuario lo pida.
    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Downloaded);
    assert_eq!(state.latest_version, version("0.2.0"));
    assert!(harness.installer.calls().is_empty());
    assert!(harness.quits.requests().is_empty());
}

#[test]
fn does_not_install_when_the_download_came_from_a_background_check() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    assert_eq!(harness.updater.state().status, UpdateStatus::Downloaded);
    assert!(
        harness
            .events
            .events()
            .iter()
            .all(|event| !event.manual_check)
    );
    assert!(harness.installer.calls().is_empty());
    assert!(harness.quits.requests().is_empty());
}

#[test]
fn emits_status_events_to_the_renderer_and_tracks_them_in_get_state() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    let events = harness.events.events();
    assert_eq!(
        harness.events.status_sequence(),
        [
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
        ]
    );

    let downloading: Vec<&UpdaterState> = events
        .iter()
        .filter(|event| event.status == UpdateStatus::Downloading)
        .collect();
    assert!(downloading.len() >= 3, "{downloading:?}");
    for event in &downloading {
        assert_eq!(event.latest_version, version("0.2.0"));
        assert!(event.can_auto_install);
        assert!(event.percent <= 100);
    }
    // 150 000 bytes en bloques de 64 KiB: 0 %, 44 %, 87 %, 100 %.
    let percents: Vec<u8> = downloading.iter().map(|event| event.percent).collect();
    assert_eq!(percents, [0, 44, 87, 100]);

    // El último payload es exactamente el estado que devuelve `state()`.
    assert_eq!(events.last(), Some(&harness.updater.state()));
    assert_eq!(harness.updater.state().percent, 100);
}

#[test]
fn install_update_quits_and_installs_only_once_an_update_is_downloaded_on_a_supported_platform() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, data) = windows_with_update(&cache);

    assert_eq!(harness.updater.install(), InstallResult::unsupported());
    assert!(harness.installer.calls().is_empty());

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    assert_eq!(harness.updater.install(), InstallResult::ok());

    // `quitAndInstall`: el instalador recibe el archivo verificado...
    let calls = harness.installer.calls();
    assert_eq!(calls.len(), 1);
    let (update, options) = &calls[0];
    assert_eq!(update.version, "0.2.0");
    assert_eq!(update.file_name, EXE);
    assert_eq!(update.path, cache.path().join("pending").join(EXE));
    assert_eq!(fs::read(&update.path).unwrap(), data);
    assert_eq!(update.sha512, sha512_base64(&data));
    assert_eq!(
        *options,
        InstallOptions {
            silent: true,
            relaunch: true
        }
    );
    // ...y la app, la orden de salir.
    assert_eq!(harness.quits.requests(), [QuitRequest { relaunch: None }]);
}

#[test]
fn reports_development_and_unconfigured_builds_without_attempting_an_update() {
    let cache = tempfile::tempdir().unwrap();

    let dev = new_harness(UpdaterConfig::from_env_with_built_in_feed(
        version("0.1.6"),
        false,
        Platform::Linux,
        Arch::X64,
        &Env::new(),
        None,
        cache.path(),
    ));
    let state = dev.updater.check(CheckOptions::manual());
    assert_eq!(state.status, UpdateStatus::Unsupported);
    assert!(!state.is_packaged);
    assert!(dev.transport.requests().is_empty());

    // Un build de desarrollo tampoco consulta aunque tenga feed.
    let mut dev_config = config(Platform::Linux, "0.1.6", &[], cache.path());
    dev_config.is_packaged = false;
    let dev_with_feed = new_harness(dev_config);
    assert_eq!(
        dev_with_feed.updater.check(CheckOptions::default()).status,
        UpdateStatus::Unsupported
    );
    assert!(dev_with_feed.transport.requests().is_empty());

    let packaged = new_harness(UpdaterConfig::from_env_with_built_in_feed(
        version("0.1.6"),
        true,
        Platform::Linux,
        Arch::X64,
        &Env::new(),
        None,
        cache.path(),
    ));
    let state = packaged.updater.check(CheckOptions::manual());
    assert_eq!(state.status, UpdateStatus::NotConfigured);
    assert!(!state.updates_configured);
    assert!(packaged.transport.requests().is_empty());
}

// --- Ciclo completo por plataforma -------------------------------------------

#[test]
fn windows_checks_downloads_verifies_and_installs() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, data) = windows_with_update(&cache);

    let returned = harness.updater.check(CheckOptions::manual());
    // `check` vuelve al terminar la consulta; la descarga sigue por su cuenta.
    assert!(
        matches!(
            returned.status,
            UpdateStatus::Available | UpdateStatus::Downloading | UpdateStatus::Downloaded
        ),
        "{returned:?}"
    );
    settle(&harness.updater);

    assert_eq!(
        harness.transport.requested_paths(),
        [
            format!("{FEED_URL}/latest.yml"),
            format!("{FEED_URL}/latest.yml.sig"),
            format!("{FEED_URL}/{EXE}"),
        ]
    );
    let downloaded = cache.path().join("pending").join(EXE);
    assert_eq!(fs::read(&downloaded).unwrap(), data);

    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Downloaded);
    assert_eq!(state.percent, 100);
    assert_eq!(state.error, None);
    assert!(state.can_auto_install);
    // La marca manual viaja hasta el evento `downloaded` y ahí se limpia.
    assert!(!state.manual_check);
    assert!(
        harness
            .events
            .events()
            .iter()
            .all(|event| event.manual_check)
    );

    assert_eq!(harness.updater.install(), InstallResult::ok());
    assert_eq!(harness.installer.calls().len(), 1);
    assert_eq!(harness.quits.requests().len(), 1);
}

#[cfg(unix)]
#[test]
fn linux_appimage_is_replaced_and_relaunched() {
    let cache = tempfile::tempdir().unwrap();
    let apps = tempfile::tempdir().unwrap();
    let current = apps.path().join("Mergeon.AppImage");
    fs::write(&current, b"old build").unwrap();
    let current_text = current.to_string_lossy().into_owned();
    let data = payload(80_000);
    let deb = payload(500);

    let config = config(
        Platform::Linux,
        "0.1.6",
        &[("APPIMAGE", current_text.as_str())],
        cache.path(),
    );
    assert!(config.can_auto_install);
    assert_eq!(config.artifact_kind, Some(ArtifactKind::AppImage));

    // Con el instalador real de AppImage, no con el de prueba.
    let transport = Arc::new(FakeTransport::new().with_signing_key(test_private_key()));
    transport.serve(
        "latest-linux.yml",
        manifest(
            "0.2.0",
            &[("mergeon_0.2.0_amd64.deb", &deb), (APPIMAGE, &data)],
        ),
    );
    transport.serve_file(APPIMAGE, data.clone());
    let quits = QuitLog::default();
    let sink = quits.clone();
    let updater = AppUpdater::builder(config)
        .transport(transport.clone())
        .installer(Arc::new(AppImageInstaller::new(&current)))
        .on_quit(move |request| sink.0.lock().unwrap().push(request.clone()))
        .build();

    updater.check(CheckOptions::default());
    settle(&updater);
    assert_eq!(updater.state().status, UpdateStatus::Downloaded);
    // Solo se baja el AppImage; el .deb del manifiesto se ignora.
    assert_eq!(
        transport.requested_paths(),
        [
            format!("{FEED_URL}/latest-linux.yml"),
            format!("{FEED_URL}/latest-linux.yml.sig"),
            format!("{FEED_URL}/{APPIMAGE}"),
        ]
    );
    assert_eq!(fs::read(&current).unwrap(), b"old build");

    assert_eq!(updater.install(), InstallResult::ok());

    assert_eq!(fs::read(&current).unwrap(), data);
    assert_eq!(
        quits.requests(),
        [QuitRequest {
            relaunch: Some(RelaunchCommand {
                program: current.clone(),
                args: Vec::new(),
                env: vec![("APPIMAGE_SILENT_INSTALL".to_string(), "true".to_string())],
            })
        }]
    );
}

#[test]
fn linux_deb_only_announces_the_new_version() {
    let cache = tempfile::tempdir().unwrap();
    let config = config(Platform::Linux, "0.1.6", &[], cache.path());
    assert!(!config.can_auto_install);
    assert_eq!(config.artifact_kind, None);
    let harness = new_harness(config);
    let data = payload(1_000);
    harness.transport.serve(
        "latest-linux.yml",
        manifest(
            "0.2.0",
            &[(APPIMAGE, &data), ("mergeon_0.2.0_amd64.deb", &data)],
        ),
    );

    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    // Mismo contrato que en Electron (`downloaded` + `canAutoInstall: false`
    // → "available to download"), pero sin bajar un paquete que nadie instala.
    assert_eq!(
        harness.events.statuses(),
        [
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloaded
        ]
    );
    let state = harness.updater.state();
    assert_eq!(state.latest_version, version("0.2.0"));
    assert_eq!(state.percent, 100);
    assert!(!state.can_auto_install);
    assert_eq!(
        harness.transport.requested_paths(),
        [
            format!("{FEED_URL}/latest-linux.yml"),
            format!("{FEED_URL}/latest-linux.yml.sig"),
        ]
    );
    assert!(!cache.path().join("pending").exists());

    // No se instala sola: la interfaz abre la página de releases.
    assert_eq!(harness.updater.install(), InstallResult::unsupported());
    assert_eq!(
        harness.updater.install_on_quit(),
        InstallResult::unsupported()
    );
    assert!(harness.installer.calls().is_empty());
    assert!(harness.quits.requests().is_empty());
}

#[test]
fn macos_is_notify_only_unless_the_app_opts_in() {
    let data = payload(20_000);
    let intel = payload(900);
    let files: [(&str, &[u8]); 2] = [
        ("Mergeon-0.2.0-mac.zip", &intel),
        ("Mergeon-0.2.0-arm64-mac.zip", &data),
    ];

    // Por defecto, igual que `resolveCanAutoInstall`: solo avisa.
    let cache = tempfile::tempdir().unwrap();
    let default_config = config(Platform::Darwin, "0.1.6", &[], cache.path());
    assert!(!default_config.can_auto_install);
    assert_eq!(default_config.artifact_kind, Some(ArtifactKind::MacZip));
    let notify_only = new_harness(default_config);
    notify_only
        .transport
        .serve("latest-mac.yml", manifest("0.2.0", &files));
    notify_only.updater.check(CheckOptions::default());
    settle(&notify_only.updater);
    assert_eq!(notify_only.updater.state().status, UpdateStatus::Downloaded);
    assert!(!notify_only.updater.state().can_auto_install);
    assert_eq!(notify_only.transport.requests().len(), 2);
    assert_eq!(notify_only.updater.install(), InstallResult::unsupported());

    // Con una build firmada la app puede activarlo: baja el zip de su arquitectura.
    let cache = tempfile::tempdir().unwrap();
    let mut opted_in = config(Platform::Darwin, "0.1.6", &[], cache.path());
    opted_in.can_auto_install = true;
    opted_in.arch = Arch::Arm64;
    let harness = harness_with(
        opted_in,
        Arc::new(FakeTransport::new().with_signing_key(test_private_key())),
        RecordingInstaller::with_outcome(InstallOutcome {
            relaunch: Some(RelaunchCommand {
                program: "/usr/bin/open".into(),
                args: vec!["-n".to_string(), "/Applications/Mergeon.app".to_string()],
                env: Vec::new(),
            }),
        }),
    );
    harness
        .transport
        .serve("latest-mac.yml", manifest("0.2.0", &files));
    harness
        .transport
        .serve_file("Mergeon-0.2.0-arm64-mac.zip", data.clone());

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    assert_eq!(harness.updater.state().status, UpdateStatus::Downloaded);
    assert!(harness.updater.state().can_auto_install);
    assert_eq!(
        harness.transport.requested_paths(),
        [
            format!("{FEED_URL}/latest-mac.yml"),
            format!("{FEED_URL}/latest-mac.yml.sig"),
            format!("{FEED_URL}/Mergeon-0.2.0-arm64-mac.zip"),
        ]
    );
    assert_eq!(harness.updater.install(), InstallResult::ok());
    assert_eq!(
        harness.installer.calls()[0].0.file_name,
        "Mergeon-0.2.0-arm64-mac.zip"
    );
    let quits = harness.quits.requests();
    assert_eq!(quits.len(), 1);
    assert_eq!(
        quits[0]
            .relaunch
            .as_ref()
            .map(|command| command.args.clone()),
        Some(vec![
            "-n".to_string(),
            "/Applications/Mergeon.app".to_string()
        ])
    );
}

#[test]
fn github_releases_feed_uses_the_latest_download_urls() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(3_000);
    let mut config = config(Platform::Win32, "0.1.6", &[], cache.path());
    config.feed = Some(UpdateFeed::github("Spilveru", "mergeon"));
    let harness = new_harness(config);
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    harness.transport.serve_file(EXE, data);

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    assert_eq!(harness.updater.state().status, UpdateStatus::Downloaded);
    let urls: Vec<String> = harness
        .transport
        .requests()
        .iter()
        .map(|request| request.url.clone())
        .collect();
    assert_eq!(
        urls,
        [
            "https://github.com/Spilveru/mergeon/releases/latest/download/latest.yml".to_string(),
            "https://github.com/Spilveru/mergeon/releases/latest/download/latest.yml.sig"
                .to_string(),
            format!("https://github.com/Spilveru/mergeon/releases/latest/download/{EXE}"),
        ]
    );
}

#[test]
fn rejects_unsigned_tampered_wrong_key_and_insecure_http_updates() {
    use mergeon_updater::{generate_signing_keypair, sign_manifest_bytes};

    // 1. Feed http:// rechazado sin tocar la red.
    let cache = tempfile::tempdir().unwrap();
    let mut cfg = config(Platform::Win32, "0.1.6", &[], cache.path());
    cfg.feed = Some(UpdateFeed::generic("http://updates.example.com/win"));
    let harness = new_harness(cfg);
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Error);
    assert!(harness.transport.requests().is_empty());

    // 2. Sin clave pública configurada, falla en cerrado (fail-closed) y desactiva auto-install.
    let cache = tempfile::tempdir().unwrap();
    let mut cfg = config(Platform::Win32, "0.1.6", &[], cache.path());
    cfg.public_key = None;
    let harness = new_harness(cfg);
    assert!(!harness.updater.state().can_auto_install);
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, b"bin")]));
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Error);

    // 3. Falta el archivo .sig → rechazado.
    let cache = tempfile::tempdir().unwrap();
    let transport = Arc::new(FakeTransport::new());
    transport.serve("latest.yml", manifest("0.2.0", &[(EXE, b"bin")]));
    let harness = harness_with(
        config(Platform::Win32, "0.1.6", &[], cache.path()),
        transport,
        RecordingInstaller::new(),
    );
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Error);

    // 4. Manifiesto alterado tras firmar → rechazado.
    let cache = tempfile::tempdir().unwrap();
    let raw = manifest("0.2.0", &[(EXE, b"bin")]);
    let sig = sign_manifest_bytes(raw.as_bytes(), test_private_key()).unwrap();
    let tampered = raw.replace("0.2.0", "0.2.9");
    let transport = Arc::new(FakeTransport::new());
    transport.serve("latest.yml", tampered);
    transport.serve("latest.yml.sig", sig);
    let harness = harness_with(
        config(Platform::Win32, "0.1.6", &[], cache.path()),
        transport,
        RecordingInstaller::new(),
    );
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Error);

    // 5. Firmado con otra clave privada → rechazado.
    let cache = tempfile::tempdir().unwrap();
    let (other_priv, _) = generate_signing_keypair().unwrap();
    let transport = Arc::new(FakeTransport::new().with_signing_key(other_priv));
    transport.serve("latest.yml", manifest("0.2.0", &[(EXE, b"bin")]));
    let harness = harness_with(
        config(Platform::Win32, "0.1.6", &[], cache.path()),
        transport,
        RecordingInstaller::new(),
    );
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Error);

    // 6. URL de artefacto http:// dentro de un manifiesto firmado → rechazada.
    let cache = tempfile::tempdir().unwrap();
    let insecure_manifest = format!(
        "version: 0.2.0\nfiles:\n  - url: http://evil.example.com/{EXE}\n    sha512: {}\n    size: 3\npath: http://evil.example.com/{EXE}\nsha512: {}\nreleaseDate: '2026-09-30T18:21:04.512Z'\n",
        sha512_base64(b"bin"),
        sha512_base64(b"bin"),
    );
    let mut cfg = config(Platform::Win32, "0.1.6", &[], cache.path());
    cfg.public_key = Some(test_public_key().to_string());
    let harness = new_harness(cfg);
    harness.transport.serve("latest.yml", insecure_manifest);
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Error);
    assert!(harness.installer.calls().is_empty());
}

// --- Versiones ---------------------------------------------------------------

#[test]
fn ignores_older_and_equal_versions_and_follows_prereleases() {
    let check = |current: &str, published: &str| {
        let cache = tempfile::tempdir().unwrap();
        let harness = new_harness(config(Platform::Linux, current, &[], cache.path()));
        harness
            .transport
            .serve("latest-linux.yml", manifest(published, &[(APPIMAGE, b"x")]));
        harness.updater.check(CheckOptions::default());
        settle(&harness.updater);
        harness.updater.state()
    };

    assert_eq!(check("0.9.0", "0.9.0").status, UpdateStatus::NotAvailable);
    // Sin downgrade: una versión publicada más vieja no es una actualización.
    let older = check("0.9.0", "0.8.9");
    assert_eq!(older.status, UpdateStatus::NotAvailable);
    assert_eq!(older.latest_version, version("0.8.9"));

    assert_eq!(check("0.9.0", "0.10.0").status, UpdateStatus::Downloaded);
    assert_eq!(
        check("1.0.0-beta.2", "1.0.0-beta.10").status,
        UpdateStatus::Downloaded
    );
    assert_eq!(
        check("1.0.0-rc.1", "1.0.0").status,
        UpdateStatus::Downloaded
    );
    assert_eq!(
        check("1.0.0", "1.0.0-rc.1").status,
        UpdateStatus::NotAvailable
    );
}

#[test]
fn an_up_to_date_check_clears_stale_downloads() {
    let cache = tempfile::tempdir().unwrap();
    let pending = cache.path().join("pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(
        pending.join("Mergeon-Setup-0.1.6.exe"),
        b"installer already applied",
    )
    .unwrap();

    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness.transport.serve(
        "latest.yml",
        manifest("0.1.6", &[("Mergeon-Setup-0.1.6.exe", b"x")]),
    );
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    assert_eq!(harness.updater.state().status, UpdateStatus::NotAvailable);
    assert!(!pending.exists());
}

// --- Errores -----------------------------------------------------------------

#[test]
fn an_invalid_manifest_is_a_check_error() {
    for body in [
        "<!DOCTYPE html>\n<html><body>Sign in</body></html>\n",
        "files:\n  - url: Mergeon.exe\n",
        "version: 'unterminated\n",
    ] {
        let cache = tempfile::tempdir().unwrap();
        let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
        harness.transport.serve("latest.yml", body);

        harness.updater.check(CheckOptions::manual());
        settle(&harness.updater);

        let state = harness.updater.state();
        assert_eq!(state.status, UpdateStatus::Error, "{body:?}");
        assert_eq!(state.error_phase, Some(ErrorPhase::Check));
        assert_eq!(
            state.error.as_deref(),
            Some("El canal de actualizaciones devolvió un manifiesto que no se puede leer.")
        );
        assert_eq!(state.latest_version, None);
        assert_eq!(harness.transport.requests().len(), 2);
        // El evento de error de una comprobación manual lleva la marca; el estado, ya no.
        assert!(harness.events.events().last().unwrap().manual_check);
        assert!(!state.manual_check);
    }
}

#[test]
fn an_invalid_published_version_is_a_check_error() {
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness
        .transport
        .serve("latest.yml", manifest("nightly", &[(EXE, b"x")]));

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.error_phase, Some(ErrorPhase::Check));
    assert_eq!(
        state.error.as_deref(),
        Some("La versión publicada en el canal de actualizaciones no es válida.")
    );
    assert_eq!(harness.transport.requests().len(), 2);
}

#[test]
fn a_network_failure_is_reported_with_the_friendly_message() {
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness.transport.fail(
        "latest.yml",
        TransportError::Network("io: failed to lookup address information".to_string()),
    );

    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.error_phase, Some(ErrorPhase::Check));
    assert_eq!(
        state.error.as_deref(),
        Some("No se pudo contactar con el servidor de actualizaciones.")
    );

    // Un timeout cuenta igual.
    harness
        .transport
        .fail("latest.yml", TransportError::Timeout);
    harness.updater.check(CheckOptions::default());
    assert_eq!(
        harness.updater.state().error.as_deref(),
        Some("No se pudo contactar con el servidor de actualizaciones.")
    );
}

#[test]
fn a_wrong_sha512_is_a_download_error_and_nothing_is_installable() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(9_000);
    let mut tampered = data.clone();
    tampered[100] ^= 0x80;
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    harness.transport.serve_file(EXE, tampered);

    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.error_phase, Some(ErrorPhase::Download));
    assert_eq!(
        state.error.as_deref(),
        Some("La descarga de la actualización está dañada (el sha512 no coincide).")
    );
    assert_eq!(state.latest_version, version("0.2.0"));
    assert_eq!(
        harness.events.status_sequence(),
        [
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Error,
        ]
    );

    // El archivo dañado no se queda en la caché ni se puede instalar.
    assert!(!cache.path().join("pending").join(EXE).exists());
    assert_eq!(harness.updater.install(), InstallResult::unsupported());
    assert!(harness.installer.calls().is_empty());
}

#[test]
fn a_manifest_without_sha512_or_installer_is_a_download_error() {
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness.transport.serve(
        "latest.yml",
        "version: 0.2.0\nfiles:\n  - url: Mergeon-Setup-0.2.0.exe\n",
    );
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.error_phase, Some(ErrorPhase::Download));
    assert_eq!(
        state.error.as_deref(),
        Some("El manifiesto de actualización no incluye el sha512 del instalador.")
    );
    // Sin hash no se descarga nada (solo manifiesto + firma).
    assert_eq!(harness.transport.requests().len(), 2);

    // Manifiesto de Linux que solo trae el .deb: no hay AppImage que aplicar.
    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(
        Platform::Linux,
        "0.1.6",
        &[("APPIMAGE", "/opt/Mergeon.AppImage")],
        cache.path(),
    ));
    harness.transport.serve(
        "latest-linux.yml",
        manifest("0.2.0", &[("mergeon_0.2.0_amd64.deb", b"deb")]),
    );
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    assert_eq!(
        harness.updater.state().error.as_deref(),
        Some("El canal de actualizaciones no tiene un instalador para esta plataforma.")
    );

    let cache = tempfile::tempdir().unwrap();
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness.transport.serve("latest.yml", "version: 0.2.0\n");
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    assert_eq!(
        harness.updater.state().error.as_deref(),
        Some("El manifiesto de actualización no incluye archivos.")
    );
}

#[test]
fn retrying_after_a_failed_download_resumes_it() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(100_000);
    let harness = new_harness(config(Platform::Win32, "0.1.6", &[], cache.path()));
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    {
        let data = data.clone();
        harness.transport.route(EXE, move |request| {
            Ok(FakeResponse::file(request, &data).failing_after(20_000))
        });
    }

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.error_phase, Some(ErrorPhase::Download));
    assert_eq!(
        state.error.as_deref(),
        Some("No se pudo contactar con el servidor de actualizaciones.")
    );

    // El usuario pulsa "Retry" con la red ya recuperada.
    harness.transport.serve_file(EXE, data.clone());
    harness.updater.check(CheckOptions::manual());
    settle(&harness.updater);

    assert_eq!(harness.updater.state().status, UpdateStatus::Downloaded);
    assert_eq!(
        fs::read(cache.path().join("pending").join(EXE)).unwrap(),
        data
    );
    let last = harness.transport.requests().pop().unwrap();
    assert_eq!(last.header("range"), Some("bytes=60000-"));
}

#[test]
fn a_new_process_reuses_the_update_already_downloaded() {
    let cache = tempfile::tempdir().unwrap();
    let (first, _) = windows_with_update(&cache);
    first.updater.check(CheckOptions::default());
    settle(&first.updater);
    assert_eq!(first.updater.state().status, UpdateStatus::Downloaded);
    drop(first);

    // La app se cerró sin instalar y vuelve a abrirse.
    let (second, _) = windows_with_update(&cache);
    second.updater.check(CheckOptions::default());
    settle(&second.updater);

    assert_eq!(second.updater.state().status, UpdateStatus::Downloaded);
    assert_eq!(
        second.transport.requested_paths(),
        [
            format!("{FEED_URL}/latest.yml"),
            format!("{FEED_URL}/latest.yml.sig"),
        ]
    );
    assert_eq!(second.updater.install(), InstallResult::ok());
}

// --- Instalación -------------------------------------------------------------

#[test]
fn a_failed_install_is_reported_and_does_not_quit_the_app() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(2_000);
    let harness = harness_with(
        config(Platform::Win32, "0.1.6", &[], cache.path()),
        Arc::new(FakeTransport::new().with_signing_key(test_private_key())),
        RecordingInstaller::failing(UpdaterError::Install {
            detail: "Access is denied.".to_string(),
        }),
    );
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    harness.transport.serve_file(EXE, data);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    let result = harness.updater.install();
    settle(&harness.updater);

    // `ok: false` hace que la interfaz abra la página de releases.
    assert_eq!(result, InstallResult::failure(InstallFailureReason::Failed));
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::json!({ "ok": false, "reason": "failed" })
    );
    assert!(harness.quits.requests().is_empty());
    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    assert_eq!(state.error_phase, Some(ErrorPhase::Download));
    assert_eq!(
        state.error.as_deref(),
        Some("No se pudo instalar la actualización: Access is denied.")
    );
    assert_eq!(harness.events.events().last(), Some(&state));
}

#[test]
fn the_installer_runs_only_once() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);

    assert_eq!(harness.updater.install(), InstallResult::ok());
    // Segundo clic, y el cierre de la app llamando a `install_on_quit`.
    assert_eq!(harness.updater.install(), InstallResult::ok());
    assert_eq!(harness.updater.install_on_quit(), InstallResult::ok());

    assert_eq!(harness.installer.calls().len(), 1);
    // Cada `install()` vuelve a pedir la salida; `install_on_quit` no.
    assert_eq!(harness.quits.requests().len(), 2);

    // Con el instalador ya lanzado no tiene sentido volver a consultar el feed.
    let requests = harness.transport.requests().len();
    harness.updater.check(CheckOptions::manual());
    assert_eq!(harness.transport.requests().len(), requests);
}

#[test]
fn install_on_quit_applies_the_update_without_relaunching() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);

    // Nada descargado todavía.
    assert_eq!(
        harness.updater.install_on_quit(),
        InstallResult::unsupported()
    );

    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    assert_eq!(harness.updater.install_on_quit(), InstallResult::ok());

    let calls = harness.installer.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].1,
        InstallOptions {
            silent: true,
            relaunch: false
        }
    );
    // La app ya se está cerrando: no se le pide que salga.
    assert!(harness.quits.requests().is_empty());

    // Desactivado por configuración.
    let cache = tempfile::tempdir().unwrap();
    let data = payload(1_000);
    let mut disabled = config(Platform::Win32, "0.1.6", &[], cache.path());
    disabled.auto_install_on_app_quit = false;
    let harness = new_harness(disabled);
    harness
        .transport
        .serve("latest.yml", manifest("0.2.0", &[(EXE, &data)]));
    harness.transport.serve_file(EXE, data);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    assert_eq!(
        harness.updater.install_on_quit(),
        InstallResult::unsupported()
    );
    assert!(harness.installer.calls().is_empty());
}

#[test]
fn install_on_quit_still_applies_a_download_after_a_later_check_fails() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    assert_eq!(harness.updater.state().status, UpdateStatus::Downloaded);

    // La comprobación periódica pilla al equipo sin red.
    harness
        .transport
        .fail("latest.yml", TransportError::Timeout);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    let state = harness.updater.state();
    assert_eq!(state.status, UpdateStatus::Error);
    // Regla del original: con una versión nueva conocida, el fallo cuenta como de descarga.
    assert_eq!(state.error_phase, Some(ErrorPhase::Download));

    // El botón ya no ofrece instalar (igual que en Electron)...
    assert_eq!(harness.updater.install(), InstallResult::unsupported());
    assert!(harness.installer.calls().is_empty());
    // ...pero al cerrar la app la descarga verificada se aplica igualmente.
    assert_eq!(harness.updater.install_on_quit(), InstallResult::ok());
    assert_eq!(harness.installer.calls().len(), 1);
    assert!(harness.quits.requests().is_empty());
}

// --- Concurrencia, eventos y cierre --------------------------------------------

/// Transporte cuyo manifiesto no responde hasta que el test lo libera.
struct GatedTransport {
    inner: FakeTransport,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl HttpTransport for GatedTransport {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if request.url.contains("latest") && !request.url.contains(".sig") {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(WAIT).unwrap();
        }
        self.inner.get(request)
    }
}

#[test]
fn concurrent_checks_share_the_one_in_flight() {
    let cache = tempfile::tempdir().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let transport = Arc::new(GatedTransport {
        inner: FakeTransport::new().with_signing_key(test_private_key()),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    transport
        .inner
        .serve("latest.yml", manifest("0.1.6", &[(EXE, b"x")]));
    let updater = Arc::new(
        AppUpdater::builder(config(Platform::Win32, "0.1.6", &[], cache.path()))
            .transport(transport.clone())
            .installer(Arc::new(RecordingInstaller::new()))
            .build(),
    );
    let events = EventLog::attach(&updater);

    // Comprobación en segundo plano (la del arranque), detenida en la red.
    let background = {
        let updater = updater.clone();
        thread::spawn(move || updater.check(CheckOptions::default()))
    };
    entered_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(updater.state().status, UpdateStatus::Checking);

    // El usuario pulsa "Check for updates" mientras tanto.
    let manual = {
        let updater = updater.clone();
        thread::spawn(move || updater.check(CheckOptions::manual()))
    };
    thread::sleep(Duration::from_millis(50));
    assert!(!manual.is_finished());

    release_tx.send(()).unwrap();
    let background_state = background.join().unwrap();
    let manual_state = manual.join().unwrap();
    settle(&updater);

    // Una sola consulta al feed (manifiesto + firma) y el mismo resultado para los dos.
    assert_eq!(transport.inner.requests().len(), 2);
    assert_eq!(background_state.status, UpdateStatus::NotAvailable);
    assert_eq!(manual_state.status, UpdateStatus::NotAvailable);

    // La marca manual pedida a mitad de vuelo llega al evento final.
    let statuses: Vec<(UpdateStatus, bool)> = events
        .events()
        .iter()
        .map(|event| (event.status, event.manual_check))
        .collect();
    assert_eq!(
        statuses,
        [
            (UpdateStatus::Checking, false),
            (UpdateStatus::NotAvailable, true),
        ]
    );
}

/// Cuerpo que entrega los datos poco a poco, para tener descargas "largas".
struct SlowBody {
    data: Vec<u8>,
    position: usize,
    chunk: usize,
    delay: Duration,
    dropped: Arc<AtomicBool>,
}

impl Read for SlowBody {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        thread::sleep(self.delay);
        let end = (self.position + self.chunk.min(buffer.len())).min(self.data.len());
        let read = end - self.position;
        buffer[..read].copy_from_slice(&self.data[self.position..end]);
        self.position = end;
        Ok(read)
    }
}

impl Drop for SlowBody {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

/// Sirve el manifiesto al instante y el instalador a cuentagotas.
struct SlowTransport {
    manifest: String,
    signature: String,
    data: Vec<u8>,
    chunk: usize,
    delay: Duration,
    manifest_requests: AtomicUsize,
    artifact_requests: AtomicUsize,
    body_dropped: Arc<AtomicBool>,
}

impl SlowTransport {
    fn new(data: Vec<u8>, chunk: usize, delay: Duration) -> Self {
        let manifest = manifest("0.2.0", &[(EXE, &data)]);
        let signature =
            mergeon_updater::sign_manifest_bytes(manifest.as_bytes(), test_private_key())
                .expect("sign test manifest");
        Self {
            manifest,
            signature,
            data,
            chunk,
            delay,
            manifest_requests: AtomicUsize::new(0),
            artifact_requests: AtomicUsize::new(0),
            body_dropped: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl HttpTransport for SlowTransport {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if request.url.contains("latest.yml.sig") {
            return Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Box::new(std::io::Cursor::new(self.signature.clone().into_bytes())),
            });
        }
        if request.url.contains("latest.yml") {
            self.manifest_requests.fetch_add(1, Ordering::SeqCst);
            return Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Box::new(std::io::Cursor::new(self.manifest.clone().into_bytes())),
            });
        }
        self.artifact_requests.fetch_add(1, Ordering::SeqCst);
        Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: Box::new(SlowBody {
                data: self.data.clone(),
                position: 0,
                chunk: self.chunk,
                delay: self.delay,
                dropped: self.body_dropped.clone(),
            }),
        })
    }
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "no ocurrió a tiempo: {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_check_during_a_download_does_not_restart_it() {
    let cache = tempfile::tempdir().unwrap();
    let transport = Arc::new(SlowTransport::new(
        payload(100_000),
        1_000,
        Duration::from_millis(2),
    ));
    let updater = AppUpdater::builder(config(Platform::Win32, "0.1.6", &[], cache.path()))
        .transport(transport.clone())
        .installer(Arc::new(RecordingInstaller::new()))
        .build();
    let events = EventLog::attach(&updater);

    updater.check(CheckOptions::default());
    wait_until("la descarga arranca", || {
        updater.state().status == UpdateStatus::Downloading && updater.state().percent > 0
    });

    // Comprobación periódica o manual a mitad de descarga: no consulta el feed
    // ni devuelve el estado a `checking`.
    let during = updater.check(CheckOptions::manual());
    assert_eq!(during.status, UpdateStatus::Downloading);
    // No se puede instalar algo a medio bajar.
    assert_eq!(updater.install(), InstallResult::unsupported());
    settle(&updater);

    assert_eq!(transport.manifest_requests.load(Ordering::SeqCst), 1);
    assert_eq!(transport.artifact_requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        events.status_sequence(),
        [
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
        ]
    );
    // La marca manual pedida durante la descarga llega al evento final.
    assert!(events.events().last().unwrap().manual_check);
    assert!(!updater.state().manual_check);

    // El progreso nunca retrocede.
    let percents: Vec<u8> = events
        .events()
        .iter()
        .filter(|event| event.status == UpdateStatus::Downloading)
        .map(|event| event.percent)
        .collect();
    assert!(
        percents.windows(2).all(|pair| pair[0] < pair[1]),
        "{percents:?}"
    );
    assert_eq!(percents.first(), Some(&0));
    assert_eq!(percents.last(), Some(&100));
}

#[test]
fn dropping_the_updater_cancels_the_download_and_silences_events() {
    let cache = tempfile::tempdir().unwrap();
    // 2 MB a 1 KB cada 2 ms: tardaría varios segundos en terminar.
    let transport = Arc::new(SlowTransport::new(
        payload(2_000_000),
        1_000,
        Duration::from_millis(2),
    ));
    let updater = AppUpdater::builder(config(Platform::Win32, "0.1.6", &[], cache.path()))
        .transport(transport.clone())
        .installer(Arc::new(RecordingInstaller::new()))
        .build();
    let events = EventLog::attach(&updater);

    updater.check(CheckOptions::default());
    wait_until("la descarga arranca", || {
        updater.state().status == UpdateStatus::Downloading && updater.state().percent > 0
    });

    drop(updater);

    // El hilo de descarga suelta la conexión enseguida, sin terminar el archivo.
    wait_until("el hilo de descarga termina", || {
        transport.body_dropped.load(Ordering::SeqCst)
    });
    let pending = cache.path().join("pending");
    assert!(!pending.join(EXE).exists());
    let kept = fs::metadata(pending.join(format!("{EXE}.part")))
        .unwrap()
        .len();
    assert!(kept > 0 && kept < 2_000_000, "{kept}");

    // Tras el cierre no llega ningún evento más (ni de progreso ni de error).
    let seen = events.events().len();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(events.events().len(), seen);
    assert!(
        events
            .statuses()
            .iter()
            .all(|status| *status != UpdateStatus::Error && *status != UpdateStatus::Downloaded)
    );
}

#[test]
fn shutdown_stops_everything_and_later_calls_are_inert() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);
    harness.updater.check(CheckOptions::default());
    settle(&harness.updater);
    let before = harness.updater.state();
    let requests = harness.transport.requests().len();
    let events = harness.events.events().len();

    harness.updater.shutdown();
    assert!(harness.updater.wait_idle(WAIT));

    // Sigue respondiendo el último estado, pero ya no consulta ni emite.
    assert_eq!(harness.updater.check(CheckOptions::manual()), before);
    harness.updater.check_in_background(CheckOptions::manual());
    harness.updater.start();
    assert!(harness.updater.wait_idle(WAIT));
    assert_eq!(harness.transport.requests().len(), requests);
    assert_eq!(harness.events.events().len(), events);
    // Cerrar dos veces no hace daño.
    harness.updater.shutdown();
}

#[test]
fn start_checks_on_launch_and_then_periodically() {
    let cache = tempfile::tempdir().unwrap();
    let mut config = config(Platform::Win32, "0.1.6", &[], cache.path());
    config.check_on_start = true;
    config.check_interval = Some(Duration::from_millis(20));
    let harness = new_harness(config);
    harness
        .transport
        .serve("latest.yml", manifest("0.1.6", &[(EXE, b"x")]));

    harness.updater.start();
    // Llamarlo otra vez no crea un segundo temporizador.
    harness.updater.start();

    wait_until("comprobación al arrancar y dos periódicas", || {
        harness.transport.requests().len() >= 3
    });
    // Las comprobaciones automáticas nunca llevan la marca manual.
    assert!(
        harness
            .events
            .events()
            .iter()
            .all(|event| !event.manual_check)
    );
    assert!(!harness.updater.wait_idle(Duration::from_millis(30)));

    harness.updater.shutdown();
    assert!(harness.updater.wait_idle(WAIT));
    let after = harness.transport.requests().len();
    thread::sleep(Duration::from_millis(80));
    assert_eq!(harness.transport.requests().len(), after);
}

#[test]
fn start_without_interval_checks_once() {
    let cache = tempfile::tempdir().unwrap();
    let mut config = config(Platform::Win32, "0.1.6", &[], cache.path());
    config.check_on_start = true;
    config.check_interval = None;
    let harness = new_harness(config);
    harness
        .transport
        .serve("latest.yml", manifest("0.1.6", &[(EXE, b"x")]));

    harness.updater.start();
    settle(&harness.updater);

    assert_eq!(harness.transport.requests().len(), 2);
    assert_eq!(harness.updater.state().status, UpdateStatus::NotAvailable);

    // Sin comprobación al arrancar ni intervalo, `start` no hace nada.
    let cache = tempfile::tempdir().unwrap();
    let mut idle = common::config(Platform::Win32, "0.1.6", &[], cache.path());
    idle.check_on_start = false;
    let harness = new_harness(idle);
    harness.updater.start();
    settle(&harness.updater);
    assert!(harness.transport.requests().is_empty());
    assert_eq!(harness.updater.state().status, UpdateStatus::Idle);
}

#[test]
fn check_in_background_reports_through_events() {
    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);

    harness.updater.check_in_background(CheckOptions::manual());
    wait_until("la descarga termina", || {
        harness.updater.state().status == UpdateStatus::Downloaded
    });
    settle(&harness.updater);

    assert_eq!(
        harness.events.status_sequence(),
        [
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
        ]
    );
    assert!(
        harness
            .events
            .events()
            .iter()
            .all(|event| event.manual_check)
    );
}

#[test]
fn subscriptions_can_be_cancelled_and_listeners_may_call_back_into_the_updater() {
    let cache = tempfile::tempdir().unwrap();
    let transport = Arc::new(FakeTransport::new().with_signing_key(test_private_key()));
    transport.serve("latest.yml", manifest("0.1.6", &[(EXE, b"x")]));
    let updater = Arc::new(
        AppUpdater::builder(config(Platform::Win32, "0.1.6", &[], cache.path()))
            .transport(transport.clone())
            .installer(Arc::new(RecordingInstaller::new()))
            .build(),
    );

    let first = EventLog::attach(&updater);
    let cancelled_count = Arc::new(AtomicUsize::new(0));
    let cancelled = {
        let count = cancelled_count.clone();
        updater.subscribe(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        })
    };

    // Un suscriptor que consulta el estado y hasta pide otra comprobación
    // desde dentro del callback no bloquea nada: la comprobación en curso es
    // la suya propia y se le devuelve el estado tal cual.
    let reentrant_states = Arc::new(Mutex::new(Vec::new()));
    {
        let weak = Arc::downgrade(&updater);
        let states = reentrant_states.clone();
        updater.subscribe(move |payload| {
            if let Some(updater) = weak.upgrade() {
                let seen = updater.state().status;
                let checked = updater.check(CheckOptions::default()).status;
                states.lock().unwrap().push((payload.status, seen, checked));
            }
        });
    }

    updater.check(CheckOptions::default());
    settle(&updater);
    assert_eq!(
        first.statuses(),
        [UpdateStatus::Checking, UpdateStatus::NotAvailable]
    );
    assert_eq!(cancelled_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        *reentrant_states.lock().unwrap(),
        [
            (
                UpdateStatus::Checking,
                UpdateStatus::Checking,
                UpdateStatus::Checking
            ),
            (
                UpdateStatus::NotAvailable,
                UpdateStatus::NotAvailable,
                UpdateStatus::NotAvailable
            ),
        ]
    );

    assert!(updater.unsubscribe(cancelled));
    assert!(!updater.unsubscribe(cancelled));

    updater.check(CheckOptions::default());
    settle(&updater);
    assert_eq!(first.events().len(), 4);
    assert_eq!(cancelled_count.load(Ordering::SeqCst), 2);
}

#[test]
fn the_updater_can_be_shared_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AppUpdater>();

    let cache = tempfile::tempdir().unwrap();
    let (harness, _) = windows_with_update(&cache);
    let updater = Arc::new(harness.updater);

    let workers: Vec<_> = (0..8)
        .map(|index| {
            let updater = updater.clone();
            thread::spawn(move || {
                updater.check(CheckOptions {
                    manual: index % 2 == 0,
                });
                updater.state()
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    settle(&updater);

    assert_eq!(updater.state().status, UpdateStatus::Downloaded);
    // Por muchas comprobaciones simultáneas, el instalador se baja una vez.
    let artifact_requests = harness
        .transport
        .requested_paths()
        .iter()
        .filter(|path| path.ends_with(EXE))
        .count();
    assert_eq!(artifact_requests, 1);
}
