//! Registro de logs rotativo en disco y captura de pánicos (`crash.log`).
//!
//! En builds empaquetados (AppImage, NSIS, `.app`) nadie ve `stderr`: este
//! módulo replica el log en `<userData>/logs/mergeon.log` (con rotación en
//! `mergeon.log.1` al superar 2 MiB) e instala un panic hook que escribe el
//! backtrace y los datos del entorno en `<userData>/logs/crash.log`.

use std::backtrace::Backtrace;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const LOG_FILE_NAME: &str = "mergeon.log";
const ROTATED_LOG_FILE_NAME: &str = "mergeon.log.1";
const CRASH_FILE_NAME: &str = "crash.log";

/// Directorio de logs de la app: `<MERGEON_USER_DATA_DIR>/logs` o
/// `<default_user_data_dir>/logs`.
pub fn resolve_logs_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MERGEON_USER_DATA_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("logs"));
    }
    mergeon_backend::default_user_data_dir().map(|dir| dir.join("logs"))
}

struct FileSink {
    dir: PathBuf,
    file: Option<File>,
    written: u64,
}

impl FileSink {
    fn open(dir: PathBuf) -> Option<Self> {
        fs::create_dir_all(&dir).ok()?;
        let path = dir.join(LOG_FILE_NAME);
        let written = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let file = open_log_file(&path).ok()?;
        Some(Self {
            dir,
            file: Some(file),
            written,
        })
    }

    fn write_line(&mut self, line: &str) {
        if self.written >= MAX_LOG_BYTES {
            self.rotate();
        }
        if let Some(file) = self.file.as_mut()
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.written += line.len() as u64;
        }
    }

    fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        let current = self.dir.join(LOG_FILE_NAME);
        let rotated = self.dir.join(ROTATED_LOG_FILE_NAME);
        let _ = fs::rename(&current, &rotated);
        self.file = open_log_file(&current).ok();
        self.written = 0;
    }
}

fn open_log_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

struct DualLogger {
    stderr: env_logger::Logger,
    file: Option<Mutex<FileSink>>,
}

impl Log for DualLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.stderr.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        self.stderr.log(record);
        if let Some(file) = &self.file {
            let line = format!(
                "[{} {} {}] {}\n",
                unix_timestamp_secs(),
                record.level(),
                record.target(),
                record.args()
            );
            file.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write_line(&line);
        }
    }

    fn flush(&self) {
        self.stderr.flush();
        if let Some(file) = &self.file {
            file.lock().unwrap_or_else(PoisonError::into_inner).flush();
        }
    }
}

fn format_crash_report(message: &str, location: &str, backtrace: &str) -> String {
    format!(
        "=== Mergeon crash report ===\n\
         timestamp_unix: {}\n\
         version: {}\n\
         os: {} ({})\n\
         location: {location}\n\
         message: {message}\n\
         backtrace:\n{backtrace}\n\n",
        unix_timestamp_secs(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn install_panic_hook(logs_dir: Option<PathBuf>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
            (*text).to_string()
        } else if let Some(text) = info.payload().downcast_ref::<String>() {
            text.clone()
        } else {
            "panic payload is not a string".to_string()
        };
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let backtrace = Backtrace::force_capture().to_string();
        let report = format_crash_report(&message, &location, &backtrace);

        if let Some(dir) = &logs_dir {
            let _ = fs::create_dir_all(dir);
            if let Ok(mut file) = open_log_file(&dir.join(CRASH_FILE_NAME)) {
                let _ = file.write_all(report.as_bytes());
                let _ = file.flush();
            }
        }
        eprintln!("{report}");
        previous(info);
    }));
}

/// Inicializa el logger dual (`stderr` + archivo rotativo) y el panic hook.
pub fn init() {
    let stderr =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .filter_module("naga", LevelFilter::Error)
            .filter_module("blade_graphics", LevelFilter::Error)
            .build();
    let max_level = stderr.filter();
    let logs_dir = resolve_logs_dir();
    let file = logs_dir.clone().and_then(FileSink::open).map(Mutex::new);

    let logger = DualLogger { stderr, file };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max_level);
    }
    install_panic_hook(logs_dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_the_log_file_once_it_reaches_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::open(dir.path().to_path_buf()).unwrap();
        sink.written = MAX_LOG_BYTES;
        sink.write_line("first line after rotation\n");
        sink.flush();

        assert!(dir.path().join(ROTATED_LOG_FILE_NAME).is_file());
        let current = fs::read_to_string(dir.path().join(LOG_FILE_NAME)).unwrap();
        assert_eq!(current, "first line after rotation\n");
    }

    #[test]
    fn crash_report_includes_version_os_location_and_message() {
        let report = format_crash_report("boom", "src/main.rs:42:1", "frame 0");
        assert!(report.contains(env!("CARGO_PKG_VERSION")));
        assert!(report.contains(std::env::consts::OS));
        assert!(report.contains("src/main.rs:42:1"));
        assert!(report.contains("boom"));
        assert!(report.contains("frame 0"));
    }
}
