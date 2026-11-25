//! Marcas de cambios junto a la barra de desplazamiento del visor de diffs:
//! una tira fina con una marca verde (altas) o roja (bajas) en la altura
//! proporcional de cada bloque de cambios. Un clic lleva a ese bloque.
//!
//! Las posiciones se calculan una vez por documento (diff y vista), al
//! aplicar el diff analizado; al pintar solo se recorren las marcas, que
//! están acotadas por [`MARKER_RESOLUTION`] sea cual sea el tamaño del diff.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, Hsla, Pixels, Window, fill, point, px, size};

/// Cuántas posiciones distintas puede tener una marca: dos bloques que caen
/// en la misma (o en la de al lado) se pintan como una sola. Así un diff con
/// cien mil bloques no pinta cien mil rectángulos por frame.
pub(crate) const MARKER_RESOLUTION: f32 = 1024.;

/// Ancho de la tira, a la derecha de la barra de desplazamiento.
pub(crate) const MARKER_STRIP_WIDTH: f32 = 12.;

/// Alto mínimo de una marca, para que un bloque de una línea en un diff de
/// miles se siga viendo.
const MIN_MARKER_HEIGHT: f32 = 4.;

/// Un bloque de cambios (o varios muy juntos) en la tira.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangeMarker {
    /// Primera fila del primer bloque, en la vista para la que se calculó.
    pub row: usize,
    /// Dónde empieza y acaba, de 0 (arriba) a 1 (abajo).
    pub top: f32,
    pub bottom: f32,
    pub additions: bool,
    pub deletions: bool,
}

/// Las marcas de un documento: por cada fila, si tiene altas y si tiene
/// bajas. Las filas seguidas con cambios forman un bloque.
pub(crate) fn change_markers(rows: &[(bool, bool)]) -> Vec<ChangeMarker> {
    let total = rows.len();
    if total == 0 {
        return Vec::new();
    }
    let position = |row: usize| row as f32 / total as f32;
    let bucket = |fraction: f32| (fraction * MARKER_RESOLUTION) as i64;

    let mut markers: Vec<ChangeMarker> = Vec::new();
    let mut row = 0;
    while row < total {
        let (added, deleted) = rows[row];
        if !added && !deleted {
            row += 1;
            continue;
        }
        let start = row;
        let (mut additions, mut deletions) = (false, false);
        while row < total && (rows[row].0 || rows[row].1) {
            additions |= rows[row].0;
            deletions |= rows[row].1;
            row += 1;
        }
        let (top, bottom) = (position(start), position(row));
        match markers.last_mut() {
            // Pegado (o encima) del anterior a esta resolución: se suman.
            Some(last) if bucket(top) <= bucket(last.bottom) + 1 => {
                last.bottom = bottom;
                last.additions |= additions;
                last.deletions |= deletions;
            }
            _ => markers.push(ChangeMarker {
                row: start,
                top,
                bottom,
                additions,
                deletions,
            }),
        }
    }
    markers
}

/// La marca más cercana a `fraction` (0..1 de la tira): la que la contiene
/// o, si cae entre dos, la más próxima.
pub(crate) fn marker_at(markers: &[ChangeMarker], fraction: f32) -> Option<&ChangeMarker> {
    markers
        .iter()
        .min_by(|left, right| distance(left, fraction).total_cmp(&distance(right, fraction)))
}

fn distance(marker: &ChangeMarker, fraction: f32) -> f32 {
    if fraction < marker.top {
        marker.top - fraction
    } else if fraction > marker.bottom {
        fraction - marker.bottom
    } else {
        0.
    }
}

/// Dónde quedó la tira en el último frame, para traducir un clic.
pub(crate) type StripBounds = Rc<Cell<Bounds<Pixels>>>;

/// Pinta las marcas dentro de `bounds`. Una marca con altas y bajas lleva las
/// dos: las bajas a la izquierda y las altas a la derecha.
pub(crate) fn paint_markers(
    markers: &[ChangeMarker],
    bounds: Bounds<Pixels>,
    additions: Hsla,
    deletions: Hsla,
    window: &mut Window,
) {
    let inset = px(2.);
    let width = (bounds.size.width - inset * 2.).max(px(2.));
    let height = bounds.size.height;
    for marker in markers {
        let top = bounds.top() + height * marker.top;
        let bottom = (bounds.top() + height * marker.bottom).max(top + px(MIN_MARKER_HEIGHT));
        let left = bounds.left() + inset;
        let mut paint = |x: Pixels, w: Pixels, color: Hsla| {
            window.paint_quad(fill(
                Bounds::new(point(x, top), size(w, bottom - top)),
                color,
            ));
        };
        match (marker.deletions, marker.additions) {
            (true, true) => {
                let half = width / 2.;
                paint(left, half, deletions);
                paint(left + half, width - half, additions);
            }
            (true, false) => paint(left, width, deletions),
            (false, true) => paint(left, width, additions),
            (false, false) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD: (bool, bool) = (true, false);
    const DEL: (bool, bool) = (false, true);
    const CTX: (bool, bool) = (false, false);

    #[test]
    fn one_marker_per_block_at_its_proportional_place() {
        let mut rows = vec![CTX; 100];
        rows[10] = DEL;
        rows[11] = ADD;
        rows[50] = ADD;
        rows[51] = ADD;
        rows[99] = DEL;

        let markers = change_markers(&rows);

        assert_eq!(markers.len(), 3);
        assert_eq!(markers[0].row, 10);
        assert_eq!((markers[0].top, markers[0].bottom), (0.10, 0.12));
        assert!(markers[0].additions && markers[0].deletions);
        assert_eq!(markers[1].row, 50);
        assert!(markers[1].additions && !markers[1].deletions);
        assert_eq!(markers[2].row, 99);
        assert_eq!(markers[2].bottom, 1.);
        assert!(!markers[2].additions && markers[2].deletions);
    }

    #[test]
    fn no_changes_no_markers() {
        assert!(change_markers(&[]).is_empty());
        assert!(change_markers(&[CTX, CTX]).is_empty());
    }

    // Un diff enorme no puede dar más marcas que posiciones tiene la tira.
    #[test]
    fn blocks_too_close_to_tell_apart_become_one() {
        let rows: Vec<(bool, bool)> = (0..200_000)
            .map(|row| if row % 2 == 0 { ADD } else { CTX })
            .collect();

        let markers = change_markers(&rows);

        assert!(
            markers.len() <= MARKER_RESOLUTION as usize,
            "{}",
            markers.len()
        );
        assert_eq!(markers[0].row, 0);
        assert_eq!(
            markers.last().map(|marker| marker.bottom),
            Some(199_999. / 200_000.)
        );
    }

    #[test]
    fn a_click_goes_to_the_nearest_marker() {
        let mut rows = vec![CTX; 100];
        rows[10] = ADD;
        rows[80] = DEL;
        let markers = change_markers(&rows);

        assert_eq!(
            marker_at(&markers, 0.105).map(|marker| marker.row),
            Some(10)
        );
        assert_eq!(marker_at(&markers, 0.3).map(|marker| marker.row), Some(10));
        assert_eq!(marker_at(&markers, 0.7).map(|marker| marker.row), Some(80));
        assert_eq!(marker_at(&[], 0.5), None);
    }
}
