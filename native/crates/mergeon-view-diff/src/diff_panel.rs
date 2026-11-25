//! El panel que envuelve al visor de diffs en los cambios locales
//! (`src/components/repository/RepositoryDiffPanel.vue`): cabecera con la ruta
//! y el estado del archivo, los interruptores "Ignore spaces" y "Split", las
//! acciones sobre el archivo y, debajo, el diff o el aviso que toque.
//!
//! En "Local Changes" ([`DiffPanel::side`]) la cabecera dice además qué lado
//! se ve (`Unstaged | Staged`, clicable si el archivo tiene cambios en los
//! dos) y las acciones son las de ese lado: "Stage file" y "Discard" en el
//! working tree, "Unstage file" en el índice. "Discard" nunca toca un lado
//! que no está a la vista.

use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, div, rems,
    transparent_black,
};
use mergeon_model::diff_preferences::DiffViewMode;
use mergeon_model::format::format_size;
use mergeon_ui::style::{DISABLED_OPACITY, focus_ring_overlay};
use mergeon_ui::{ActivePaletteExt as _, Radius, SpinnerColor};

use crate::diff_viewer::{DiffViewer, HunkAction};
use crate::image_diff::{ImageDiffViewer, image_text_toggle, is_svg_path};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type SideHandler = Rc<dyn Fn(&DiffSide, &mut Window, &mut App)>;

/// Qué lado de un archivo de los cambios locales se ve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffSide {
    /// Lo que no está preparado (working tree frente al índice).
    Unstaged,
    /// Lo preparado (índice frente a HEAD).
    Staged,
}

impl DiffSide {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Unstaged => Self::Staged,
            Self::Staged => Self::Unstaged,
        }
    }
}

/// La nota de la vista partida en los cambios locales: los botones de bloque
/// y la selección de líneas solo existen en la unificada.
pub const SPLIT_STAGING_NOTE: &str = "Switch to Unified to stage single blocks or lines.";

/// Texto del interruptor de vista: a qué vista se cambia.
pub fn view_toggle_label(mode: DiffViewMode) -> &'static str {
    match mode {
        DiffViewMode::Unified => "Split",
        DiffViewMode::Split => "Unified",
    }
}

/// Texto de ayuda de "Ignore spaces".
pub fn ignore_whitespace_tooltip(ignore_whitespace: bool) -> &'static str {
    if ignore_whitespace {
        "Ignoring whitespace — click to bring it back"
    } else {
        "Hide whitespace-only changes, so a reindented file shows just what really changed"
    }
}

/// Texto de ayuda de "Whole file".
pub fn whole_file_tooltip(whole_file: bool) -> &'static str {
    if whole_file {
        "Showing the whole file — click to show only the changes"
    } else {
        "Show the whole file, not just the lines around the changes"
    }
}

/// Letra del estado de un archivo en la cabecera del panel: sin seguir y en
/// conflicto se enseñan como `U`.
pub fn panel_status_label(status: &str) -> &str {
    match status {
        "?" | "??" | "U" => "U",
        other => other,
    }
}

/// Aspecto de una acción de la cabecera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionTone {
    Normal,
    /// Interruptor encendido: se distingue del resto de acciones.
    Active,
    /// "Discard": fantasma, no rojo sólido. Es lo más destructivo de la vista
    /// y está pegado a los interruptores; el rojo queda para la confirmación.
    Danger,
}

/// Botón `ghost` `sm` de la cabecera con los colores que el panel le cambia.
/// Es la receta de `mergeon_ui::button(..).ghost().sm()` con otro tono: el
/// botón compartido no deja cambiar su color de hover.
#[derive(IntoElement)]
struct HeaderAction {
    id: &'static str,
    label: SharedString,
    tone: ActionTone,
    disabled: bool,
    tooltip: SharedString,
    on_click: Option<ClickHandler>,
}

impl RenderOnce for HeaderAction {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Lg.px(cx);
        let disabled = self.disabled;
        let focus = window
            .use_keyed_state(ElementId::from(self.id), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = !disabled && focus.is_focused(window);

        let (background, foreground, hover): (Hsla, Hsla, Hsla) = match self.tone {
            ActionTone::Normal => (
                transparent_black(),
                palette.foreground.into(),
                palette.accent.into(),
            ),
            // Señal: el modo activo se invierte a tinta, como un segmentado.
            ActionTone::Active => (
                palette.sel_bg.into(),
                palette.sel_fg.into(),
                palette.sel_bg.into(),
            ),
            ActionTone::Danger => (
                transparent_black(),
                palette.destructive_foreground.into(),
                palette.destructive_soft.into(),
            ),
        };

        BaseButton::new(self.id)
            .disabled(disabled)
            .track_focus(&focus)
            .accessibility_label(self.label.clone())
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .h(rems(1.75))
            .px(rems(9. / 16.))
            .rounded(radius)
            .border_1()
            .border_color(transparent_black())
            .bg(background)
            .text_color(foreground)
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .map(|this| {
                if disabled {
                    this.opacity(DISABLED_OPACITY)
                } else {
                    this.cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .active(move |style| style.bg(hover))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                }
            })
            .tooltip(mergeon_ui::tooltip(self.tooltip))
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .child(self.label)
            .when(focused, |this| this.child(focus_ring_overlay(radius, cx)))
    }
}

/// Panel de diff de los cambios locales. Es un componente controlado: se
/// construye en cada `render` con el estado actual y avisa con sus `on_*`.
/// El visor que envuelve (desplazamiento, selección, cachés) lo guarda quien
/// lo aloja.
///
/// ```ignore
/// DiffPanel::new(&self.diff_viewer)
///     .file(file.path.clone(), file.working_dir.clone())
///     .diff(self.file_diff.clone())
///     .loading(self.loading_diff)
///     .too_large(self.diff_too_large.then_some(self.diff_size))
///     .view_mode(preferences.diff_view_mode)
///     .ignore_whitespace(preferences.ignore_whitespace)
///     .hunk_action(self.hunk_action())
///     .discardable(self.can_discard())
///     .on_toggle_view(cx.listener(|this, _, _, cx| this.toggle_diff_view(cx)))
///     .on_toggle_ignore_whitespace(cx.listener(|this, _, _, cx| this.toggle_whitespace(cx)))
///     .on_discard(cx.listener(|this, _, window, cx| this.discard_selected(window, cx)))
/// ```
///
/// El clic en "Stage hunk" / "Unstage hunk" llega como
/// [`crate::DiffViewerEvent::ApplyHunk`] del visor (`cx.subscribe(&viewer, ..)`).
#[derive(IntoElement)]
pub struct DiffPanel {
    viewer: Entity<DiffViewer>,
    file: Option<(SharedString, SharedString)>,
    diff: Option<SharedString>,
    conflict: Option<AnyElement>,
    loading: bool,
    too_large: Option<u64>,
    view_mode: DiffViewMode,
    ignore_whitespace: bool,
    whole_file: bool,
    whole_file_fell_back: bool,
    hunk_action: Option<HunkAction>,
    discardable: bool,
    image: Option<Entity<ImageDiffViewer>>,
    side: Option<DiffSide>,
    both_sides: bool,
    line_stats: Option<(u64, u64)>,
    stageable: bool,
    on_toggle_view: Option<ClickHandler>,
    on_toggle_ignore_whitespace: Option<ClickHandler>,
    on_toggle_whole_file: Option<ClickHandler>,
    on_discard: Option<ClickHandler>,
    on_stage: Option<ClickHandler>,
    on_unstage: Option<ClickHandler>,
    on_switch_side: Option<SideHandler>,
}

impl DiffPanel {
    pub fn new(viewer: &Entity<DiffViewer>) -> Self {
        Self {
            viewer: viewer.clone(),
            file: None,
            diff: None,
            conflict: None,
            loading: false,
            too_large: None,
            view_mode: DiffViewMode::Unified,
            ignore_whitespace: false,
            whole_file: false,
            whole_file_fell_back: false,
            hunk_action: None,
            discardable: true,
            image: None,
            side: None,
            both_sides: false,
            line_stats: None,
            stageable: true,
            on_toggle_view: None,
            on_toggle_ignore_whitespace: None,
            on_toggle_whole_file: None,
            on_discard: None,
            on_stage: None,
            on_unstage: None,
            on_switch_side: None,
        }
    }

    /// El lado que se ve, en los cambios locales. Con él la cabecera enseña
    /// el segmentado `Unstaged | Staged` y las acciones de ese lado; sin él
    /// (otras vistas), solo "Discard".
    pub fn side(mut self, side: Option<DiffSide>) -> Self {
        self.side = side;
        self
    }

    /// El archivo tiene cambios en los dos lados: el segmentado deja cambiar.
    pub fn both_sides(mut self, both_sides: bool) -> Self {
        self.both_sides = both_sides;
        self
    }

    /// Las cifras `+N −N` del lado que se ve, si se conocen.
    pub fn line_stats(mut self, stats: Option<(u64, u64)>) -> Self {
        self.line_stats = stats;
        self
    }

    /// Si "Stage file" / "Unstage file" están disponibles. Por defecto sí.
    pub fn stageable(mut self, stageable: bool) -> Self {
        self.stageable = stageable;
        self
    }

    /// Clic en "Stage file" (lado sin preparar).
    pub fn on_stage(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_stage = Some(Rc::new(handler));
        self
    }

    /// Clic en "Unstage file" (lado preparado).
    pub fn on_unstage(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_unstage = Some(Rc::new(handler));
        self
    }

    /// Clic en el otro lado del segmentado `Unstaged | Staged`.
    pub fn on_switch_side(
        mut self,
        handler: impl Fn(&DiffSide, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_switch_side = Some(Rc::new(handler));
        self
    }

    /// El archivo seleccionado: su ruta y su estado en el working tree
    /// (`working_dir`: `M`, `D`, `?`…). Sin archivo el panel no pinta nada.
    pub fn file(mut self, path: impl Into<SharedString>, status: impl Into<SharedString>) -> Self {
        self.file = Some((path.into(), status.into()));
        self
    }

    /// Como [`Self::file`], para un archivo que puede no haber.
    pub fn file_opt(mut self, file: Option<(SharedString, SharedString)>) -> Self {
        self.file = file;
        self
    }

    /// El diff del archivo. El panel se lo pasa al visor (con la ruta, que
    /// decide el coloreado); vacío muestra "No changes to display".
    pub fn diff(mut self, diff: impl Into<SharedString>) -> Self {
        self.diff = Some(diff.into());
        self
    }

    /// "Loading diff...".
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// El diff es demasiado grande para mostrarlo; lleva sus bytes.
    pub fn too_large(mut self, size: Option<u64>) -> Self {
        self.too_large = size;
        self
    }

    pub fn view_mode(mut self, mode: DiffViewMode) -> Self {
        self.view_mode = mode;
        self
    }

    /// "Ignore spaces" encendido. Con él no se ofrece preparar bloques sueltos
    /// (git no puede aplicar un diff que se saltó los espacios) y una nota lo
    /// explica; quien usa el panel pasa además `hunk_action(None)`.
    pub fn ignore_whitespace(mut self, ignore_whitespace: bool) -> Self {
        self.ignore_whitespace = ignore_whitespace;
        self
    }

    /// "Whole file" encendido: el diff trae el archivo entero en un solo
    /// bloque. Sin botones de bloque (serían el archivo entero); las líneas
    /// sueltas siguen, y una nota lo dice.
    pub fn whole_file(mut self, whole_file: bool) -> Self {
        self.whole_file = whole_file;
        self
    }

    /// El archivo era demasiado grande para "Whole file" y cayó al diff normal
    /// por bloques: se avisa al pie y se conservan los botones de bloque.
    pub fn whole_file_fell_back(mut self, fell_back: bool) -> Self {
        self.whole_file_fell_back = fell_back;
        self
    }

    /// El archivo es una imagen: en lugar del diff se pinta este visor (las
    /// dos versiones lado a lado). Quien aloja el panel lee las versiones.
    /// Un SVG lleva además el conmutador "Image | Text": con "Text" se pinta
    /// el diff (quien aloja el panel lo pide igual que el de cualquier
    /// archivo).
    pub fn image(mut self, viewer: Option<Entity<ImageDiffViewer>>) -> Self {
        self.image = viewer;
        self
    }

    pub fn hunk_action(mut self, action: Option<HunkAction>) -> Self {
        self.hunk_action = action;
        self
    }

    /// Si "Discard" está disponible (`canDiscard`). Por defecto sí.
    pub fn discardable(mut self, discardable: bool) -> Self {
        self.discardable = discardable;
        self
    }

    /// El archivo está en conflicto: en lugar del diff se pinta este elemento
    /// (el resolvedor de conflictos, que no es de este crate).
    pub fn conflict(mut self, resolver: impl IntoElement) -> Self {
        self.conflict = Some(resolver.into_any_element());
        self
    }

    /// Clic en "Split" / "Unified".
    pub fn on_toggle_view(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_view = Some(Rc::new(handler));
        self
    }

    /// Clic en "Ignore spaces".
    pub fn on_toggle_ignore_whitespace(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_ignore_whitespace = Some(Rc::new(handler));
        self
    }

    /// Clic en "Whole file".
    pub fn on_toggle_whole_file(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_whole_file = Some(Rc::new(handler));
        self
    }

    /// Clic en "Discard".
    pub fn on_discard(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_discard = Some(Rc::new(handler));
        self
    }

    fn placeholder(cx: &App) -> Div {
        div()
            .flex()
            .flex_col()
            .h_full()
            .items_center()
            .justify_center()
            .gap(rems(0.5))
            .text_color(cx.palette().muted_foreground)
    }
}

impl RenderOnce for DiffPanel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let Some((path, status)) = self.file else {
            return div().into_any_element();
        };

        let root = div()
            .id("diff-panel")
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(palette.card)
            .border_l_1()
            .border_color(palette.border);

        if let Some(conflict) = self.conflict {
            return root
                .child(div().flex_1().min_h_0().min_w_0().child(conflict))
                .into_any_element();
        }

        // El visor guarda el diff analizado; aquí solo se le pone al día.
        let (view_mode, hunk_action, diff) = (self.view_mode, self.hunk_action, self.diff);
        let effective_whole_file = self.whole_file && !self.whole_file_fell_back;
        let is_empty = self.viewer.update(cx, |viewer, cx| {
            if let Some(diff) = diff {
                viewer.set_diff(diff, path.clone(), cx);
            }
            viewer.set_view_mode(view_mode, cx);
            viewer.set_hunk_action(hunk_action, cx);
            viewer.set_hunk_buttons(!effective_whole_file, cx);
            viewer.is_empty()
        });
        // Un SVG lleva "Image | Text"; con "Text" se ve su diff como el de
        // cualquier archivo de texto.
        let svg_toggle = self
            .image
            .as_ref()
            .filter(|_| is_svg_path(&path))
            .map(|viewer| image_text_toggle(viewer, cx));
        let image_viewer = self
            .image
            .filter(|viewer| viewer.read(cx).shows_image(&path));
        let lines_only_note =
            effective_whole_file && hunk_action.is_some() && image_viewer.is_none();
        let whole_file_fallback_note = self.whole_file_fell_back && image_viewer.is_none();
        // Una imagen no tiene diff de texto: sin sus interruptores.
        let image = image_viewer.is_some();

        let (folder, name) = match path.rfind('/') {
            Some(at) => (
                Some(SharedString::from(path[..=at].to_string())),
                SharedString::from(path[at + 1..].to_string()),
            ),
            None => (None, path.clone()),
        };
        let toggle_label = view_toggle_label(view_mode);
        let stats = self
            .line_stats
            .and_then(|(additions, deletions)| {
                crate::file_stats(additions as usize, deletions as usize)
            })
            .map(|stats| {
                let (additions, deletions) = stats.labels();
                let label = [additions, deletions]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                div()
                    .id("diff-line-stats")
                    .test_support()
                    .aria_label(SharedString::from(label))
                    .flex_shrink_0()
                    .child(stats)
            });
        // `Unstaged | Staged`: el lado que se ve, invertido a tinta; el otro,
        // clicable solo si el archivo tiene cambios ahí.
        let sides = self.side.map(|side| {
            let segment = |segment: DiffSide| {
                let active = segment == side;
                let enabled = !active && self.both_sides && self.on_switch_side.is_some();
                let handler = self.on_switch_side.clone().filter(|_| enabled);
                HeaderAction {
                    id: match segment {
                        DiffSide::Unstaged => "diff-side-unstaged",
                        DiffSide::Staged => "diff-side-staged",
                    },
                    label: segment.label().into(),
                    tone: if active {
                        ActionTone::Active
                    } else {
                        ActionTone::Normal
                    },
                    disabled: !active && !enabled,
                    tooltip: if active {
                        format!("Showing the {} changes", segment.label().to_lowercase()).into()
                    } else if enabled {
                        format!(
                            "Show the {} changes of this file",
                            segment.label().to_lowercase()
                        )
                        .into()
                    } else {
                        format!("No {} changes in this file", segment.label().to_lowercase()).into()
                    },
                    on_click: handler.map(|handler| -> ClickHandler {
                        Rc::new(move |_, window, cx| handler(&segment, window, cx))
                    }),
                }
            };
            div()
                .id("diff-sides")
                .test_support()
                .aria_label(SharedString::from(side.label()))
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(rems(0.125))
                .p(rems(0.125))
                .rounded(Radius::Lg.px(cx))
                .border_1()
                .border_color(palette.border)
                .child(segment(DiffSide::Unstaged))
                .child(segment(DiffSide::Staged))
        });
        // Las acciones sobre el archivo: las del lado a la vista.
        let file_actions: Vec<HeaderAction> = match self.side {
            None => vec![HeaderAction {
                id: "discard-selected",
                label: "Discard".into(),
                tone: ActionTone::Danger,
                disabled: !self.discardable,
                tooltip: "Discard changes in this file".into(),
                on_click: self.on_discard,
            }],
            Some(DiffSide::Unstaged) => vec![
                HeaderAction {
                    id: "stage-selected",
                    label: "Stage file".into(),
                    tone: ActionTone::Normal,
                    disabled: !self.stageable,
                    tooltip: "Stage every unstaged change in this file (Space in the list)".into(),
                    on_click: self.on_stage,
                },
                HeaderAction {
                    id: "discard-selected",
                    label: "Discard".into(),
                    tone: ActionTone::Danger,
                    disabled: !self.discardable,
                    tooltip: "Discard the unstaged changes in this file (D in the list)".into(),
                    on_click: self.on_discard,
                },
            ],
            // Lo preparado se quita del stage; descartar aquí tocaría un lado
            // que no se ve.
            Some(DiffSide::Staged) => vec![HeaderAction {
                id: "unstage-selected",
                label: "Unstage file".into(),
                tone: ActionTone::Normal,
                disabled: !self.stageable,
                tooltip:
                    "Move the staged changes of this file back to Unstaged (Space in the list)"
                        .into(),
                on_click: self.on_unstage,
            }],
        };
        let split_note =
            self.side.is_some() && view_mode == DiffViewMode::Split && hunk_action.is_some();

        let header = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(rems(0.5))
            .py(rems(0.55))
            .px(rems(0.8))
            .border_b_1()
            .border_color(palette.border)
            .bg(palette.accent)
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap(rems(0.75))
                    .child(
                        div()
                            .id("diff-file-path")
                            .flex()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .tooltip(mergeon_ui::tooltip(path.clone()))
                            .when_some(folder, |this, folder| {
                                this.child(div().text_color(palette.muted_foreground).child(folder))
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.foreground)
                                    .child(name),
                            ),
                    )
                    .child(
                        // El badge de la lista, con su color.
                        div()
                            .id("diff-status")
                            .test_support()
                            .aria_label(SharedString::from(panel_status_label(&status).to_string()))
                            .flex_shrink_0()
                            .child(mergeon_ui::status_badge(&status)),
                    )
                    .children(stats)
                    .children(sides),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rems(0.5))
                    .children(svg_toggle)
                    .when(!image, |this| {
                        this.child(HeaderAction {
                            id: "toggle-ignore-whitespace",
                            label: "Ignore spaces".into(),
                            tone: if self.ignore_whitespace {
                                ActionTone::Active
                            } else {
                                ActionTone::Normal
                            },
                            disabled: false,
                            tooltip: ignore_whitespace_tooltip(self.ignore_whitespace).into(),
                            on_click: self.on_toggle_ignore_whitespace,
                        })
                    })
                    .when(!image, |this| {
                        this.child(HeaderAction {
                            id: "toggle-whole-file",
                            label: "Whole file".into(),
                            tone: if self.whole_file {
                                ActionTone::Active
                            } else {
                                ActionTone::Normal
                            },
                            disabled: false,
                            tooltip: whole_file_tooltip(self.whole_file).into(),
                            on_click: self.on_toggle_whole_file,
                        })
                    })
                    .when(!image, |this| {
                        this.child(HeaderAction {
                            id: "toggle-diff-view",
                            label: toggle_label.into(),
                            tone: ActionTone::Normal,
                            disabled: false,
                            tooltip: format!("Switch to {toggle_label} View").into(),
                            on_click: self.on_toggle_view,
                        })
                    })
                    .children(file_actions),
            );

        let body = if let Some(image) = image_viewer {
            image.into_any_element()
        } else if self.loading {
            Self::placeholder(cx)
                .id("diff-loading")
                .child(mergeon_ui::spinner().color(SpinnerColor::Primary))
                .child("Loading diff...")
                .into_any_element()
        } else if let Some(size) = self.too_large {
            Self::placeholder(cx)
                .id("diff-too-large")
                .child(format!(
                    "Diff too large to display here (over {})",
                    format_size(size as f64)
                ))
                .into_any_element()
        } else if is_empty {
            Self::placeholder(cx)
                .id("diff-empty")
                .child("No changes to display")
                .into_any_element()
        } else {
            self.viewer.clone().into_any_element()
        };

        root.child(header)
            .child(
                // Sin relleno propio: el visor ya deja su margen.
                div().flex().flex_1().min_h_0().min_w_0().child(body),
            )
            // Ocultar las acciones de bloque sin más deja al usuario
            // preguntándose si está roto.
            .when_some(
                if whole_file_fallback_note {
                    Some("File too large for Whole file — showing changes only.")
                } else if self.ignore_whitespace {
                    Some(
                        "Staging single blocks is off while whitespace is ignored: git \
                         cannot apply a diff that skipped it.",
                    )
                } else if split_note && !image {
                    Some(SPLIT_STAGING_NOTE)
                } else if lines_only_note {
                    Some(
                        "Whole file is a single block: pick lines in the gutter to stage or \
                         discard part of it, or switch Whole file off for per-block buttons.",
                    )
                } else {
                    None
                },
                |this, note| {
                    this.child(
                        div()
                            .id("diff-note")
                            .test_support()
                            .aria_label(note)
                            .flex_none()
                            .py(rems(0.35))
                            .px(rems(0.65))
                            .border_t_1()
                            .border_color(palette.border)
                            .text_size(rems(0.6875))
                            .line_height(gpui_kit::relative(1.45))
                            .text_color(palette.muted_foreground)
                            .child(note),
                    )
                },
            )
            .into_any_element()
    }
}
