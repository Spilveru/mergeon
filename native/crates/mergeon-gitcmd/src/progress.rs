//! Progreso de las operaciones de red (`git fetch/pull/push/clone --progress`).
//!
//! Con `--progress` git escribe en stderr una línea por actualización,
//! terminada en `\r` mientras la fase avanza y en `\n` al cerrarla:
//!
//! ```text
//! remote: Counting objects:  45% (206/458)        \r
//! Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s\r
//! Receiving objects: 100% (1000/1000), 2.61 MiB | 2.00 MiB/s, done.\n
//! Resolving deltas: 100% (153/153), done.\n
//! ```
//!
//! [`parse_progress_line`] reconoce esas líneas sin depender del idioma (el
//! runner fuerza `LC_ALL=C`, pero la fase se toma tal cual venga) y
//! [`ProgressFilter`] las separa del resto de stderr: el progreso se publica y
//! lo demás (errores, `To <url>`, avisos de hooks) queda como estaba sin
//! `--progress`, para que los mensajes de error no se llenen de porcentajes.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Una actualización de progreso de git.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitProgress {
    /// Fase tal como la escribe git, sin `remote: ` ("Receiving objects").
    pub phase: String,
    /// La fase ocurre en el servidor (líneas `remote: ...`).
    pub remote: bool,
    /// Porcentaje de la fase, si git lo da (no lo da al enumerar objetos).
    pub percent: Option<u8>,
    /// Elementos procesados (el `450` de `(450/1000)`, o el contador suelto).
    pub processed: u64,
    /// Total de la fase (el `1000` de `(450/1000)`).
    pub total: Option<u64>,
    /// Bytes recibidos o enviados, con su unidad ("1.20 MiB").
    pub transferred: Option<String>,
    /// Velocidad ("2.00 MiB/s").
    pub throughput: Option<String>,
    /// La línea cierra la fase (`, done.`).
    pub done: bool,
}

impl GitProgress {
    /// Avance de la fase entre 0 y 1, si git dio porcentaje.
    pub fn fraction(&self) -> Option<f32> {
        if let Some(total) = self.total.filter(|total| *total > 0) {
            return Some((self.processed as f32 / total as f32).clamp(0., 1.));
        }
        self.percent
            .map(|percent| (f32::from(percent) / 100.).clamp(0., 1.))
    }
}

/// Receptor de las actualizaciones de progreso. Se llama desde el hilo que
/// lee stderr: debe volver enseguida.
pub type ProgressSink = Arc<dyn Fn(GitProgress) + Send + Sync>;

/*
 * `<fase>: <pct>% (<hechos>/<total>)[, <bytes> | <velocidad>][, done.]` o
 * `<fase>: <contador>[, done.]`. Lo traducible es la fase y la palabra de
 * cierre: la fase no puede llevar `:` ni dígitos al principio, y el cierre es
 * cualquier texto final terminado en punto (`done.`, `listo.`, `fertig.`,
 * `completed with 1 local object.`).
 * El `%` admite espacio delante (algunas traducciones usan `45 %`).
 */
static PROGRESS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        ^(?P<remote>remote:\s+)?
        (?P<phase>[^\d:\s][^:]*?):\s+
        (?:
            (?P<percent>\d{1,3})[\s\u{a0}]?%\s*\((?P<processed>\d+)/(?P<total>\d+)\)
          | (?P<count>\d+)
        )
        (?:,\s*(?P<transferred>\d[\d.,]*\s*[^\s|,]+)(?:\s*\|\s*(?P<throughput>\d[\d.,]*\s*\S+?/s))?)?
        (?P<done>,\s*[^\d\s,|][^,|]*\.)?
        \s*$",
    )
    .expect("progress regex")
});

/// Líneas que git solo escribe con `--progress` y no son una fase.
static PROGRESS_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:remote:\s+)?(?:Total \d+ \(delta \d+\)|Delta compression using up to \d+ threads?)",
    )
    .expect("progress noise regex")
});

/// Reconoce una línea de progreso de git (sin su `\r`/`\n`).
pub fn parse_progress_line(line: &str) -> Option<GitProgress> {
    let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{1b}');
    let captures = PROGRESS_LINE.captures(line)?;
    let number = |name: &str| {
        captures
            .name(name)
            .and_then(|value| value.as_str().parse::<u64>().ok())
    };
    let text = |name: &str| captures.name(name).map(|value| value.as_str().to_string());

    let percent = number("percent").map(|value| value.min(100) as u8);
    let processed = number("processed").or_else(|| number("count"))?;
    Some(GitProgress {
        phase: captures["phase"].trim().to_string(),
        remote: captures.name("remote").is_some(),
        percent,
        processed,
        total: number("total"),
        transferred: text("transferred"),
        throughput: text("throughput"),
        done: captures.name("done").is_some(),
    })
}

/// Una línea que git escribe solo por `--progress` (el resumen `Total ...`,
/// `Delta compression ...`) o una fase de progreso.
pub fn is_progress_output(line: &str) -> bool {
    let trimmed = line.trim();
    PROGRESS_NOISE.is_match(trimmed) || parse_progress_line(trimmed).is_some()
}

/// Separa el progreso del resto de stderr mientras llega por trozos.
///
/// Cada trozo se corta en segmentos terminados en `\r` o `\n`; los de
/// progreso se devuelven parseados y no pasan a `kept`, el resto se copia
/// byte a byte (con su terminador). Un segmento sin terminar espera al
/// siguiente trozo, o a [`ProgressFilter::finish`].
#[derive(Debug, Default)]
pub struct ProgressFilter {
    pending: Vec<u8>,
    kept: Vec<u8>,
    /// El último segmento era progreso terminado en `\r`: un `\n` vacío
    /// detrás (`\r\n` de Windows) también se descarta.
    dropped_cr: bool,
}

impl ProgressFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Añade un trozo de stderr y devuelve las actualizaciones que trae.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<GitProgress> {
        let mut updates = Vec::new();
        let mut start = 0;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte != b'\r' && *byte != b'\n' {
                continue;
            }
            self.pending.extend_from_slice(&chunk[start..index]);
            let segment = std::mem::take(&mut self.pending);
            self.segment(&segment, Some(*byte), &mut updates);
            start = index + 1;
        }
        self.pending.extend_from_slice(&chunk[start..]);
        updates
    }

    /// Cierra el flujo: el último segmento sin terminador.
    pub fn finish(&mut self) -> Vec<GitProgress> {
        let mut updates = Vec::new();
        if !self.pending.is_empty() {
            let segment = std::mem::take(&mut self.pending);
            self.segment(&segment, None, &mut updates);
        }
        updates
    }

    /// stderr sin el progreso.
    pub fn into_kept(mut self) -> Vec<u8> {
        self.finish();
        self.kept
    }

    fn segment(&mut self, segment: &[u8], terminator: Option<u8>, updates: &mut Vec<GitProgress>) {
        if segment.is_empty() && terminator == Some(b'\n') && self.dropped_cr {
            self.dropped_cr = false;
            return;
        }
        self.dropped_cr = false;
        let text = String::from_utf8_lossy(segment);
        if let Some(progress) = parse_progress_line(&text) {
            updates.push(progress);
            self.dropped_cr = terminator == Some(b'\r');
            return;
        }
        if !text.trim().is_empty() && is_progress_output(&text) {
            self.dropped_cr = terminator == Some(b'\r');
            return;
        }
        self.kept.extend_from_slice(segment);
        if let Some(terminator) = terminator {
            self.kept.push(terminator);
        }
    }
}

/// Cuántas actualizaciones dejar pasar: git escribe una por cada punto
/// porcentual (y cada segundo con la velocidad), que con varias fases son
/// cientos. Pasan la primera, cada cambio de fase y, dentro de una fase, una
/// cada `interval` como mucho.
#[derive(Debug, Clone)]
pub struct ProgressThrottle {
    interval: Duration,
    last: Option<(Instant, String, bool)>,
}

/// ~10 actualizaciones por segundo.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new(PROGRESS_INTERVAL)
    }
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Si `progress` debe publicarse en `now` (y, si sí, lo apunta).
    pub fn admit(&mut self, progress: &GitProgress, now: Instant) -> bool {
        let admit = match &self.last {
            None => true,
            Some((at, phase, remote)) => {
                *phase != progress.phase
                    || *remote != progress.remote
                    || now.saturating_duration_since(*at) >= self.interval
            }
        };
        if admit {
            self.last = Some((now, progress.phase.clone(), progress.remote));
        }
        admit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> GitProgress {
        parse_progress_line(line).unwrap_or_else(|| panic!("not progress: {line:?}"))
    }

    #[test]
    fn parses_receiving_with_throughput() {
        let progress = parse("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s");
        assert_eq!(progress.phase, "Receiving objects");
        assert!(!progress.remote);
        assert_eq!(progress.percent, Some(45));
        assert_eq!(progress.processed, 450);
        assert_eq!(progress.total, Some(1000));
        assert_eq!(progress.transferred.as_deref(), Some("1.20 MiB"));
        assert_eq!(progress.throughput.as_deref(), Some("2.00 MiB/s"));
        assert!(!progress.done);
        assert_eq!(progress.fraction(), Some(0.45));
    }

    #[test]
    fn parses_done_lines_and_bytes() {
        let progress = parse("Receiving objects: 100% (458/458), 5.90 MiB | 14.34 MiB/s, done.");
        assert!(progress.done);
        assert_eq!(progress.throughput.as_deref(), Some("14.34 MiB/s"));
        let unpacking = parse("Unpacking objects: 100% (3/3), 250 bytes | 250.00 KiB/s, done.");
        assert_eq!(unpacking.transferred.as_deref(), Some("250 bytes"));
        assert_eq!(unpacking.throughput.as_deref(), Some("250.00 KiB/s"));
        let deltas = parse("Resolving deltas: 100% (153/153), done.");
        assert_eq!(deltas.phase, "Resolving deltas");
        assert!(deltas.done);
        assert_eq!(deltas.transferred, None);
        let thin = parse("Resolving deltas: 100% (1/1), completed with 1 local object.");
        assert!(thin.done);
        assert_eq!(thin.processed, 1);
    }

    #[test]
    fn parses_remote_lines_with_padding() {
        let progress = parse("remote: Counting objects:  12% (55/458)        ");
        assert!(progress.remote);
        assert_eq!(progress.phase, "Counting objects");
        assert_eq!(progress.percent, Some(12));
        let enumerating = parse("remote: Enumerating objects: 458, done.        ");
        assert_eq!(enumerating.phase, "Enumerating objects");
        assert_eq!(enumerating.percent, None);
        assert_eq!(enumerating.processed, 458);
        assert_eq!(enumerating.total, None);
        assert_eq!(enumerating.fraction(), None);
        assert!(enumerating.done);
    }

    #[test]
    fn parses_counters_without_percentage() {
        let progress = parse("Counting objects: 1234");
        assert_eq!(progress.processed, 1234);
        assert_eq!(progress.fraction(), None);
        assert!(!progress.done);
        assert_eq!(parse("Updating files:  30% (3/10)").phase, "Updating files");
        assert_eq!(
            parse("Writing objects:  50% (280/560)").phase,
            "Writing objects"
        );
    }

    #[test]
    fn tolerates_translated_output() {
        let progress = parse("Recibiendo objetos:  45% (450/1000), 1.20 MiB | 2.00 MiB/s");
        assert_eq!(progress.phase, "Recibiendo objetos");
        assert_eq!(progress.percent, Some(45));
        let french = parse("Réception d'objets:  45 % (450/1000), 1.20 Mio | 2.00 Mio/s, fait.");
        assert_eq!(french.phase, "Réception d'objets");
        assert_eq!(french.percent, Some(45));
        assert_eq!(french.throughput.as_deref(), Some("2.00 Mio/s"));
        assert!(french.done);
        let german = parse("Empfange Objekte: 100% (10/10), fertig.");
        assert!(german.done);
        assert_eq!(
            parse("remote: Zähle Objekte: 100% (5/5), fertig.").phase,
            "Zähle Objekte"
        );
    }

    #[test]
    fn rejects_regular_messages() {
        for line in [
            "Cloning into 'clone1'...",
            "From file:///tmp/src",
            "   4ab1f38..9c2bb9a  main       -> origin/main",
            "To ../bare.git",
            " * [new branch]      main -> main",
            "fatal: unable to access 'https://example.com/': Could not resolve host",
            "error: 3 files would be overwritten",
            "remote: Create a pull request for 'feature' on GitHub by visiting:",
            "remote:      https://github.com/o/r/pull/new/feature",
            "hint: Updates were rejected",
            "",
        ] {
            assert!(parse_progress_line(line).is_none(), "{line:?}");
        }
    }

    #[test]
    fn noise_lines_are_progress_output() {
        assert!(is_progress_output(
            "Total 560 (delta 154), reused 457 (delta 153), pack-reused 0 (from 0)"
        ));
        assert!(is_progress_output(
            "remote: Total 458 (delta 153), reused 0 (delta 0), pack-reused 0        "
        ));
        assert!(is_progress_output(
            "Delta compression using up to 6 threads"
        ));
        assert!(!is_progress_output("To ../bare.git"));
    }

    /// Salida real de `git clone --progress file://...` (git 2.53), recortada.
    const CLONE_STDERR: &[u8] = b"Cloning into 'clone1'...\nremote: Enumerating objects: 458, done.        \nremote: Counting objects:   0% (1/458)        \rremote: Counting objects: 100% (458/458)        \rremote: Counting objects: 100% (458/458), done.        \nremote: Compressing objects:  50% (229/458)        \rremote: Compressing objects: 100% (458/458), done.        \nReceiving objects:   0% (1/458)\rReceiving objects:  51% (234/458), 2.94 MiB | 5.80 MiB/s\rReceiving objects: 100% (458/458), 5.90 MiB | 14.34 MiB/s, done.\nremote: Total 458 (delta 153), reused 0 (delta 0), pack-reused 0 (from 0)        \nResolving deltas:   0% (0/153)\rResolving deltas: 100% (153/153), done.\n";

    #[test]
    fn filter_splits_a_real_clone() {
        let mut filter = ProgressFilter::new();
        let mut updates = Vec::new();
        // En trozos que cortan líneas por la mitad.
        for chunk in CLONE_STDERR.chunks(7) {
            updates.extend(filter.push(chunk));
        }
        updates.extend(filter.finish());
        assert_eq!(
            String::from_utf8(filter.into_kept()).unwrap(),
            "Cloning into 'clone1'...\n"
        );
        let phases: Vec<_> = updates.iter().map(|update| update.phase.as_str()).collect();
        assert_eq!(phases.first(), Some(&"Enumerating objects"));
        assert_eq!(phases.last(), Some(&"Resolving deltas"));
        assert_eq!(updates.len(), 11);
        let receiving = &updates[7];
        assert_eq!(receiving.phase, "Receiving objects");
        assert_eq!(receiving.throughput.as_deref(), Some("5.80 MiB/s"));
    }

    /// Salida real de `git push --progress` y de `git fetch --progress`.
    #[test]
    fn filter_keeps_the_push_summary_and_errors() {
        let push = b"Enumerating objects: 560, done.\nCounting objects: 100% (560/560), done.\nDelta compression using up to 6 threads\nCompressing objects: 100% (407/407), done.\nWriting objects:  40% (224/560), 3.1 MiB | 6.2 MiB/s\rWriting objects: 100% (560/560), 7.86 MiB | 29.48 MiB/s, done.\nTotal 560 (delta 154), reused 457 (delta 153), pack-reused 0 (from 0)\nremote: Resolving deltas: 100% (154/154), done.        \nTo ../bare.git\n * [new branch]      main -> main\n";
        let mut filter = ProgressFilter::new();
        let updates = filter.push(push);
        assert_eq!(updates.len(), 6);
        assert!(updates[5].remote);
        assert_eq!(
            String::from_utf8(filter.into_kept()).unwrap(),
            "To ../bare.git\n * [new branch]      main -> main\n"
        );

        let failed = b"remote: Counting objects:  10% (1/10)        \rfatal: the remote end hung up unexpectedly\r\nfatal: early EOF";
        let mut filter = ProgressFilter::new();
        assert_eq!(filter.push(failed).len(), 1);
        assert_eq!(
            String::from_utf8(filter.into_kept()).unwrap(),
            "fatal: the remote end hung up unexpectedly\r\nfatal: early EOF"
        );
    }

    #[test]
    fn filter_drops_windows_line_endings_of_progress() {
        let mut filter = ProgressFilter::new();
        let updates =
            filter.push(b"Receiving objects: 100% (3/3), done.\r\nFrom C:\\repos\\src\r\n");
        assert_eq!(updates.len(), 1);
        assert_eq!(
            String::from_utf8(filter.into_kept()).unwrap(),
            "From C:\\repos\\src\r\n"
        );
    }

    #[test]
    fn throttle_admits_phase_changes_and_one_update_per_interval() {
        let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let receiving = |percent: u8| GitProgress {
            phase: "Receiving objects".into(),
            percent: Some(percent),
            ..Default::default()
        };
        assert!(throttle.admit(&receiving(1), at(0)));
        assert!(!throttle.admit(&receiving(2), at(10)));
        assert!(!throttle.admit(&receiving(3), at(99)));
        assert!(throttle.admit(&receiving(4), at(100)));
        let deltas = GitProgress {
            phase: "Resolving deltas".into(),
            ..Default::default()
        };
        assert!(throttle.admit(&deltas, at(101)));
        assert!(!throttle.admit(&deltas, at(150)));
        // Misma fase en el servidor y en local: son fases distintas.
        let remote = GitProgress {
            remote: true,
            ..deltas.clone()
        };
        assert!(throttle.admit(&remote, at(151)));

        // 1000 actualizaciones en un segundo: unas 10 pasan.
        let mut throttle = ProgressThrottle::default();
        let admitted = (0..1000)
            .filter(|ms| throttle.admit(&receiving((ms / 10) as u8), at(*ms)))
            .count();
        assert_eq!(admitted, 10);
    }
}
