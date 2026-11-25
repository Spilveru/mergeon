//! Trazas del arranque: cuánto tarda cada paso desde que empezó el proceso.
//! Se ven con `RUST_LOG=mergeon::startup=info` (el filtro por defecto es
//! `warn`, así que normalmente no cuestan más que una comparación).

use std::sync::OnceLock;
use std::time::Instant;

static ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Fija el instante cero. Lo llama `main` lo antes posible; si nadie lo
/// llama, el cero es la primera traza.
pub fn mark_origin() {
    ORIGIN.get_or_init(Instant::now);
}

/// Milisegundos desde el instante cero.
pub fn elapsed_ms() -> f64 {
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// Anota un paso del arranque.
pub fn trace(step: &str) {
    log::info!(target: "mergeon::startup", "{step}: {:.1} ms", elapsed_ms());
}
