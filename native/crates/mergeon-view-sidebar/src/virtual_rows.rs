//! Filas virtuales dentro del scroll del sidebar.
//!
//! Las secciones comparten un único scroll con la navegación, el filtro, las
//! cabeceras y los textos vacíos, cada uno con su alto. Lo que puede crecer
//! (ramas, ramas remotas, tags, stashes, submódulos) son tramos de filas de
//! alto conocido: [`VirtualRows`] ocupa el alto de todas ellas en el layout,
//! pero solo construye las que caen dentro de la zona visible del scroll que
//! lo contiene (la máscara de contenido del frame). Con miles de ramas cada
//! frame (hover, scroll, una notificación del modelo) cuesta lo mismo que con
//! diez.

use std::ops::Range;
use std::panic::Location;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window, point, px, relative, size,
};

/// El alto de cada fila de un tramo.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowHeights {
    /// `count` filas de `height` cada una.
    Uniform { count: usize, height: Pixels },
    /// Dónde empieza cada fila y, al final, dónde acaba la última
    /// (`count + 1` valores, crecientes).
    Varying(Rc<[Pixels]>),
}

impl RowHeights {
    pub(crate) fn uniform(count: usize, height: Pixels) -> Self {
        RowHeights::Uniform { count, height }
    }

    /// A partir del alto de cada fila y del hueco que la sigue. Los inicios
    /// se redondean a píxeles enteros sobre la suma sin redondear, como
    /// reparte las fracciones el layout de GPUI.
    pub(crate) fn from_extents(extents: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let mut tops = vec![px(0.)];
        let mut cursor = 0f32;
        for (height, gap) in extents {
            cursor += height + gap;
            tops.push(px(cursor.round()));
        }
        RowHeights::Varying(tops.into())
    }

    pub(crate) fn count(&self) -> usize {
        match self {
            RowHeights::Uniform { count, .. } => *count,
            RowHeights::Varying(tops) => tops.len().saturating_sub(1),
        }
    }

    pub(crate) fn total(&self) -> Pixels {
        match self {
            RowHeights::Uniform { count, height } => *height * *count as f32,
            RowHeights::Varying(tops) => tops.last().copied().unwrap_or_default(),
        }
    }

    pub(crate) fn top(&self, ix: usize) -> Pixels {
        match self {
            RowHeights::Uniform { height, .. } => *height * ix as f32,
            RowHeights::Varying(tops) => tops[ix],
        }
    }

    /// El espacio de la fila `ix`: su alto más el hueco que la sigue.
    pub(crate) fn extent(&self, ix: usize) -> Pixels {
        match self {
            RowHeights::Uniform { height, .. } => *height,
            RowHeights::Varying(tops) => tops[ix + 1] - tops[ix],
        }
    }

    /// Las filas que tocan `[start, end)`, medido desde el inicio del tramo.
    pub(crate) fn range(&self, start: Pixels, end: Pixels) -> Range<usize> {
        let count = self.count();
        if count == 0 || end <= start {
            return 0..0;
        }
        match self {
            RowHeights::Uniform { height, .. } => {
                if *height <= px(0.) {
                    return 0..0;
                }
                let first = ((start / *height).floor().max(0.) as usize).min(count);
                let last = ((end / *height).ceil().max(0.) as usize).min(count);
                first..last.max(first)
            }
            RowHeights::Varying(tops) => {
                // Primera fila que acaba después de `start`; primera que
                // empieza en o después de `end`.
                let first = tops[1..].partition_point(|bottom| *bottom <= start);
                let last = tops[..count].partition_point(|top| *top < end);
                first.min(count)..last.max(first).min(count)
            }
        }
    }
}

type RenderRows = Box<dyn Fn(Range<usize>, &mut Window, &mut App) -> Vec<AnyElement>>;

/// Un tramo de filas virtuales. `render_rows` recibe el rango de índices a
/// construir y devuelve una fila por índice; cada fila se coloca en su sitio
/// con el alto que dice [`RowHeights`].
pub(crate) struct VirtualRows {
    id: ElementId,
    heights: RowHeights,
    render_rows: RenderRows,
}

pub(crate) fn virtual_rows(
    id: impl Into<ElementId>,
    heights: RowHeights,
    render_rows: impl Fn(Range<usize>, &mut Window, &mut App) -> Vec<AnyElement> + 'static,
) -> VirtualRows {
    VirtualRows {
        id: id.into(),
        heights,
        render_rows: Box::new(render_rows),
    }
}

impl IntoElement for VirtualRows {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for VirtualRows {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = self.heights.total().into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // Lo que se ve del tramo: la máscara del scroll que lo contiene.
        let visible = window.content_mask().bounds.intersect(&bounds);
        if visible.size.height <= px(0.) || visible.size.width <= px(0.) {
            return Vec::new();
        }
        let range = self.heights.range(
            visible.top() - bounds.top(),
            visible.bottom() - bounds.top(),
        );
        if range.is_empty() {
            return Vec::new();
        }

        let mut rows = (self.render_rows)(range.clone(), window, cx);
        debug_assert_eq!(rows.len(), range.len(), "una fila por índice");
        for (row, ix) in rows.iter_mut().zip(range) {
            row.layout_as_root(
                size(
                    AvailableSpace::Definite(bounds.size.width),
                    AvailableSpace::Definite(self.heights.extent(ix)),
                ),
                window,
                cx,
            );
            row.prepaint_at(
                point(bounds.left(), bounds.top() + self.heights.top(ix)),
                window,
                cx,
            );
        }
        rows
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        rows: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for row in rows {
            row.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_rows_cover_the_visible_span() {
        let heights = RowHeights::uniform(100, px(26.));
        assert_eq!(heights.total(), px(2600.));
        assert_eq!(heights.range(px(0.), px(60.)), 0..3);
        assert_eq!(heights.range(px(26.), px(52.)), 1..2);
        assert_eq!(heights.range(px(2590.), px(4000.)), 99..100);
        assert_eq!(heights.range(px(-50.), px(10.)), 0..1);
        assert_eq!(heights.range(px(10.), px(10.)), 0..0);
    }

    #[test]
    fn varying_rows_round_their_gaps_like_the_layout() {
        // Dos grupos: dos filas de 26 y un hueco de 1.2px tras cada grupo.
        let heights = RowHeights::from_extents([(26., 0.), (26., 1.2), (26., 0.), (26., 1.2)]);
        assert_eq!(heights.count(), 4);
        assert_eq!(heights.top(2), px(53.));
        assert_eq!(heights.extent(1), px(27.));
        assert_eq!(heights.total(), px(106.));
        assert_eq!(heights.range(px(0.), px(26.)), 0..1);
        assert_eq!(heights.range(px(52.5), px(54.)), 1..3);
        assert_eq!(heights.range(px(105.), px(500.)), 3..4);
        assert_eq!(heights.range(px(200.), px(500.)), 4..4);
    }
}
