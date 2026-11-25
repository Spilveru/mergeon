//! Utilidades compartidas por los tests de integración.
#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mergeon_updater::{
    AppUpdater, Arch, DownloadSettings, Env, Platform, UpdateStatus, UpdaterConfig, UpdaterState,
    generate_signing_keypair, sha512_base64,
};

pub const FEED_URL: &str = "https://updates.example.com/app";
pub const WAIT: Duration = Duration::from_secs(20);

/// Par de claves Ed25519 efímero compartido por los tests de integración.
pub static TEST_KEYPAIR: LazyLock<(String, String)> =
    LazyLock::new(|| generate_signing_keypair().expect("generate Ed25519 test keypair"));

pub fn test_private_key() -> &'static str {
    &TEST_KEYPAIR.0
}

pub fn test_public_key() -> &'static str {
    &TEST_KEYPAIR.1
}

/// Contenido determinista de `len` bytes para hacer de instalador.
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 31 % 251) as u8).collect()
}

/// Manifiesto con el formato de electron-builder para los archivos dados.
pub fn manifest(version: &str, files: &[(&str, &[u8])]) -> String {
    let mut text = format!("version: {version}\nfiles:\n");
    for (name, data) in files {
        text.push_str(&format!(
            "  - url: {name}\n    sha512: {}\n    size: {}\n",
            sha512_base64(data),
            data.len()
        ));
    }
    if let Some((name, data)) = files.first() {
        text.push_str(&format!("path: {name}\nsha512: {}\n", sha512_base64(data)));
    }
    text.push_str("releaseDate: '2026-09-30T18:21:04.512Z'\n");
    text
}

/// Configuración de un build empaquetado con feed genérico, sin esperas
/// largas entre reintentos y sin temporizador.
pub fn config(
    platform: Platform,
    current_version: &str,
    extra_env: &[(&str, &str)],
    cache_dir: &Path,
) -> UpdaterConfig {
    let mut env = Env::from_pairs([("APP_UPDATE_FEED_URL", FEED_URL)]);
    for (key, value) in extra_env {
        env.set(*key, *value);
    }
    let mut config = UpdaterConfig::from_env(
        Some(current_version.to_string()),
        true,
        platform,
        Arch::X64,
        &env,
        cache_dir,
    );
    config.check_interval = None;
    config.download = fast_download_settings();
    config.public_key = Some(test_public_key().to_string());
    config
}

pub fn fast_download_settings() -> DownloadSettings {
    DownloadSettings {
        attempts: 3,
        retry_delay: Duration::from_millis(1),
        response_timeout: Duration::from_secs(5),
        body_timeout: Duration::from_secs(5),
    }
}

/// Registro de los payloads emitidos, en orden.
#[derive(Clone, Default)]
pub struct EventLog(Arc<Mutex<Vec<UpdaterState>>>);

impl EventLog {
    pub fn attach(updater: &AppUpdater) -> Self {
        let log = Self::default();
        let sink = log.clone();
        updater.subscribe(move |state| sink.0.lock().unwrap().push(state.clone()));
        log
    }

    pub fn events(&self) -> Vec<UpdaterState> {
        self.0.lock().unwrap().clone()
    }

    pub fn statuses(&self) -> Vec<UpdateStatus> {
        self.events().iter().map(|state| state.status).collect()
    }

    /// Secuencia de estados sin repeticiones consecutivas (los eventos de
    /// progreso repiten `downloading`).
    pub fn status_sequence(&self) -> Vec<UpdateStatus> {
        let mut sequence = self.statuses();
        sequence.dedup();
        sequence
    }
}

/// Espera a que el actualizador termine lo que tenga en marcha.
pub fn settle(updater: &AppUpdater) {
    assert!(
        updater.wait_idle(WAIT),
        "el actualizador no terminó a tiempo"
    );
}
