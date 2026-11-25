//! Piezas que comparten los diálogos de rama (`CheckoutBranchDialog.vue`,
//! `CreateBranchDialog.vue`, `AddRemoteDialog.vue`).

use gpui_kit::prelude::*;
use gpui_kit::{App, Div, SharedString, div, px, rems};
use mergeon_app::LocalChangesMode;
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, MergeonStyled as _, Radius};

/// Insignia cuadrada con el ícono del diálogo (`*-dialog__badge`): borde y
/// fondo teñidos de primario.
pub(crate) fn icon_badge(icon: Lucide, side_rems: f32, icon_px: f32, cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(side_rems))
        .radius(Radius::Xl, cx)
        .border_1()
        .border_color(with_alpha(palette.primary, 0.35))
        .bg(with_alpha(palette.primary, 0.12))
        .child(
            mergeon_ui::icon(icon)
                .size(px(icon_px))
                .tone(IconTone::Active),
        )
}

/// La ficha con el nombre de una rama (`inline-flex ... rounded-lg border
/// bg-background px-2.5 py-1.5 text-sm font-medium`).
pub(crate) fn branch_chip(label: impl Into<SharedString>, cx: &App) -> Div {
    let palette = cx.palette();
    // La fila exterior evita que la ficha se estire al ancho de la columna.
    div().flex().child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .radius(Radius::Lg, cx)
            .border_1()
            .border_color(palette.border)
            .bg(palette.background)
            .px(rems(0.625))
            .py(rems(0.375))
            .tw_text_sm()
            .tw_font_medium()
            .text_color(palette.foreground)
            .child(
                mergeon_ui::icon(Lucide::GitBranch)
                    .size(px(14.))
                    .tone(IconTone::Muted),
            )
            .child(div().min_w_0().truncate().child(label.into())),
    )
}

/// Rótulo pequeño sobre un campo (`text-xs text-muted-foreground`).
pub(crate) fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .tw_text_xs()
        .text_color(cx.palette().muted_foreground)
        .child(text.into())
}

/// El recuadro "Local changes:" con sus tres opciones. `group` es el
/// `RadioGroup` ya armado.
pub(crate) fn local_changes_box(has_local_changes: bool, group: impl IntoElement, cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .radius(Radius::Xl, cx)
        .border_1()
        .border_color(with_alpha(palette.border, 0.8))
        .p(rems(0.625))
        .when(!has_local_changes, |this| this.opacity(0.6))
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(112.))
                        .flex_shrink_0()
                        .tw_text_sm()
                        .tw_font_medium()
                        .text_color(palette.foreground)
                        .child("Local changes:"),
                )
                .child(div().flex_1().min_w_0().child(group)),
        )
        .when(!has_local_changes, |this| {
            this.child(caption(
                "No local changes detected in the working tree.",
                cx,
            ))
        })
}

/// Valor de `localChangesMode` de una opción del grupo.
pub(crate) fn local_changes_mode(value: &str) -> LocalChangesMode {
    match value {
        "stash" => LocalChangesMode::Stash,
        "discard" => LocalChangesMode::Discard,
        _ => LocalChangesMode::Keep,
    }
}
