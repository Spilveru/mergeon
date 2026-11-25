//! Estado vacío (`src/components/ui/empty-state/EmptyState.vue`).

use gpui_kit::component::{IconNamed, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, px, relative, rems,
};

use crate::icon::{AppIcon, IconTone};
use crate::style::{MergeonStyled as _, Radius};
use crate::theme::ActivePaletteExt as _;

/// Estado vacío de Mergeon. Se crea con [`empty_state`].
#[derive(IntoElement)]
pub struct EmptyState {
    title: SharedString,
    description: Option<AnyElement>,
    icon: Option<AppIcon>,
    glyph: Option<SharedString>,
    spin_icon: bool,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

/// Crea un estado vacío con el título dado.
///
/// ```ignore
/// mergeon_ui::empty_state("No repository selected")
///     .description("Choose an open or recent repository from the sidebar to see its summary.")
///     .icon(Lucide::FolderGit2)
///     .child(mergeon_ui::button("open").label("Open repository"))
/// ```
pub fn empty_state(title: impl Into<SharedString>) -> EmptyState {
    EmptyState {
        title: title.into(),
        description: None,
        icon: None,
        glyph: None,
        spin_icon: false,
        children: Vec::new(),
        style: StyleRefinement::default(),
    }
}

impl EmptyState {
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into().into_any_element());
        self
    }

    /// Descripción con formato (p. ej. un hash en mono). Hereda el tamaño y
    /// el tono apagado de la descripción.
    pub fn rich_description(mut self, description: impl IntoElement) -> Self {
        self.description = Some(description.into_any_element());
        self
    }

    /// `iconComponent`: ícono de 18px en tono apagado.
    pub fn icon(mut self, icon: impl IconNamed) -> Self {
        self.icon = Some(AppIcon::new(icon));
        self
    }

    /// `icon`: un carácter o emoji en lugar del ícono.
    pub fn glyph(mut self, glyph: impl Into<SharedString>) -> Self {
        self.glyph = Some(glyph.into());
        self
    }

    /// `spinIcon`: hace girar el ícono (estado de carga).
    pub fn spin_icon(mut self, spin: bool) -> Self {
        self.spin_icon = spin;
        self
    }
}

/// El slot por defecto: acciones centradas bajo la descripción.
impl ParentElement for EmptyState {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for EmptyState {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let badge = self
            .icon
            .map(|icon| {
                icon.size(px(18.))
                    .tone(IconTone::Current)
                    .spin(self.spin_icon)
                    .into_any_element()
            })
            .or_else(|| {
                self.glyph
                    .map(|glyph| div().tw_text_lg().child(glyph).into_any_element())
            });

        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .min_h(rems(8.75))
            .radius(Radius::Md, cx)
            .border_1()
            .border_color(palette.border)
            .bg(palette.bg_base)
            // `.empty` de Señal: `padding: 26px 20px`.
            .px(px(20.))
            .py(px(26.))
            .text_center()
            .refine_style(&self.style)
            .when_some(badge, |this, badge| {
                // Señal: el glifo va en un marco de filete con marcas de esquina
                // naranjas, como un registro de imprenta. GPUI pinta el borde
                // de un elemento encima de sus hijos, así que las marcas son
                // hermanas del marco (pintadas después) y no hijas suyas.
                let mark = |w: f32, h: f32| div().absolute().w(px(w)).h(px(h)).bg(palette.primary);
                this.child(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .mb_3()
                        .size(px(44.))
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .border_1()
                                .border_color(palette.border_base)
                                .text_color(palette.text_2)
                                .child(badge),
                        )
                        .child(mark(7., 1.).top_0().left_0())
                        .child(mark(1., 7.).top_0().left_0())
                        .child(mark(7., 1.).bottom_0().right_0())
                        .child(mark(1., 7.).bottom_0().right_0()),
                )
            })
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .child(self.title),
            )
            .when_some(self.description, |this, description| {
                this.child(
                    div()
                        .mt(px(8.))
                        .max_w(rems(28.))
                        .text_size(px(12.5))
                        .line_height(relative(1.5))
                        .text_color(palette.text_2)
                        .child(description),
                )
            })
            .when(!self.children.is_empty(), |this| {
                this.child(
                    div()
                        .mt(px(12.))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .children(self.children),
                )
            })
    }
}
