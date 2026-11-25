//! Traza opcional de cuánto tarda cada lectura del repositorio
//! (`src/utils/repositoryReadInstrumentation.js`).
//!
//! El original se activaba en desarrollo con `localStorage['mergeon:trace-reads']
//! = '1'`. Aquí, en compilaciones de depuración, con la variable de entorno
//! `MERGEON_TRACE_READS=1`; la línea sale por `log::debug!`.

use std::time::Instant;

/// Variable de entorno que activa la traza.
pub const TRACE_READS_ENV: &str = "MERGEON_TRACE_READS";

pub fn should_log_repository_reads() -> bool {
    cfg!(debug_assertions) && std::env::var(TRACE_READS_ENV).is_ok_and(|value| value == "1")
}

/// Línea de traza de una lectura.
pub fn repository_read_trace(operation: &str, repo_path: &str, duration_ms: u128) -> String {
    format!("[repo-read] {operation} {repo_path} {duration_ms}ms")
}

/// Ejecuta `callback` y devuelve su resultado; con la traza activa, anota cuánto tardó.
pub fn measure_repository_read<T>(
    operation: &str,
    repo_path: &str,
    callback: impl FnOnce() -> T,
) -> T {
    let started_at = Instant::now();

    let result = callback();

    if should_log_repository_reads() {
        // `Math.round` sobre milisegundos con decimales.
        let duration_ms = (started_at.elapsed().as_secs_f64() * 1000.0).round() as u128;
        log::debug!(
            "{}",
            repository_read_trace(operation, repo_path, duration_ms)
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn returns_the_callback_result_when_read_tracing_is_disabled() {
        let calls = Cell::new(0);

        let result = measure_repository_read("getBranches", "/tmp/demo", || {
            calls.set(calls.get() + 1);
            ("master", vec!["master", "remotes/origin/master"])
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(result, ("master", vec!["master", "remotes/origin/master"]));
    }

    #[test]
    fn hands_back_errors_untouched() {
        let result: Result<(), String> = measure_repository_read("getStatus", "/tmp/demo", || {
            Err("Failed to get status: boom".to_string())
        });

        assert_eq!(result, Err("Failed to get status: boom".to_string()));
    }

    #[test]
    fn writes_the_trace_line_like_the_original() {
        assert_eq!(
            repository_read_trace("getBranches", "/tmp/demo", 12),
            "[repo-read] getBranches /tmp/demo 12ms"
        );
    }
}
