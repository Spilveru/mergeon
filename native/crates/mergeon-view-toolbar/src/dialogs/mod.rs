//! Diálogos de la barra: Fetch, Pull, Push, Stash, Create Branch, Operation
//! log y Reflog (`src/components/repository/*Dialog.vue`).
//!
//! Cada uno se abre con su función `open_*` sobre `mergeon_ui::dialog`. El
//! formulario es una entidad que guarda solo lo que el usuario elige; los
//! remotos, las ramas y el resto de datos del repositorio se leen de
//! `AppModel` en cada frame.

mod create_branch;
mod fetch;
mod operations_log;
mod pull;
mod push;
mod reflog;
mod stash;

pub use create_branch::{CreateBranchDialogOptions, open_create_branch_dialog};
pub use fetch::open_fetch_dialog;
pub use operations_log::open_operations_log;
pub use pull::open_pull_dialog;
pub use push::open_push_dialog;
pub use reflog::open_reflog;
pub use stash::{StashChoice, StashEntryPreview, StashSummary, open_stash_dialog, stash_summary};

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarHandle};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled, TestSupportExt as _, Window, div, px, rems,
};
use mergeon_ui::dialog;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, MergeonStyled as _, Radius};

use crate::parts::tint;

/// Un diálogo con ese título ya está abierto en la ventana.
///
/// En la app Vue cada diálogo era un `v-if` sobre una bandera: pedirlo dos
/// veces (el botón y su atajo) lo dejaba abierto una sola vez.
pub(crate) fn is_open(title: &str, window: &Window, cx: &App) -> bool {
    dialog::open_titles(window, cx)
        .iter()
        .any(|open| open.as_ref() == title)
}

/// Insignia con el ícono de la acción (`.git-action-dialog__badge`,
/// `.create-branch-dialog__badge`); `side` es su lado en rems.
pub(crate) fn action_badge(icon: Lucide, side: f32, cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(side))
        .radius(Radius::Xl, cx)
        .border_1()
        .border_color(tint(palette.primary, 0.35))
        .bg(tint(palette.primary, 0.12))
        .child(mergeon_ui::icon(icon).size(px(30.)).tone(IconTone::Active))
}

/// `.git-action-dialog`: la insignia en una columna de 52px y el contenido
/// en la otra.
pub(crate) fn action_layout(
    icon: Lucide,
    content: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> Div {
    div()
        .flex()
        .items_start()
        .gap(rems(0.75))
        .child(
            div()
                .w(px(52.))
                .flex_shrink_0()
                .child(action_badge(icon, 2.8, cx)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(rems(0.6))
                .children(content),
        )
}

/// `.git-action-dialog__field`: etiqueta de 74px y su control.
pub(crate) fn action_field(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(rems(0.4))
        .child(
            div()
                .w(px(74.))
                .flex_shrink_0()
                .text_size(rems(0.8125))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.palette().foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// Ancho de las barras de scroll de la app (`::-webkit-scrollbar { width: 6px }`).
const SCROLLBAR_WIDTH: f32 = 6.;

/// Lista con scroll propio dentro de un diálogo (`.reflog__list`,
/// `.ops-log__list`): `max-height`, `overflow-y: auto` y `padding-right: 0.25rem`.
///
/// `list` es una lista virtual (`uniform_list` o `list`) que lleva el scroll
/// `scroll_handle`: solo construye las filas a la vista. Antes cada render
/// armaba todas.
///
/// La barra del original ocupa sitio cuando la lista no cabe y la de gpui-kit
/// flota encima, así que ese hueco se reserva a mano. Si la lista desborda solo
/// se sabe tras el layout: `overflowing` es lo que se vio en el frame anterior
/// y `on_overflow_change` avisa cuando deja de ser cierto.
pub(crate) fn scroll_list<H: ScrollbarHandle + Clone>(
    id: &'static str,
    list: impl Styled + IntoElement,
    scroll_handle: &H,
    max_height: f32,
    overflowing: bool,
    on_overflow_change: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) -> Div {
    let handle = scroll_handle.clone();
    div()
        .relative()
        .flex()
        .flex_col()
        .min_h_0()
        .child(
            // La lista virtual mide todas sus filas: sin un contenedor que la
            // recorte, su alto completo estiraría el diálogo.
            div()
                .id(id)
                .test_support()
                .flex()
                .flex_col()
                .max_h(px(max_height))
                .overflow_hidden()
                .child(
                    list.min_h_0()
                        .max_h(px(max_height))
                        .pr(rems(0.25))
                        .when(overflowing, |this| this.pr(px(4. + SCROLLBAR_WIDTH))),
                ),
        )
        .vertical_scrollbar(scroll_handle)
        .on_prepaint(move |_, window, cx| {
            let viewport = handle.viewport_bounds().size.height;
            let now_overflowing = handle.content_size().height - viewport > px(0.5);
            if now_overflowing != overflowing {
                on_overflow_change(now_overflowing, window, cx);
            }
        })
}
