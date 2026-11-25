//! Descarga del artefacto a la caché, con reanudación, reintentos y sha512.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD as BASE64_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

use crate::error::UpdaterError;
use crate::transport::{HttpRequest, HttpTransport};

const PENDING_DIR: &str = "pending";
const INFO_FILE: &str = "update-info.json";
const PART_SUFFIX: &str = ".part";
const BUFFER_SIZE: usize = 64 * 1024;

/// Artefacto ya descargado y verificado, listo para el instalador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedUpdate {
    pub version: String,
    pub path: PathBuf,
    pub file_name: String,
    pub sha512: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRequest {
    pub url: String,
    pub file_name: String,
    /// sha512 esperado, en base64.
    pub sha512: String,
    pub size: Option<u64>,
    pub version: String,
}

/// Lo que se recuerda de la descarga en curso para poder reanudarla o reutilizarla.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingInfo {
    file_name: String,
    sha512: String,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadSettings {
    /// Intentos totales ante fallos de red o del servidor.
    pub attempts: u32,
    /// Espera antes del primer reintento; crece linealmente con cada intento.
    pub retry_delay: Duration,
    pub response_timeout: Duration,
    pub body_timeout: Duration,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            attempts: 3,
            retry_delay: Duration::from_secs(2),
            response_timeout: Duration::from_secs(30),
            body_timeout: Duration::from_secs(30 * 60),
        }
    }
}

pub struct Downloader {
    transport: Arc<dyn HttpTransport>,
    cache_dir: PathBuf,
    settings: DownloadSettings,
}

/// sha512 de un archivo, en base64 (el formato de electron-builder).
pub fn sha512_base64_of_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha512::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(BASE64.encode(hasher.finalize()))
}

/// sha512 de unos bytes, en base64.
pub fn sha512_base64(data: &[u8]) -> String {
    BASE64.encode(Sha512::digest(data))
}

/// Compara dos sha512 en base64 por su valor, tolerando espacios y relleno.
pub fn sha512_matches(expected: &str, actual: &str) -> bool {
    let expected = expected.trim();
    let actual = actual.trim();
    if expected == actual {
        return true;
    }
    let decode = |text: &str| BASE64_NO_PAD.decode(text.trim_end_matches('=')).ok();
    match (decode(expected), decode(actual)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Verifica un archivo contra su sha512 esperado.
pub fn verify_sha512(path: &Path, expected: &str) -> Result<(), UpdaterError> {
    let actual = sha512_base64_of_file(path)
        .map_err(|error| UpdaterError::io("No se pudo leer la actualización descargada", &error))?;
    if sha512_matches(expected, &actual) {
        Ok(())
    } else {
        Err(UpdaterError::ChecksumMismatch {
            expected: expected.trim().to_string(),
            actual,
        })
    }
}

/// Resultado de un intento de descarga que no terminó el archivo.
enum AttemptError {
    /// Vale la pena reintentar (red caída, 5xx, corte a mitad).
    Retry(UpdaterError),
    /// No tiene sentido insistir (404, 403, disco lleno...).
    Fatal(UpdaterError),
}

impl Downloader {
    pub fn new(
        transport: Arc<dyn HttpTransport>,
        cache_dir: impl Into<PathBuf>,
        settings: DownloadSettings,
    ) -> Self {
        Self {
            transport,
            cache_dir: cache_dir.into(),
            settings,
        }
    }

    /// Directorio donde quedan las descargas (`<caché>/pending`, como electron-updater).
    pub fn pending_dir(&self) -> PathBuf {
        self.cache_dir.join(PENDING_DIR)
    }

    /// Borra lo descargado: se llama cuando ya no hay nada que instalar.
    pub fn clear(&self) {
        let dir = self.pending_dir();
        if dir.exists()
            && let Err(error) = fs::remove_dir_all(&dir)
        {
            log::warn!("[updater] could not clear the download cache: {error}");
        }
    }

    fn read_info(&self) -> Option<PendingInfo> {
        let text = fs::read_to_string(self.pending_dir().join(INFO_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write_info(&self, info: &PendingInfo) -> io::Result<()> {
        let text = serde_json::to_string_pretty(info).map_err(io::Error::other)?;
        fs::write(self.pending_dir().join(INFO_FILE), text)
    }

    /// Descarga `request` a la caché y lo verifica.
    ///
    /// - Si el archivo ya está en caché con el mismo sha512, se reutiliza.
    /// - Una descarga a medias (`.part`) del mismo artefacto se reanuda con `Range`.
    /// - Los fallos de red y los 5xx se reintentan, reanudando lo ya bajado.
    ///
    /// `on_progress` recibe el porcentaje (0–100) cada vez que cambia el entero.
    pub fn download(
        &self,
        request: &DownloadRequest,
        on_progress: &mut dyn FnMut(f64),
        cancel: &AtomicBool,
    ) -> Result<DownloadedUpdate, UpdaterError> {
        let save_error =
            |error: &io::Error| UpdaterError::io("No se pudo guardar la actualización", error);

        let dir = self.pending_dir();
        let expected_info = PendingInfo {
            file_name: request.file_name.clone(),
            sha512: request.sha512.trim().to_string(),
            version: Some(request.version.clone()),
        };

        // Lo que hubiera de otra versión (u otro artefacto) ya no sirve.
        let reusable = self.read_info().is_some_and(|info| {
            info.file_name == expected_info.file_name && info.sha512 == expected_info.sha512
        });
        if !reusable {
            self.clear();
        }
        fs::create_dir_all(&dir).map_err(|error| save_error(&error))?;
        self.write_info(&expected_info)
            .map_err(|error| save_error(&error))?;

        let final_path = dir.join(&request.file_name);
        let part_path = dir.join(format!("{}{PART_SUFFIX}", request.file_name));

        if final_path.is_file() {
            match verify_sha512(&final_path, &request.sha512) {
                Ok(()) => {
                    log::info!(
                        "[updater] update already downloaded to {}",
                        final_path.display()
                    );
                    return self.finish(request, final_path);
                }
                Err(error) => {
                    log::warn!(
                        "[updater] cached update is not valid, downloading again: {error:?}"
                    );
                    remove_file_if_exists(&final_path);
                }
            }
        }

        let attempts = self.settings.attempts.max(1);
        let mut attempt = 0;
        // Un hash incorrecto tras reanudar suele ser un `.part` viejo o corrupto:
        // se concede una única descarga limpia desde cero.
        let mut clean_retry_used = false;
        let mut last_error = None;

        while attempt < attempts {
            attempt += 1;
            if cancel.load(Ordering::SeqCst) {
                return Err(UpdaterError::Cancelled);
            }

            let resumed_from = file_len(&part_path);
            match self.attempt(request, &part_path, on_progress, cancel) {
                Ok(()) => {}
                Err(AttemptError::Fatal(error)) => return Err(error),
                Err(AttemptError::Retry(error)) => {
                    log::warn!("[updater] download attempt {attempt}/{attempts} failed: {error:?}");
                    last_error = Some(error);
                    if attempt < attempts
                        && !sleep_cancellable(self.settings.retry_delay * attempt, cancel)
                    {
                        return Err(UpdaterError::Cancelled);
                    }
                    continue;
                }
            }

            match verify_sha512(&part_path, &request.sha512) {
                Ok(()) => {
                    fs::rename(&part_path, &final_path).map_err(|error| save_error(&error))?;
                    return self.finish(request, final_path);
                }
                Err(error) => {
                    remove_file_if_exists(&part_path);
                    if resumed_from > 0 && !clean_retry_used {
                        log::warn!(
                            "[updater] resumed download did not verify, starting over: {error:?}"
                        );
                        clean_retry_used = true;
                        // No cuenta como intento: no fue un fallo de red.
                        attempt -= 1;
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            }
        }

        Err(last_error.unwrap_or(UpdaterError::Cancelled))
    }

    fn finish(
        &self,
        request: &DownloadRequest,
        path: PathBuf,
    ) -> Result<DownloadedUpdate, UpdaterError> {
        let size = file_len(&path);
        Ok(DownloadedUpdate {
            version: request.version.clone(),
            path,
            file_name: request.file_name.clone(),
            sha512: request.sha512.trim().to_string(),
            size,
        })
    }

    /// Un intento: deja `part_path` completo o devuelve por qué no pudo.
    fn attempt(
        &self,
        request: &DownloadRequest,
        part_path: &Path,
        on_progress: &mut dyn FnMut(f64),
        cancel: &AtomicBool,
    ) -> Result<(), AttemptError> {
        let save_error = |error: &io::Error| {
            AttemptError::Fatal(UpdaterError::io(
                "No se pudo guardar la actualización",
                error,
            ))
        };

        let mut offset = file_len(part_path);
        if let Some(size) = request.size {
            if offset == size && size > 0 {
                // Ya está entero: solo falta verificarlo.
                return Ok(());
            }
            if offset > size {
                remove_file_if_exists(part_path);
                offset = 0;
            }
        }

        let mut headers = vec![("Accept".to_string(), "*/*".to_string())];
        if offset > 0 {
            headers.push(("Range".to_string(), format!("bytes={offset}-")));
        }
        let http_request = HttpRequest {
            url: request.url.clone(),
            headers,
            response_timeout: self.settings.response_timeout,
            body_timeout: self.settings.body_timeout,
        };

        let mut response = self.transport.get(&http_request).map_err(|error| {
            AttemptError::Retry(UpdaterError::Network {
                url: request.url.clone(),
                detail: error.to_string(),
            })
        })?;

        let status = response.status;
        let status_error = || UpdaterError::HttpStatus {
            status,
            url: request.url.clone(),
        };

        let append = match status {
            206 if offset > 0 => {
                // El servidor tiene que reanudar justo donde quedó el archivo.
                let starts_at_offset = response
                    .header("content-range")
                    .and_then(parse_content_range_start)
                    .is_none_or(|start| start == offset);
                if !starts_at_offset {
                    remove_file_if_exists(part_path);
                    return Err(AttemptError::Retry(status_error()));
                }
                true
            }
            200..=299 => false,
            416 => {
                // El rango pedido no existe: el `.part` no corresponde a este archivo.
                remove_file_if_exists(part_path);
                return Err(AttemptError::Retry(status_error()));
            }
            408 | 429 | 500..=599 => return Err(AttemptError::Retry(status_error())),
            _ => return Err(AttemptError::Fatal(status_error())),
        };

        if !append {
            offset = 0;
        }
        let total = request
            .size
            .or_else(|| response.content_length().map(|length| offset + length))
            .filter(|total| *total > 0);

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part_path)
            .map_err(|error| save_error(&error))?;

        let mut progress = ProgressReporter::new(total);
        progress.report(offset, on_progress);

        let mut written = offset;
        let mut buffer = vec![0u8; BUFFER_SIZE];
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(AttemptError::Fatal(UpdaterError::Cancelled));
            }
            let read = match response.body.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    // Lo bajado hasta aquí se conserva para reanudar.
                    let _ = file.flush();
                    return Err(AttemptError::Retry(UpdaterError::Network {
                        url: request.url.clone(),
                        detail: error.to_string(),
                    }));
                }
            };
            file.write_all(&buffer[..read])
                .map_err(|error| save_error(&error))?;
            written += read as u64;
            progress.report(written, on_progress);
        }
        file.flush().map_err(|error| save_error(&error))?;
        file.sync_all().map_err(|error| save_error(&error))?;
        drop(file);

        if let Some(size) = request.size {
            if written < size {
                // La conexión se cerró antes de tiempo sin dar error.
                return Err(AttemptError::Retry(UpdaterError::SizeMismatch {
                    expected: size,
                    actual: written,
                }));
            }
            if written > size {
                remove_file_if_exists(part_path);
                return Err(AttemptError::Fatal(UpdaterError::SizeMismatch {
                    expected: size,
                    actual: written,
                }));
            }
        }

        Ok(())
    }
}

/// Emite el porcentaje solo cuando cambia su valor entero.
struct ProgressReporter {
    total: Option<u64>,
    last: Option<u64>,
}

impl ProgressReporter {
    fn new(total: Option<u64>) -> Self {
        Self { total, last: None }
    }

    fn report(&mut self, transferred: u64, on_progress: &mut dyn FnMut(f64)) {
        let percent = match self.total {
            Some(total) => (transferred.min(total) as f64) * 100.0 / (total as f64),
            // Sin tamaño conocido no hay porcentaje que dar: se anuncia el inicio.
            None => 0.0,
        };
        let rounded = percent.round() as u64;
        if self.last != Some(rounded) {
            self.last = Some(rounded);
            on_progress(percent);
        }
    }
}

/// `bytes 100-999/1000` → 100.
fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes")?.trim_start();
    range.split('-').next()?.trim().parse().ok()
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn remove_file_if_exists(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!("[updater] could not remove {}: {error}", path.display());
    }
}

/// Duerme `duration` en tramos cortos. Devuelve `false` si se canceló.
pub(crate) fn sleep_cancellable(duration: Duration, cancel: &AtomicBool) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(25)));
    }
}
