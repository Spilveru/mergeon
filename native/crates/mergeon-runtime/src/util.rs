//! Utilidades compartidas por la cola de mutaciones y el watcher.

use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Toma el lock aunque otro hilo haya entrado en pánico con él tomado: el
/// estado que protegen estos mutex se mantiene coherente entre sentencias.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Equivalente a `path.resolve(String(p))` de Node: ruta absoluta y
/// normalizada de forma léxica (sin tocar el disco ni resolver symlinks).
pub(crate) fn resolve_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // En la raíz, `..` no sube más (igual que en Node).
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
}

/// `new Date().toISOString()`: UTC con milisegundos.
pub(crate) fn iso_timestamp(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_seconds = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();
    let seconds_of_day = total_seconds % 86_400;

    // Días desde 1970-01-01 a fecha civil (algoritmo de Howard Hinnant).
    let days = (total_seconds / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_iso_timestamps_like_javascript() {
        assert_eq!(iso_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso_timestamp(UNIX_EPOCH + Duration::from_millis(1_709_210_096_789)),
            "2024-02-29T12:34:56.789Z"
        );
        assert_eq!(
            iso_timestamp(UNIX_EPOCH + Duration::from_millis(1_798_761_599_999)),
            "2026-12-31T23:59:59.999Z"
        );
    }

    #[test]
    fn resolves_paths_lexically() {
        let base = resolve_path(Path::new("repo"));
        assert!(base.is_absolute());
        assert_eq!(resolve_path(Path::new("repo/")), base);
        assert_eq!(resolve_path(Path::new("repo/./sub/..")), base);
        assert_eq!(resolve_path(Path::new("./repo")), base);
        #[cfg(unix)]
        {
            assert_eq!(resolve_path(Path::new("/repo/a/")), Path::new("/repo/a"));
            assert_eq!(
                resolve_path(Path::new("/repo/a/../b")),
                Path::new("/repo/b")
            );
            assert_eq!(resolve_path(Path::new("/..")), Path::new("/"));
        }
    }
}
