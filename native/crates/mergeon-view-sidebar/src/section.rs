//! Estilos base que comparten las secciones del sidebar: la traducción de
//! `src/components/layout/sidebar-section.css`. Cada sección añade lo suyo
//! encima, como los bloques `<style scoped>` del original.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Div, ElementId, FontWeight, Pixels, SharedString, Stateful, Window, div, px,
    relative, rems,
};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, AppIcon, IconTone, Lucide, Radius};

/// `.sidebar-section`: sin líneas entre secciones, solo aire debajo.
pub(crate) fn section() -> Div {
    div().flex().flex_col().flex_shrink_0().pb(rems(0.25))
}

/// Nombre del grupo de hover de una cabecera (revela su `.header-action`).
pub(crate) fn header_group(name: &str) -> SharedString {
    SharedString::from(format!("sidebar-section-header-{name}"))
}

/// `.section-header`: la fila que despliega o pliega una sección.
pub(crate) fn header(name: &'static str, cx: &App) -> Stateful<Div> {
    let palette = cx.palette();
    div()
        .id(ElementId::Name(format!("section-header-{name}").into()))
        .group(header_group(name))
        .flex()
        .items_center()
        .gap(rems(0.375))
        .min_h(px(26.))
        .px(rems(0.625))
        .py(rems(0.2))
        // La letra mono va en `title`: el menú contextual de la cabecera se
        // pinta dentro de ella y heredaría la fuente y el tamaño.
        .text_color(palette.text_3)
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(palette.sidebar_row_hover)
                .text_color(palette.sidebar_foreground)
        })
}

/// La flecha de una cabecera: abajo si la sección está desplegada. Igual
/// (12px) en todas las secciones.
pub(crate) fn chevron(expanded: bool) -> AppIcon {
    mergeon_ui::icon(if expanded {
        Lucide::ChevronDown
    } else {
        Lucide::ChevronRight
    })
    .size(px(12.))
    .tone(IconTone::Muted)
}

/// `.section-title`, en mayúsculas como las etiquetas de sección de Señal
/// (mono, `text-3` heredado de la cabecera).
pub(crate) fn title(text: &'static str, cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.5))
        .line_height(relative(1.4))
        .font_weight(FontWeight::MEDIUM)
        // Con el sidebar estrecho se recorta en vez de partirse en dos líneas.
        .truncate()
        .whitespace_nowrap()
        .child(text.to_uppercase())
}

/// `.section-count`: el contador a la derecha de una cabecera o de un remoto.
pub(crate) fn count(value: usize, cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        // Señal: contador en mono `text-3`, sin caja.
        .min_w(rems(1.125))
        .h(rems(1.125))
        .px(rems(0.2))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.))
        .line_height(relative(1.))
        .text_color(palette.text_3)
        .child(value.to_string())
}

/// El número de una cabecera mientras la sección se lee por primera vez: un
/// spinner en el hueco del contador (no un 0 que luego salta).
pub(crate) fn count_loading() -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .min_w(rems(1.125))
        .h(rems(1.125))
        .child(
            mergeon_ui::icon(Lucide::LoaderCircle)
                .size(px(11.))
                .tone(IconTone::Muted)
                .spin(true),
        )
}

/// `.header-action`: botón que solo se ve con el cursor sobre la cabecera.
/// Va antes del contador, para que los contadores queden alineados a la
/// derecha en todas las secciones.
pub(crate) fn header_action(
    id: &'static str,
    icon: Lucide,
    tooltip: &'static str,
    header: &'static str,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    header_action_with(
        id,
        HeaderActionState::default(),
        icon,
        tooltip,
        header,
        cx,
        on_click,
    )
}

/// Estado de un [`header_action_with`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeaderActionState {
    /// Apagado (una operación en curso): se ve atenuado y no hace nada.
    pub enabled: bool,
    /// El ícono gira y queda visible (algo en marcha).
    pub busy: bool,
}

impl Default for HeaderActionState {
    fn default() -> Self {
        Self {
            enabled: true,
            busy: false,
        }
    }
}

/// [`header_action`] que puede estar apagado o girando.
pub(crate) fn header_action_with(
    id: &'static str,
    state: HeaderActionState,
    icon: Lucide,
    tooltip: &'static str,
    header: &'static str,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let hover = cx.palette().sidebar_row_hover;
    let interactive = state.enabled && !state.busy;
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(1.25))
        .rounded(Radius::Md.px(cx))
        .map(|this| {
            if state.busy {
                // En marcha se ve siempre: es el indicador.
                this.opacity(1.)
            } else {
                let shown = if state.enabled { 1. } else { 0.4 };
                this.opacity(0.)
                    .group_hover(header_group(header), move |style| style.opacity(shown))
            }
        })
        .when(interactive, |this| {
            this.hover(|style| style.bg(hover)).cursor_pointer()
        })
        .when(!interactive, |this| this.cursor_default())
        .tooltip(mergeon_ui::tooltip(tooltip))
        .child(
            mergeon_ui::icon(icon)
                .size(px(13.))
                .tone(IconTone::Active)
                .spin(state.busy),
        )
        // `@click.stop`: no despliega ni pliega la sección.
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            if interactive {
                on_click(event, window, cx);
            }
        })
}

/// `.section-content` (`padding: 0.075rem 0`, 1.2px arriba y abajo).
///
/// GPUI redondea cada medida a píxeles enteros antes de componer; Chromium
/// acumula las fracciones y redondea al pintar. Con 1px + 1px cada sección
/// desplegada quedaría 0.4px corta y las de abajo subirían hasta 2px. Las
/// secciones `roomy` (una de cada dos) llevan 2px arriba, que es como reparte
/// Chromium esas fracciones cuando están todas desplegadas.
pub(crate) fn content(roomy: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .pt(px(if roomy { 2. } else { 1. }))
        .pb(px(1.))
}

/// `.section-empty`: el texto de una sección sin elementos.
pub(crate) fn empty(text: &'static str, cx: &App) -> Div {
    div()
        .pt(rems(0.3))
        .pr(rems(0.875))
        .pb(rems(0.45))
        .pl(rems(1.625))
        .text_size(rems(0.8125))
        .line_height(relative(1.5))
        .text_color(cx.palette().muted_foreground)
        .child(text)
}

/// Alto de una fila de sección: el mínimo de `.section-item` (26px), que
/// ninguna fila sin botones de acción supera.
pub(crate) fn row_height(_rem: Pixels) -> Pixels {
    px(26.)
}

/// `.section-item`: una fila de sección.
pub(crate) fn item(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        // Las listas virtuales maquetan cada fila como raíz: sin ancho, un
        // nombre largo (un stash con su mensaje) la estiraba más allá del
        // sidebar y sus botones quedaban fuera de la vista (QA 0.12.0).
        .w_full()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(rems(0.45))
        .min_h(px(26.))
        .pt(rems(0.2))
        .pr(rems(0.625))
        .pb(rems(0.2))
        .pl(rems(1.625))
        .text_size(rems(0.8125))
        .line_height(relative(1.2))
        .text_color(cx.palette().foreground)
}

/// `.section-item.viewing`: barrita izquierda de la ref cuyo tip se está
/// viendo en el historial. El original la pinta con una sombra interior; aquí
/// es un borde, así que la fila cede esos 2px de su relleno izquierdo.
///
/// Señal: la fila que se está viendo queda seleccionada sin foco (gris).
pub(crate) fn viewing(row: Stateful<Div>, padding_left_rems: f32, cx: &App) -> Stateful<Div> {
    row.bg(cx.palette().sel_inactive)
        .pl(rems(padding_left_rems))
}

/// `.item-name`: el texto de la fila, que ocupa lo que quede y se recorta.
pub(crate) fn item_name(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().truncate().child(text.into())
}

/// Ícono de 13px al inicio de una fila (`.item-icon`).
pub(crate) fn item_icon(icon: Lucide, tone: IconTone) -> AppIcon {
    mergeon_ui::icon(icon).size(px(13.)).tone(tone)
}

/// Señal: la rama actual (HEAD) se marca con un cuadrado naranja de 7px en el
/// hueco del ícono.
pub(crate) fn head_mark(cx: &App) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(13.))
        .child(div().size(px(7.)).bg(cx.palette().primary))
}

/// Grupo de hover de una fila con acciones reveladas.
pub(crate) fn row_group(kind: &str, key: &str) -> SharedString {
    SharedString::from(format!("sidebar-row-{kind}:{key}"))
}

/// `.item-actions`: los botones al final de una fila.
pub(crate) fn item_actions() -> Div {
    div().flex().flex_shrink_0().items_center().gap(rems(0.2))
}

/// Los botones de una fila, revelados al pasar por ella (como en Starred).
/// Van superpuestos al final de la fila: sin el cursor encima no ocupan
/// sitio y el nombre (el mensaje de un stash) usa todo el ancho. La fila
/// necesita `.relative()` y `.group(group)`.
pub(crate) fn item_actions_on_hover(group: SharedString, cx: &App) -> Div {
    let palette = cx.palette();
    item_actions()
        .absolute()
        .top_0()
        .bottom_0()
        .right(rems(0.4))
        .pl(rems(0.3))
        .bg(palette.sidebar_row_hover)
        .opacity(0.)
        .group_hover(group, |style| style.opacity(1.))
}

/// `.item-action`: botón de ícono de una fila (`danger` tiñe su hover).
/// Apagado (`enabled == false`, con una operación en curso) se ve atenuado y
/// no hace nada.
pub(crate) fn item_action(
    id: impl Into<ElementId>,
    icon: Lucide,
    tooltip: &'static str,
    danger: bool,
    enabled: bool,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let palette = cx.palette();
    let hover = if danger {
        with_alpha(palette.destructive, 0.12)
    } else {
        palette.sidebar_row_hover.into()
    };
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(1.35))
        .rounded(Radius::Md.px(cx))
        .map(|this| {
            if enabled {
                this.opacity(0.7)
                    .hover(|style| style.opacity(1.).bg(hover))
                    .cursor_pointer()
            } else {
                this.opacity(0.3).cursor_not_allowed()
            }
        })
        .tooltip(mergeon_ui::tooltip(tooltip))
        .child(mergeon_ui::icon(icon).size(px(13.)).tone(if danger {
            IconTone::Danger
        } else {
            IconTone::Muted
        }))
        // `@click.stop`: no cuenta como clic sobre la fila.
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            if enabled {
                on_click(event, window, cx);
            }
        })
}
