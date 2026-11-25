//! Fila de un listado de archivos (`src/components/repository/FileTreeRow.vue`):
//! carpeta plegable o archivo, con lo que cada lista ponga antes (el estado) y
//! después (cuánto cambió, las acciones) del nombre.
//!
//! Filas apretadas: en un commit grande importa cuántos archivos caben sin
//! desplazarse, no el aire alrededor de cada nombre.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement, Interactivity, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, px, rems,
};
use gpui_kit::{Hsla, TestSupportExt as _};
use mergeon_model::file_tree::{FileTreeRow as TreeRow, FileTreeRowType};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide};

use crate::typography::code_font_family;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Fila de carpeta o de archivo. Se crea con [`FileTreeRow::folder`],
/// [`FileTreeRow::file`] o [`FileTreeRow::from_row`].
///
/// ```ignore
/// FileTreeRow::from_row(&row)
///     .expanded(self.is_expanded(&row.path))
///     .selected(selected_path == Some(&row.path))
///     .leading(mergeon_ui::status_badge(&file.status))
///     .trailing(file_stats(file.additions, file.deletions))
///     .on_activate(cx.listener(move |this, _, window, cx| this.select(&path, window, cx)))
///     .on_open_new(cx.listener(move |this, _, window, cx| this.open(&path, window, cx)))
///     .context_menu_items(move |_, _| vec![/* MenuItem… */])   // mergeon_ui::menu::MenuTriggerExt
/// ```
#[derive(IntoElement)]
pub struct FileTreeRow {
    base: Stateful<Div>,
    kind: FileTreeRowType,
    name: SharedString,
    dir: Option<SharedString>,
    path: Option<SharedString>,
    depth: usize,
    expanded: bool,
    selected: bool,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    children: Vec<AnyElement>,
    on_activate: Option<ClickHandler>,
    on_open_new: Option<ClickHandler>,
}

impl FileTreeRow {
    fn new(id: impl Into<ElementId>, kind: FileTreeRowType, name: impl Into<SharedString>) -> Self {
        Self {
            base: div().id(id),
            kind,
            name: name.into(),
            dir: None,
            path: None,
            depth: 0,
            expanded: true,
            selected: false,
            leading: None,
            trailing: None,
            children: Vec::new(),
            on_activate: None,
            on_open_new: None,
        }
    }

    /// Fila de carpeta: flecha, ícono y nombre.
    pub fn folder(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self {
        Self::new(id, FileTreeRowType::Folder, name)
    }

    /// Fila de archivo.
    pub fn file(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self {
        Self::new(id, FileTreeRowType::File, name)
    }

    /// Fila a partir de la que da `mergeon_model::file_tree::build_file_tree_rows`:
    /// tipo, nombre, sangría y ruta; su `key` (`folder:<ruta>` / `file:<ruta>`)
    /// es el id del elemento.
    pub fn from_row<T>(row: &TreeRow<T>) -> Self {
        Self::new(
            SharedString::from(row.key.clone()),
            row.kind,
            row.name.clone(),
        )
        .depth(row.depth)
        .path(row.path.clone())
    }

    /// Otro texto en lugar del nombre de la fila (un renombre: `viejo → nuevo`).
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    /// Nivel de sangría (0.9rem por nivel).
    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// Carpeta desplegada (flecha hacia abajo, carpeta abierta). Por defecto sí.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// En una lista plana la carpeta acompaña al nombre, en gris.
    pub fn dir(mut self, dir: impl Into<SharedString>) -> Self {
        let dir = dir.into();
        self.dir = (!dir.is_empty()).then_some(dir);
        self
    }

    /// Ruta completa, que se enseña al pasar el cursor por el nombre (`title`).
    pub fn path(mut self, path: impl Into<SharedString>) -> Self {
        let path = path.into();
        self.path = (!path.is_empty()).then_some(path);
        self
    }

    /// Lo que la lista pone antes del nombre de un archivo (su estado) o, en
    /// una carpeta, entre la flecha y el ícono (una casilla de carpeta).
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Y después: cuánto cambió, o las acciones de stage y discard.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// El clic (`@activate`): en un archivo, seleccionarlo; en una carpeta,
    /// plegarla o desplegarla.
    pub fn on_activate(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }

    /// El doble clic (`@open-new`), que en una pestaña de archivo significa
    /// abrir otra en vez de cambiar la actual. Como en el original, llega
    /// después de los dos `on_activate` de sus clics.
    pub fn on_open_new(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_new = Some(Rc::new(handler));
        self
    }

    /// El botón secundario (`@contextmenu`), para quien abra su menú a mano.
    /// Para un menú de `mergeon_ui` basta `.context_menu_items(..)`.
    pub fn on_context_menu(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.base = self.base.on_mouse_down(MouseButton::Right, handler);
        self
    }

    pub fn is_folder(&self) -> bool {
        self.kind == FileTreeRowType::Folder
    }
}

impl Styled for FileTreeRow {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for FileTreeRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for FileTreeRow {}

/// Los hijos van al final de la fila, tras `trailing`.
impl ParentElement for FileTreeRow {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for FileTreeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let is_folder = self.is_folder();
        // `.tree-arrow` / `.tree-icon`: hueco de 0.9rem con el ícono centrado.
        let slot = || {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .w(rems(0.9))
        };
        // Señal: la fila seleccionada se invierte a tinta con la marca naranja.
        let selected = self.selected;
        let (text, quiet) = if selected {
            (palette.sel_fg, palette.sel_fg_2)
        } else {
            (palette.foreground, palette.muted_foreground)
        };
        let label = |text_color, value: SharedString| {
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(rems(0.8125))
                .text_color(text_color)
                .child(value)
        };
        let on_activate = self.on_activate;
        let on_open_new = self.on_open_new;
        // Guías de sangría: una línea de 1px por nivel, bajo la flecha de la
        // carpeta de ese nivel, para que un archivo de la raíz no parezca
        // dentro de la carpeta de encima. Son `depth` divs por fila pintada
        // (las listas van virtualizadas): nada se recorre por frame.
        let guide = if selected {
            with_alpha(palette.sel_fg, 0.16)
        } else {
            palette.border_hair.into()
        };
        let guides = (0..self.depth).map(move |level| {
            div()
                .id(("tree-guide", level))
                .test_support()
                .absolute()
                .top_0()
                .bottom_0()
                .left(rems(level as f32 * 0.9 + 1.0))
                .w(px(1.))
                .bg(guide)
        });

        self.base
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(rems(0.38))
            .relative()
            .min_h(px(26.))
            .pl(rems(self.depth as f32 * 0.9 + 0.55))
            .pr(rems(0.55))
            .map(|this| {
                if is_folder {
                    this.text_color(palette.foreground)
                } else if selected {
                    this.cursor_pointer()
                } else {
                    this.cursor_pointer()
                        .hover(|style| style.bg(palette.accent))
                }
            })
            .children(guides)
            .when(selected, |this| {
                this.bg(palette.sel_bg).text_color(palette.sel_fg).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(palette.primary),
                )
            })
            .on_click(move |event, window, cx| {
                if !event.standard_click() {
                    return;
                }
                if let Some(on_activate) = &on_activate {
                    on_activate(event, window, cx);
                }
                if event.click_count() == 2
                    && let Some(on_open_new) = &on_open_new
                {
                    on_open_new(event, window, cx);
                }
            })
            .map(|this| {
                if is_folder {
                    let (arrow, folder) = if self.expanded {
                        (Lucide::ChevronDown, Lucide::FolderOpen)
                    } else {
                        (Lucide::ChevronRight, Lucide::Folder)
                    };
                    this.child(
                        slot().child(mergeon_ui::icon(arrow).size(px(12.)).tone(IconTone::Muted)),
                    )
                    .children(self.leading)
                    .child(
                        slot().child(mergeon_ui::icon(folder).size(px(13.)).tone(IconTone::Muted)),
                    )
                    .child(label(text, self.name).flex_1())
                } else {
                    let with_dir = self.dir.is_some();
                    this.child(slot())
                        .children(self.leading)
                        .child(
                            label(text, self.name.clone())
                                .id("tree-label")
                                .test_support()
                                .aria_label(self.name)
                                .map(|this| {
                                    if with_dir {
                                        this.flex_shrink_1()
                                    } else {
                                        this.flex_1()
                                    }
                                })
                                .when_some(self.path.clone(), |this, path| {
                                    this.tooltip(mergeon_ui::tooltip(path))
                                }),
                        )
                        .when_some(self.dir, |this, dir| {
                            this.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(rems(0.75))
                                    .text_color(quiet)
                                    .child(dir),
                            )
                        })
                        .children(self.trailing)
                }
            })
            .children(self.children)
    }
}

/// Cuánto cambió un archivo, con los mismos colores que el diff: `+12 −3`
/// (`.tree-stats`). `None` si no hay nada que contar.
///
/// Con [`FileStats::bars`] añade a la derecha las cinco barritas de Señal
/// (ver [`change_bars`]):
///
/// ```ignore
/// .trailing(div().children(file_stats(file.additions, file.deletions)
///     .map(|stats| stats.bars(true).on_ink(selected))))
/// ```
pub fn file_stats(additions: usize, deletions: usize) -> Option<FileStats> {
    (additions > 0 || deletions > 0).then_some(FileStats {
        additions,
        deletions,
        on_ink: false,
        bars: false,
    })
}

/// Bloques de las barritas de cambio de un archivo (`▮▮▮▮▮`).
pub const CHANGE_BAR_BLOCKS: usize = 5;

/// Qué pinta cada uno de los [`CHANGE_BAR_BLOCKS`] bloques.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeBlock {
    /// Verde: líneas añadidas.
    Added,
    /// Rojo: líneas quitadas.
    Removed,
    /// Gris: lo que sobra.
    Empty,
}

/// La proporción añadido/quitado en cinco bloques. Con cinco líneas
/// cambiadas o más se reparte la proporción redondeando, como el diseño
/// (`+10 −4` son cuatro verdes y un rojo; `+38 −2`, cinco verdes); con menos,
/// cada bloque es una línea y sobran grises, como el `diffstat` de GitHub
/// (`+1` es un verde y cuatro grises).
pub fn change_bars(additions: usize, deletions: usize) -> [ChangeBlock; CHANGE_BAR_BLOCKS] {
    let total = additions + deletions;
    let (added, removed) = if total < CHANGE_BAR_BLOCKS {
        (additions, deletions)
    } else {
        let share =
            |part: usize| (part as f64 / total as f64 * CHANGE_BAR_BLOCKS as f64).round() as usize;
        let added = share(additions).min(CHANGE_BAR_BLOCKS);
        (added, share(deletions).min(CHANGE_BAR_BLOCKS - added))
    };
    std::array::from_fn(|index| {
        if index < added {
            ChangeBlock::Added
        } else if index < added + removed {
            ChangeBlock::Removed
        } else {
            ChangeBlock::Empty
        }
    })
}

/// Los bloques como texto (`+` añadido, `−` quitado, `·` gris): la etiqueta
/// accesible de las barritas, p. ej. `+++−·`.
pub fn bar_pattern(blocks: &[ChangeBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ChangeBlock::Added => '+',
            ChangeBlock::Removed => '\u{2212}',
            ChangeBlock::Empty => '\u{b7}',
        })
        .collect()
}

/// Altas y bajas de un archivo. Se crea con [`file_stats`].
#[derive(IntoElement, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStats {
    additions: usize,
    deletions: usize,
    on_ink: bool,
    bars: bool,
}

impl FileStats {
    /// Sobre una fila seleccionada en tinta: `sel-add` / `sel-del`.
    pub fn on_ink(mut self, on_ink: bool) -> Self {
        self.on_ink = on_ink;
        self
    }

    /// Añade las cinco barritas de proporción ([`change_bars`]) tras las cifras.
    pub fn bars(mut self, bars: bool) -> Self {
        self.bars = bars;
        self
    }

    pub fn additions(&self) -> usize {
        self.additions
    }

    pub fn deletions(&self) -> usize {
        self.deletions
    }

    pub fn has_bars(&self) -> bool {
        self.bars
    }

    /// Los textos que se pintan: `+N` y `−N` (con el signo menos tipográfico).
    pub fn labels(&self) -> (Option<String>, Option<String>) {
        (
            (self.additions > 0).then(|| format!("+{}", self.additions)),
            (self.deletions > 0).then(|| format!("−{}", self.deletions)),
        )
    }
}

impl RenderOnce for FileStats {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let (additions, deletions) = self.labels();
        let (add, del) = if self.on_ink {
            (palette.sel_add, palette.sel_del)
        } else {
            (palette.success_foreground, palette.destructive_foreground)
        };
        // Bloques: `success` / `danger` y, lo que sobra, el gris del borde; sobre
        // tinta, `sel-add` / `sel-del` y el blanco al 20 %.
        let (bar_add, bar_del, bar_empty): (Hsla, Hsla, Hsla) = if self.on_ink {
            (
                palette.sel_add.into(),
                palette.sel_del.into(),
                with_alpha(palette.sel_fg, 0.2),
            )
        } else {
            (
                palette.success.into(),
                palette.destructive.into(),
                palette.border_base.into(),
            )
        };
        let bars = self.bars.then(|| {
            let blocks = change_bars(self.additions, self.deletions);
            div()
                .id("change-bars")
                .test_support()
                .aria_label(SharedString::from(bar_pattern(&blocks)))
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(1.))
                // 6px tras las cifras (`.bars { margin-left: 6px }`).
                .ml(px(6.))
                .children(blocks.into_iter().map(|block| {
                    div().flex_shrink_0().w(px(6.)).h(px(8.)).bg(match block {
                        ChangeBlock::Added => bar_add,
                        ChangeBlock::Removed => bar_del,
                        ChangeBlock::Empty => bar_empty,
                    })
                }))
        });

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .font_family(code_font_family(cx))
            .text_size(px(11.5))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .when_some(additions, |this, text| {
                        this.child(div().text_color(add).child(text))
                    })
                    .when_some(deletions, |this, text| {
                        this.child(div().text_color(del).child(text))
                    }),
            )
            .children(bars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChangeBlock::{Added as A, Empty as E, Removed as R};

    #[test]
    fn splits_the_five_blocks_by_the_share_of_added_and_removed_lines() {
        // Los cuatro archivos del diseño.
        assert_eq!(change_bars(10, 4), [A, A, A, A, R]);
        assert_eq!(change_bars(38, 2), [A, A, A, A, A]);
        assert_eq!(change_bars(3, 3), [A, A, A, R, R]);
        assert_eq!(change_bars(14, 0), [A, A, A, A, A]);
        assert_eq!(change_bars(0, 9), [R, R, R, R, R]);
    }

    #[test]
    fn leaves_grey_blocks_when_fewer_than_five_lines_changed() {
        assert_eq!(change_bars(1, 0), [A, E, E, E, E]);
        assert_eq!(change_bars(2, 1), [A, A, R, E, E]);
        assert_eq!(change_bars(0, 0), [E, E, E, E, E]);
    }

    #[test]
    fn a_tiny_share_rounds_away_like_in_the_design() {
        assert_eq!(change_bars(100, 1), [A, A, A, A, A]);
        assert_eq!(change_bars(1, 100), [R, R, R, R, R]);
        // Mitad y mitad: 2,5 bloques redondean hacia arriba y los quitados se
        // quedan con lo que cabe.
        assert_eq!(change_bars(5, 5), [A, A, A, R, R]);
    }
}
