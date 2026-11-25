//! El grafo de una fila del historial (el `<svg>` de `CommitHistoryList.vue`),
//! con la geometría de Señal: carriles cada 14px, trazo de 1,5, transiciones en
//! «escalón de metro» (vertical, curva de radio 5, horizontal y curva) dentro
//! del hueco entre filas, nodos de radio 4 y el anillo naranja de HEAD.

use gpui_kit::{
    App, BorderStyle, Bounds, ContentMask, Hsla, PathBuilder, Pixels, Point, Window, point, px,
    quad, size, transparent_black,
};
use mergeon_model::commit_view_utils::{CommitGraph, GraphEdgeType};
use mergeon_ui::ActivePaletteExt as _;

use crate::colors::css_color;

/// Distancia entre carriles.
pub const GRAPH_LANE_WIDTH: f32 = 14.;
/// Centro del primer carril, medido desde el borde izquierdo.
const GRAPH_FIRST_LANE: f32 = 12.;
/// Alto del grafo: el alto fijo de la fila (densidad estándar de Señal). Las
/// líneas empalman entre commits porque todas las filas miden lo mismo.
pub const GRAPH_HEIGHT: f32 = 28.;
const GRAPH_MIDPOINT: f32 = GRAPH_HEIGHT / 2.;

const LANE_STROKE: f32 = 1.5;
/// Radio de las curvas del escalón de metro.
const STEP_RADIUS: f32 = 5.;
/// Commit: círculo relleno de radio 4. Merge: anillo de 2px relleno del fondo.
const DOT_RADIUS: f32 = 4.;
const MERGE_RING_STROKE: f32 = 2.;
/// HEAD: anillo naranja de radio 7,5 alrededor del nodo.
const HEAD_RING_RADIUS: f32 = 7.5;
const HEAD_RING_STROKE: f32 = 1.5;
/// Seleccionado: contorno `sel-fg` sobre el nodo.
const SELECTED_RING_STROKE: f32 = 1.5;

/// Centro horizontal de un carril (`laneX`).
pub fn lane_x(lane: usize) -> f32 {
    GRAPH_FIRST_LANE + lane as f32 * GRAPH_LANE_WIDTH
}

/// Ancho del grafo para ese número de carriles (`graphWidth`): el mismo margen
/// a los dos lados del primer y el último carril.
pub fn graph_width(lane_count: usize) -> f32 {
    lane_x(lane_count.max(1) - 1) + GRAPH_FIRST_LANE
}

/// Carriles enteros que caben en `width`.
fn lanes_in(width: f32) -> usize {
    ((width - 2. * GRAPH_FIRST_LANE) / GRAPH_LANE_WIDTH)
        .floor()
        .max(-1.) as usize
        + 1
}

/// Parte del ancho de la lista que puede ocupar el grafo.
pub const GRAPH_MAX_SHARE: f32 = 0.35;
/// Por estrecha que sea la lista, el grafo muestra al menos estos carriles.
const GRAPH_MIN_VISIBLE_LANES: usize = 2;

/// Ancho del grafo con `lane_count` carriles en una lista de `list_width`.
///
/// El ancho sale del tramo más ancho de toda la historia cargada: un repo con
/// treinta ramas vivas a la vez en algún punto del pasado pedía 540px de grafo
/// en todas las filas, también arriba donde hay dos carriles, y empujaba el
/// mensaje, el autor y la fecha fuera de la lista. Por eso no pasa de
/// [`GRAPH_MAX_SHARE`] de la lista (en carriles enteros); los carriles que no
/// caben se recortan. Sin ancho medido todavía (`list_width` 0) no se limita.
pub fn fitted_graph_width(lane_count: usize, list_width: f32) -> f32 {
    let natural = graph_width(lane_count);
    if list_width <= 0. {
        return natural;
    }
    let max_lanes = lanes_in(list_width * GRAPH_MAX_SHARE);
    natural.min(graph_width(max_lanes.max(GRAPH_MIN_VISIBLE_LANES)))
}

/// Un tramo del grafo de una fila, en coordenadas de la fila.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraphSegment {
    /// Línea recta de un carril (`<line>`).
    Line {
        x: f32,
        from_y: f32,
        to_y: f32,
        width: f32,
    },
    /// Conexión con la fila siguiente (`<path>`): de media altura al borde
    /// inferior, en escalón de metro cuando cambia de carril.
    Curve { from_x: f32, to_x: f32, width: f32 },
}

/// Lo que se pinta en una fila, con los colores ya resueltos para el tema.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphPaint {
    pub segments: Vec<(GraphSegment, Hsla)>,
    /// Centro del punto del commit.
    pub dot_x: f32,
    pub dot_color: Hsla,
    /// Un merge se pinta como anillo en vez de punto.
    pub merge: bool,
    /// El commit en el que está el working tree lleva un anillo.
    pub head: bool,
    /// Color del anillo de HEAD (el naranja señal).
    pub head_color: Hsla,
    /// Contorno del nodo en la fila seleccionada (`sel-fg`), si lo está.
    pub selected: Option<Hsla>,
    /// Tramo punteado que une los cambios sin commitear con HEAD, si pasa por
    /// esta fila.
    pub working_link: Option<WorkingLink>,
}

/// El tramo punteado (`stroke-dasharray="2 3"`) que baja de la fila de cambios
/// sin commitear hasta el nodo de HEAD, en el carril de HEAD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkingLink {
    /// Carril de HEAD.
    pub lane: usize,
    pub from_y: f32,
    pub to_y: f32,
    pub color: Hsla,
}

impl WorkingLink {
    /// En la fila de HEAD: del borde superior al nodo.
    pub fn into_head(lane: usize, color: Hsla) -> Self {
        Self {
            lane,
            from_y: 0.,
            to_y: GRAPH_MIDPOINT,
            color,
        }
    }

    /// En una fila entre los cambios y HEAD: de arriba abajo.
    pub fn through(lane: usize, color: Hsla) -> Self {
        Self {
            lane,
            from_y: 0.,
            to_y: GRAPH_HEIGHT,
            color,
        }
    }

    /// En la fila de los cambios: del nodo hueco al borde inferior.
    pub fn out_of_working_changes(lane: usize, color: Hsla) -> Self {
        Self {
            lane,
            from_y: GRAPH_MIDPOINT,
            to_y: GRAPH_HEIGHT,
            color,
        }
    }
}

/// Punteado de los tramos que llevan a los cambios sin commitear.
const WORKING_DASH: [f32; 2] = [2., 3.];
/// Nodo de los cambios sin commitear: cuadrado hueco de 9px, borde punteado.
const WORKING_NODE: f32 = 9.;

/// Último carril que se ve entero en `width` (ver [`fitted_graph_width`]).
fn last_visible_lane_x(width: f32) -> f32 {
    lane_x(lanes_in(width).saturating_sub(1))
}

fn paint_working_link(link: &WorkingLink, bounds: Bounds<Pixels>, window: &mut Window) {
    let x = lane_x(link.lane).min(last_visible_lane_x(f32::from(bounds.size.width)));
    let at = |y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
    let mut path = PathBuilder::stroke(px(LANE_STROKE))
        .dash_array(&[px(WORKING_DASH[0]), px(WORKING_DASH[1])]);
    path.move_to(at(link.from_y));
    path.line_to(at(link.to_y));
    if let Ok(path) = path.build() {
        window.paint_path(path, link.color);
    }
}

/// Pinta el grafo de la fila de cambios sin commitear: el nodo hueco de borde
/// punteado en el carril de HEAD y, si hay HEAD en la lista, el tramo que baja
/// hacia él. `surface` es el fondo de la fila (el relleno del nodo).
pub fn paint_working_changes(
    bounds: Bounds<Pixels>,
    link: Option<WorkingLink>,
    lane: usize,
    node_color: Hsla,
    surface: Hsla,
    window: &mut Window,
) {
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        if let Some(link) = &link {
            paint_working_link(link, bounds, window);
        }
        let x = lane_x(lane).min(last_visible_lane_x(f32::from(bounds.size.width)));
        let half = WORKING_NODE / 2.;
        window.paint_quad(quad(
            Bounds::new(
                point(
                    bounds.origin.x + px(x - half),
                    bounds.origin.y + px(GRAPH_MIDPOINT - half),
                ),
                size(px(WORKING_NODE), px(WORKING_NODE)),
            ),
            px(0.),
            surface,
            px(LANE_STROKE),
            node_color,
            BorderStyle::Dashed,
        ));
    });
}

impl GraphPaint {
    /// `graphData(commit)` más la geometría del SVG: `fallback_color` es el
    /// color de la rama del commit, para el que llegue sin grafo calculado.
    pub fn new(graph: Option<&CommitGraph>, fallback_color: &str, head: bool, cx: &App) -> Self {
        let fallback;
        let graph = match graph {
            Some(graph) => graph,
            None => {
                fallback = CommitGraph::fallback(fallback_color);
                &fallback
            }
        };

        let mut segments = Vec::with_capacity(graph.lanes.len() + graph.edges.len());
        for lane in &graph.lanes {
            let from_y = if lane.arrives { 0. } else { GRAPH_MIDPOINT };
            let to_y = if lane.continues {
                GRAPH_HEIGHT
            } else {
                GRAPH_MIDPOINT
            };
            if to_y > from_y {
                segments.push((
                    GraphSegment::Line {
                        x: lane_x(lane.index),
                        from_y,
                        to_y,
                        width: LANE_STROKE,
                    },
                    css_color(&lane.color, cx),
                ));
            }
        }
        for edge in &graph.edges {
            segments.push((
                GraphSegment::Curve {
                    from_x: lane_x(edge.from),
                    to_x: lane_x(edge.to),
                    width: LANE_STROKE,
                },
                css_color(&edge.color, cx),
            ));
        }

        let color: &str = if graph.color.is_empty() {
            fallback_color
        } else {
            &graph.color
        };
        let merge = graph
            .edges
            .iter()
            .any(|edge| edge.kind == GraphEdgeType::Merge && edge.from == graph.lane);
        Self {
            segments,
            dot_x: lane_x(graph.lane),
            dot_color: css_color(color, cx),
            merge,
            head,
            head_color: cx.palette().primary.into(),
            selected: None,
            working_link: None,
        }
    }

    /// Añade el tramo punteado hacia los cambios sin commitear.
    pub fn working_link(mut self, link: Option<WorkingLink>) -> Self {
        self.working_link = link;
        self
    }

    /// Marca el nodo como el de la fila seleccionada (fila en tinta).
    pub fn selected(mut self, selected: bool, cx: &App) -> Self {
        self.selected = selected.then(|| cx.palette().sel_fg.into());
        self
    }

    /// Pinta la fila en `bounds` (su esquina es el origen del SVG).
    /// `surface` es el fondo real de la fila, para el halo del punto.
    ///
    /// Lo que pase del ancho de `bounds` (carriles que no caben, ver
    /// [`fitted_graph_width`]) no se pinta; el punto de un commit que cae
    /// fuera se pinta en el último carril visible, para que ninguna fila se
    /// quede sin su marca.
    pub fn paint(&self, bounds: Bounds<Pixels>, surface: Hsla, window: &mut Window) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.paint_clipped(bounds, surface, window)
        });
    }

    fn paint_clipped(&self, bounds: Bounds<Pixels>, surface: Hsla, window: &mut Window) {
        let at = |x: f32, y: f32| -> Point<Pixels> {
            point(bounds.origin.x + px(x), bounds.origin.y + px(y))
        };

        for (segment, color) in &self.segments {
            let path = match *segment {
                GraphSegment::Line {
                    x,
                    from_y,
                    to_y,
                    width,
                } => {
                    let mut path = PathBuilder::stroke(px(width));
                    path.move_to(at(x, from_y));
                    path.line_to(at(x, to_y));
                    path
                }
                GraphSegment::Curve {
                    from_x,
                    to_x,
                    width,
                } => {
                    let mut path = PathBuilder::stroke(px(width));
                    path.move_to(at(from_x, GRAPH_MIDPOINT));
                    if from_x == to_x || (to_x - from_x).abs() < 2. * STEP_RADIUS {
                        path.line_to(at(to_x, GRAPH_HEIGHT));
                    } else {
                        // Escalón de metro centrado en la mitad inferior de la
                        // fila: baja, gira con radio 5, cruza en horizontal,
                        // vuelve a girar y llega vertical al borde.
                        let side = (to_x - from_x).signum();
                        let r = STEP_RADIUS;
                        // 4/3·tan(π/8): un cuarto de círculo con una cúbica.
                        let k = 0.552_284_8 * r;
                        let turn_y = (GRAPH_MIDPOINT + GRAPH_HEIGHT) / 2. - r;
                        let cross_y = turn_y + r;
                        path.line_to(at(from_x, turn_y));
                        path.cubic_bezier_to(
                            at(from_x + side * r, cross_y),
                            at(from_x, turn_y + k),
                            at(from_x + side * (r - k), cross_y),
                        );
                        path.line_to(at(to_x - side * r, cross_y));
                        path.cubic_bezier_to(
                            at(to_x, cross_y + r),
                            at(to_x - side * (r - k), cross_y),
                            at(to_x, cross_y + r - k),
                        );
                        path.line_to(at(to_x, GRAPH_HEIGHT));
                    }
                    path
                }
            };
            // Un trazo degenerado no se puede teselar: no hay nada que pintar.
            if let Ok(path) = path.build() {
                window.paint_path(path, *color);
            }
        }
        if let Some(link) = &self.working_link {
            paint_working_link(link, bounds, window);
        }

        let last_lane = lanes_in(f32::from(bounds.size.width));
        let dot_x = self.dot_x.min(lane_x(last_lane.saturating_sub(1)));
        let center = at(dot_x, GRAPH_MIDPOINT);
        let disc = |radius: f32| {
            Bounds::new(
                point(center.x - px(radius), center.y - px(radius)),
                size(px(radius * 2.), px(radius * 2.)),
            )
        };
        let ring = |window: &mut Window, radius: f32, stroke: f32, fill: Hsla, color: Hsla| {
            window.paint_quad(quad(
                disc(radius),
                px(radius),
                fill,
                px(stroke),
                color,
                BorderStyle::Solid,
            ));
        };

        if self.head {
            // El anillo se mide por el centro del trazo, como en el SVG.
            let outer = HEAD_RING_RADIUS + HEAD_RING_STROKE / 2.;
            ring(window, outer, HEAD_RING_STROKE, surface, self.head_color);
        }
        if let Some(outline) = self.selected {
            let outer = DOT_RADIUS + SELECTED_RING_STROKE;
            ring(
                window,
                outer,
                SELECTED_RING_STROKE,
                transparent_black(),
                outline,
            );
        }
        if self.merge {
            ring(
                window,
                DOT_RADIUS,
                MERGE_RING_STROKE,
                surface,
                self.dot_color,
            );
        } else {
            ring(window, DOT_RADIUS, 0., self.dot_color, transparent_black());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_are_fourteen_pixels_apart_starting_twelve_from_the_edge() {
        assert_eq!(lane_x(0), 12.);
        assert_eq!(lane_x(2), 40.);
        // 3 carriles: 12 + 2 × 14 + 12; nunca menos de un carril.
        assert_eq!(graph_width(3), 52.);
        assert_eq!(graph_width(0), 24.);
        // El alto del grafo es el alto fijo de la fila (densidad estándar).
        assert_eq!(GRAPH_HEIGHT, 28.);
    }

    #[test]
    fn the_graph_never_takes_more_than_its_share_of_the_list() {
        // Pocos carriles: el ancho natural.
        assert_eq!(fitted_graph_width(3, 415.), 52.);
        // 30 carriles en una lista de 415px: 35% son 145px, 9 carriles.
        assert_eq!(fitted_graph_width(30, 415.), 136.);
        // Una lista muy estrecha conserva dos carriles.
        assert_eq!(fitted_graph_width(30, 60.), 38.);
        // Antes de medir la lista no hay tope.
        assert_eq!(fitted_graph_width(30, 0.), 430.);
    }
}
