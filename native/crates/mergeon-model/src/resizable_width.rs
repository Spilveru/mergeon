//! Ancho de un panel que el usuario ajusta arrastrando un divisor. Es la lógica
//! que `useSidebarWidth.js` y `useFileListWidth.js` repetían palabra por
//! palabra: límites, arrastre, teclado y cuándo toca guardar.
//!
//! No escucha el ratón ni guarda nada: la interfaz le pasa las coordenadas y
//! persiste cuando un método devuelve que hay que hacerlo.

use crate::jsstr::js_round;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidthLimits {
    pub default_width: f64,
    pub min_width: f64,
    pub max_width: f64,
    /// Lo mínimo que se deja al contenido de al lado para que el divisor no lo
    /// aplaste.
    pub min_content_width: f64,
}

impl WidthLimits {
    /// Ancho válido más cercano a `value`. `container_width` es el ancho del
    /// contenedor que comparten panel y contenido; 0 es "desconocido".
    pub fn clamp(&self, value: f64, container_width: f64) -> f64 {
        let container_known = container_width != 0.0 && !container_width.is_nan();
        let max = if container_known {
            self.max_width.min(container_width - self.min_content_width)
        } else {
            self.max_width
        };
        let target = if value.is_finite() {
            js_round(value)
        } else {
            self.default_width
        };
        target.max(self.min_width).min(self.min_width.max(max))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizableWidth {
    limits: WidthLimits,
    pub width: f64,
    /// Hay un arrastre en curso.
    pub resizing: bool,
    loaded: bool,
}

impl ResizableWidth {
    pub const fn new(limits: WidthLimits) -> Self {
        Self {
            limits,
            width: limits.default_width,
            resizing: false,
            loaded: false,
        }
    }

    pub fn limits(&self) -> WidthLimits {
        self.limits
    }

    /// Hay que leer el ancho guardado: solo la primera vez que se pregunta.
    pub fn begin_load(&mut self) -> bool {
        if self.loaded {
            return false;
        }
        self.loaded = true;
        true
    }

    /// Adopta el ancho guardado, si lo hay y es un número.
    pub fn apply_saved(&mut self, saved: Option<f64>) {
        if let Some(saved) = saved.filter(|saved| saved.is_finite()) {
            self.width = self.limits.clamp(saved, 0.0);
        }
    }

    /// Empieza el arrastre (mousedown sobre el divisor).
    pub fn start(&mut self) {
        self.resizing = true;
    }

    /// El puntero se movió. `origin_x` es el borde izquierdo del contenedor del
    /// panel y `container_width` su ancho. Fuera de un arrastre no hace nada.
    pub fn drag(&mut self, pointer_x: f64, origin_x: f64, container_width: f64) {
        if !self.resizing {
            return;
        }
        self.width = self.limits.clamp(pointer_x - origin_x, container_width);
    }

    /// Termina el arrastre. `true` si había uno y, por tanto, hay que guardar:
    /// mientras se arrastra solo cambia el ancho en memoria; al soltar se guarda
    /// una vez.
    #[must_use = "true significa que hay que guardar el ancho"]
    pub fn stop(&mut self) -> bool {
        std::mem::take(&mut self.resizing)
    }

    /// Flechas sobre el divisor enfocado. Siempre hay que guardar después.
    pub fn nudge(&mut self, delta: f64, container_width: f64) {
        self.width = self.limits.clamp(self.width + delta, container_width);
    }

    /// Doble clic: vuelve al ancho inicial. Siempre hay que guardar después.
    pub fn reset(&mut self) {
        self.width = self.limits.default_width;
    }

    /// Para que un test empiece de cero.
    pub fn reset_state(&mut self) {
        *self = Self::new(self.limits);
    }
}

/// El ancho como número de ajustes: entero, como lo guardaba el original.
pub(crate) fn width_setting(width: f64) -> serde_json::Value {
    if width.is_finite() && width.fract() == 0.0 && width.abs() < 9.0e15 {
        serde_json::Value::from(width as i64)
    } else {
        serde_json::Value::from(width)
    }
}

/// Un ancho guardado, si es un número.
pub(crate) fn saved_width(value: Option<&serde_json::Value>) -> Option<f64> {
    value
        .and_then(serde_json::Value::as_f64)
        .filter(|width| width.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: WidthLimits = WidthLimits {
        default_width: 100.0,
        min_width: 50.0,
        max_width: 300.0,
        min_content_width: 200.0,
    };

    #[test]
    fn rounds_and_keeps_the_width_between_its_limits() {
        assert_eq!(LIMITS.clamp(120.4, 0.0), 120.0);
        assert_eq!(LIMITS.clamp(120.5, 0.0), 121.0);
        assert_eq!(LIMITS.clamp(10.0, 0.0), 50.0);
        assert_eq!(LIMITS.clamp(900.0, 0.0), 300.0);
        assert_eq!(LIMITS.clamp(f64::INFINITY, 0.0), 100.0);
        // Con el contenedor conocido manda lo que hay que dejarle al contenido...
        assert_eq!(LIMITS.clamp(290.0, 450.0), 250.0);
        // ...pero nunca por debajo del mínimo del propio panel.
        assert_eq!(LIMITS.clamp(290.0, 220.0), 50.0);
        assert_eq!(LIMITS.clamp(290.0, f64::NAN), 290.0);
    }

    #[test]
    fn only_asks_to_load_once_and_ignores_a_saved_value_that_is_not_a_number() {
        let mut width = ResizableWidth::new(LIMITS);

        assert!(width.begin_load());
        assert!(!width.begin_load());
        width.apply_saved(None);
        width.apply_saved(Some(f64::NAN));
        assert_eq!(width.width, 100.0);
        width.apply_saved(Some(4000.0));
        assert_eq!(width.width, 300.0);

        width.reset_state();
        assert_eq!(width.width, 100.0);
        assert!(width.begin_load());
        assert_eq!(width.limits(), LIMITS);
    }

    #[test]
    fn writes_whole_widths_as_integers() {
        assert_eq!(width_setting(300.0).to_string(), "300");
        assert_eq!(saved_width(Some(&serde_json::json!(320))), Some(320.0));
        assert_eq!(saved_width(Some(&serde_json::json!("320"))), None);
        assert_eq!(saved_width(None), None);
    }
}
