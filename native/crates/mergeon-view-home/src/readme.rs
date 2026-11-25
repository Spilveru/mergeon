//! Pinta los bloques de [`crate::markdown`] con las medidas de
//! `.readme-content` de `Dashboard.vue`.
//!
//! El README se interpreta una sola vez por contenido ([`ReadmeDocument`],
//! guardado por la vista con el hash del texto) y se pinta como una lista
//! virtual: cada frame solo construye los bloques visibles, y cada párrafo es
//! un único texto con estilos (no un elemento por palabra).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Div, FontStyle, FontWeight, HighlightStyle, InteractiveElement as _,
    InteractiveText, IntoElement, ListAlignment, ListOffset, ListState, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText,
    Window, div, px, relative, rems,
};
use mergeon_app::AppModel;
use mergeon_ui::{ActivePaletteExt as _, BadgeVariant, MergeonStyled as _, Radius};
use mergeon_view_diff::highlight::{
    Language, SyntaxColors, Token, TokenKind, highlight_tokens, language_for_path,
};

use crate::markdown::{Align, Block, Image, Inline, ListItem, Marks, render_readme};

/// `line-height: 1.55` de `.readme-content`.
const LINE_HEIGHT: f32 = 1.55;
/// Texto del README: 14px.
const TEXT_SIZE: f32 = 0.875;
/// `font-size: 0.8em` de `code` sobre el texto de 14px.
const CODE_SIZE: f32 = TEXT_SIZE * 0.8;
/// Separación bajo un bloque de primer nivel.
const BLOCK_GAP: f32 = 0.65;
/// Separación entre los bloques de un elemento de lista, una cita o un
/// `<details>`.
const NESTED_GAP: f32 = 0.3;

/// Hash del texto del README: la clave del documento ya interpretado.
pub(crate) fn readme_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Una fila de la lista virtual: un bloque de primer nivel, o un elemento de
/// una lista de primer nivel (una lista larga no se pinta entera).
#[derive(Debug)]
enum Row {
    Block(Block),
    Item {
        /// Viñeta o número del elemento.
        marker: Marker,
        item: ListItem,
        /// Último elemento de su lista: lleva el margen de la lista.
        last: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    Bullet,
    Number(u64),
}

/// Lo que comparten las filas de un documento.
struct Shared {
    rows: Vec<Row>,
    /// Resaltado de los bloques de código, calculado al pintarlos por primera
    /// vez. La clave es la dirección del texto del bloque, que no cambia
    /// mientras viva el documento.
    code_tokens: RefCell<HashMap<usize, Rc<[Token]>>>,
}

/// Un README ya interpretado y el estado de su lista virtual.
pub(crate) struct ReadmeDocument {
    hash: u64,
    shared: Rc<Shared>,
    list: ListState,
    /// Alto medio estimado de una fila, para las que aún no se han medido.
    estimate: Pixels,
    /// Ancho para el que se pusieron las estimaciones.
    hinted_width: Cell<Option<Pixels>>,
}

impl ReadmeDocument {
    pub(crate) fn new(text: &str) -> Self {
        Self::from_blocks(readme_hash(text), render_readme(text))
    }

    fn from_blocks(hash: u64, blocks: Vec<Block>) -> Self {
        let mut rows = Vec::with_capacity(blocks.len());
        for block in blocks {
            match block {
                Block::List { start, items } => {
                    let count = items.len();
                    rows.extend(
                        items
                            .into_iter()
                            .enumerate()
                            .map(|(index, item)| Row::Item {
                                marker: marker(start, index),
                                item,
                                last: index + 1 == count,
                            }),
                    );
                }
                block => rows.push(Row::Block(block)),
            }
        }
        // Lo que se dibuja de más por encima y por debajo de lo visible.
        let list = ListState::new(rows.len(), ListAlignment::Top, px(400.));
        let estimate = if rows.is_empty() {
            px(0.)
        } else {
            px(rows.iter().map(estimated_height).sum::<f32>() / rows.len() as f32)
        };
        Self {
            hash,
            shared: Rc::new(Shared {
                rows,
                code_tokens: RefCell::default(),
            }),
            list,
            estimate,
            hinted_width: Cell::new(None),
        }
    }

    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    pub(crate) fn list_state(&self) -> &ListState {
        &self.list
    }

    /// Identidad de lo ya interpretado: no cambia mientras no cambie el texto.
    #[cfg(test)]
    pub(crate) fn parsed_ptr(&self) -> *const () {
        Rc::as_ptr(&self.shared).cast()
    }

    pub(crate) fn scroll_to_top(&self) {
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
    }

    /// La lista virtual con los bloques del README.
    pub(crate) fn element(&self) -> gpui_kit::List {
        // La lista solo conoce el alto de las filas que ya pintó, y olvida
        // todos al cambiar de ancho: sin una estimación para las demás, la
        // barra de scroll de un README largo no llegaría al final. Las
        // medidas reales sustituyen a la estimación según se pintan.
        let width = self.list.viewport_bounds().size.width;
        if width > px(0.) && self.hinted_width.get() != Some(width) {
            self.list.clone().with_uniform_item_height(self.estimate);
            self.hinted_width.set(Some(width));
        }
        let shared = self.shared.clone();
        gpui_kit::list(self.list.clone(), move |index, window, cx| {
            render_row(&shared, index, window, cx)
        })
    }
}

/// Alto aproximado de una fila en px, para la barra de scroll antes de
/// medirla (texto de 14px con línea de ~22px y unos 110 caracteres por línea).
fn estimated_height(row: &Row) -> f32 {
    const LINE: f32 = 22.;
    const GAP: f32 = 10.;
    fn text_lines(text: &str) -> f32 {
        text.split('\n')
            .map(|line| (line.chars().count() as f32 / 110.).ceil().max(1.))
            .sum()
    }
    fn blocks(blocks: &[Block]) -> f32 {
        blocks.iter().map(block).sum()
    }
    fn block(block: &Block) -> f32 {
        match block {
            Block::Heading { content, .. } => text_lines(&content.text) * 26. + 16.,
            Block::Paragraph { content, .. } => text_lines(&content.text) * LINE + GAP,
            Block::Images { .. } => 30.,
            Block::List { items, .. } => items.iter().map(|item| blocks(&item.blocks) + 6.).sum(),
            Block::Quote(inner) => blocks(inner),
            Block::Rule => 20.,
            Block::Code { text, .. } => text.split('\n').count() as f32 * 17. + 32.,
            Block::Table { rows, .. } => (rows.len() + 1) as f32 * 33. + GAP,
            Block::Details { blocks: inner, .. } => blocks(inner) + LINE,
        }
    }
    match row {
        Row::Block(row) => block(row),
        Row::Item { item, .. } => blocks(&item.blocks) + 6.,
    }
}

fn marker(start: Option<u64>, index: usize) -> Marker {
    match start {
        Some(start) => Marker::Number(start.saturating_add(index as u64)),
        None => Marker::Bullet,
    }
}

/// Solo los destinos que un navegador abriría fuera de la app.
fn opens_externally(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:")
}

fn open_external(url: &str, cx: &mut App) {
    let url = url.to_string();
    AppModel::global(cx)
        .update(cx, |model, cx| model.open_external(&url, cx))
        .detach();
}

fn render_row(shared: &Rc<Shared>, index: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(row) = shared.rows.get(index) else {
        return div().into_any_element();
    };
    let id = format!("readme-{index}");
    let element = match row {
        Row::Block(block) => render_block(shared, block, &id, window, cx),
        Row::Item { marker, item, last } => div()
            .when(*last, |this| this.pb(rems(BLOCK_GAP)))
            .child(list_item(shared, *marker, item, &id, window, cx))
            .into_any_element(),
    };
    // `.readme-content`: texto de 14px con línea de 1.55.
    div()
        .w_full()
        // `.readme-panel { padding: .75rem .8rem }` (el vertical, en la lista).
        .px(rems(0.8))
        .text_color(cx.palette().foreground)
        .text_size(rems(TEXT_SIZE))
        .line_height(relative(LINE_HEIGHT))
        .child(element)
        .into_any_element()
}

fn aligned(element: Div, align: Align) -> Div {
    match align {
        Align::Left => element,
        Align::Center => element.text_center().justify_center(),
        Align::Right => element.text_right().justify_end(),
    }
}

/// Un bloque de primer nivel lleva su margen inferior; uno anidado no (lo
/// separa el `gap` de su contenedor).
fn render_block(
    shared: &Rc<Shared>,
    block: &Block,
    id: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let top_level = !id.contains('/');
    let gap = if top_level { BLOCK_GAP } else { 0. };
    let palette = *cx.palette();

    match block {
        Block::Heading {
            level,
            align,
            content,
        } => {
            // `h1 1.15rem`, `h2 0.98rem`, `h3`/`h4 0.875rem`, `h5`/`h6`
            // algo menores; peso 500.
            let size = match level {
                1 => 1.15,
                2 => 0.98,
                3 | 4 => TEXT_SIZE,
                _ => 0.8,
            };
            aligned(div().w_full(), *align)
                .when(top_level && *level <= 2, |this| this.pt(rems(0.35)))
                .when(top_level, |this| this.mb(rems(0.7)))
                .text_size(rems(size))
                .font_weight(FontWeight::MEDIUM)
                .when(*level >= 5, |this| {
                    this.text_color(palette.muted_foreground)
                })
                .child(inline_text(id, content, cx))
                .into_any_element()
        }
        Block::Paragraph { align, content } => aligned(div().w_full(), *align)
            .mb(rems(gap))
            .child(inline_text(id, content, cx))
            .into_any_element(),
        Block::Images { align, images } => aligned(
            div().flex().flex_wrap().items_center().gap_1().w_full(),
            *align,
        )
        .mb(rems(gap))
        .children(
            images
                .iter()
                .enumerate()
                .map(|(index, image)| render_image(image, &format!("{id}-img-{index}"), cx)),
        )
        .into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .mb(rems(gap))
            .children(items.iter().enumerate().map(|(index, item)| {
                list_item(
                    shared,
                    marker(*start, index),
                    item,
                    &format!("{id}/{index}"),
                    window,
                    cx,
                )
            }))
            .into_any_element(),
        Block::Quote(blocks) => div()
            .flex()
            .flex_col()
            .gap(rems(NESTED_GAP))
            .mb(rems(gap))
            .pl(rems(0.75))
            .border_l_2()
            .border_color(palette.border)
            .text_color(palette.muted_foreground)
            .children(nested(shared, blocks, id, window, cx))
            .into_any_element(),
        Block::Rule => div()
            .w_full()
            .h(px(1.))
            .my(rems(0.6))
            .bg(palette.border)
            .into_any_element(),
        Block::Code { language, text } => code_block(shared, id, language.as_deref(), text, cx)
            .mb(rems(gap))
            .test_support()
            .into_any_element(),
        Block::Table {
            alignments,
            header,
            rows,
        } => table(id, alignments, header, rows, cx)
            .mb(rems(gap))
            .into_any_element(),
        Block::Details { summary, blocks } => div()
            .flex()
            .flex_col()
            .gap(rems(NESTED_GAP))
            .mb(rems(gap))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rems(0.35))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(palette.muted_foreground)
                            .child("▾"),
                    )
                    .child(inline_text(&format!("{id}-summary"), summary, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(NESTED_GAP))
                    .pl(rems(1.))
                    .children(nested(shared, blocks, id, window, cx)),
            )
            .into_any_element(),
    }
}

fn nested(
    shared: &Rc<Shared>,
    blocks: &[Block],
    id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| render_block(shared, block, &format!("{id}/{index}"), window, cx))
        .collect()
}

/// `ul { padding-left: 1.15rem }`, `li { margin: .18rem 0 }`.
fn list_item(
    shared: &Rc<Shared>,
    marker: Marker,
    item: &ListItem,
    id: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let palette = *cx.palette();
    let (label, width): (SharedString, f32) = match (item.checked, marker) {
        (Some(true), _) => ("☑".into(), 1.3),
        (Some(false), _) => ("☐".into(), 1.3),
        (None, Marker::Bullet) => ("•".into(), 1.15),
        (None, Marker::Number(number)) => (format!("{number}.").into(), 1.6),
    };
    // El id de un elemento anidado lleva `/`: sus bloques no llevan margen.
    let id = if id.contains('/') {
        id.to_string()
    } else {
        format!("{id}/")
    };
    div()
        .flex()
        .items_start()
        .my(rems(0.18))
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .justify_end()
                .w(rems(width))
                .pr(rems(0.45))
                .when(item.checked.is_some(), |this| {
                    this.text_color(palette.muted_foreground)
                })
                .child(label),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(rems(NESTED_GAP))
                .children(nested(shared, &item.blocks, &id, window, cx)),
        )
        .into_any_element()
}

/// Una imagen de un bloque de imágenes: un badge es una pastilla con su texto
/// alternativo; una imagen grande, `[alt]` atenuado. Si va enlazada, el clic
/// abre el enlace.
fn render_image(image: &Image, id: &str, cx: &mut App) -> AnyElement {
    let palette = *cx.palette();
    let link = image.link.clone().filter(|link| opens_externally(link));
    let label: SharedString = if image.badge {
        if image.alt.is_empty() {
            "badge".into()
        } else {
            image.alt.clone().into()
        }
    } else if image.alt.is_empty() {
        "[image]".into()
    } else {
        format!("[{}]", image.alt).into()
    };

    let content: AnyElement = if image.badge {
        mergeon_ui::badge(label)
            .variant(BadgeVariant::Outline)
            .into_any_element()
    } else {
        div()
            .text_color(palette.muted_foreground)
            .child(label)
            .into_any_element()
    };

    div()
        .id(SharedString::from(id.to_string()))
        .test_support()
        .flex_shrink_0()
        .child(content)
        .when_some(link, |this, link| {
            this.cursor_pointer()
                .hover(|style| style.opacity(0.8))
                .on_click(move |_, _, cx| open_external(&link, cx))
        })
        .into_any_element()
}

/// El estilo de un tramo con formato.
fn highlight(marks: Marks, link: bool, cx: &App) -> HighlightStyle {
    let palette = cx.palette();
    let mut style = HighlightStyle::default();
    if marks.strong {
        style.font_weight = Some(FontWeight::BOLD);
    }
    if marks.emphasis {
        style.font_style = Some(FontStyle::Italic);
    }
    if marks.strike {
        style.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.),
            color: None,
        });
    }
    if marks.code {
        // `code { background: var(--accent) }`
        style.background_color = Some(palette.accent.into());
    }
    if marks.badge {
        style.background_color = Some(palette.bg_pressed.into());
        style.color = Some(palette.text_2.into());
    }
    if marks.image {
        style.color = Some(palette.muted_foreground.into());
    }
    if link {
        style.color = Some(palette.primary_text.into());
    }
    style
}

/// Un párrafo como un único texto con estilos. Los enlaces que se abren fuera
/// son zonas de clic (con la mano como cursor).
fn inline_text(id: &str, inline: &Inline, cx: &App) -> AnyElement {
    let mono = cx.theme().mono_font_family.clone();
    let highlights: Vec<(Range<usize>, HighlightStyle)> = inline
        .runs
        .iter()
        .map(|run| {
            (
                run.range.clone(),
                highlight(run.marks, run.link.is_some(), cx),
            )
        })
        .collect();
    let code: Vec<(Range<usize>, SharedString)> = inline
        .runs
        .iter()
        .filter(|run| run.marks.code)
        .map(|run| (run.range.clone(), mono.clone()))
        .collect();
    let text = StyledText::new(inline.text.clone())
        .with_highlights(highlights)
        .with_font_family_overrides(code);

    let (ranges, urls): (Vec<Range<usize>>, Vec<String>) = inline
        .links()
        .filter(|(_, link)| opens_externally(link))
        .map(|(range, link)| (range, link.to_string()))
        .unzip();
    if ranges.is_empty() {
        return text.into_any_element();
    }
    InteractiveText::new(SharedString::from(id.to_string()), text)
        .on_click(ranges, move |index, _, cx| {
            if let Some(url) = urls.get(index) {
                open_external(url, cx);
            }
        })
        .into_any_element()
}

/// El lenguaje de un bloque de código por su etiqueta (`bash`, `ts`, `rust`…).
fn code_language(label: &str) -> Option<Language> {
    match label {
        "sh" | "shell" | "zsh" | "console" | "shellscript" | "shell-session" => {
            return Some(Language::Bash);
        }
        "js" | "jsx" | "mjs" | "cjs" | "node" => return Some(Language::JavaScript),
        "ts" => return Some(Language::TypeScript),
        "rs" => return Some(Language::Rust),
        "py" => return Some(Language::Python),
        "yml" => return Some(Language::Yaml),
        "html" => return Some(Language::Html),
        "jsonc" | "json5" => return Some(Language::Json),
        _ => {}
    }
    Language::from_name(label).or_else(|| language_for_path(&format!("readme.{label}")))
}

/// `pre { padding: .65rem .75rem; border; radius-lg; background: var(--card) }`,
/// en monoespaciado y con el resaltado del diff.
fn code_block(
    shared: &Rc<Shared>,
    id: &str,
    language: Option<&str>,
    text: &SharedString,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let palette = *cx.palette();
    let mono = cx.theme().mono_font_family.clone();
    let tokens = {
        let key = text.as_ptr() as usize;
        let mut cache = shared.code_tokens.borrow_mut();
        cache
            .entry(key)
            .or_insert_with(|| highlight_tokens(text, language.and_then(code_language)).into())
            .clone()
    };
    let colors = SyntaxColors::new(cx);
    let highlights = tokens.iter().map(|(range, kind)| {
        let mut style = HighlightStyle {
            color: Some(colors.color(*kind)),
            ..HighlightStyle::default()
        };
        if *kind == TokenKind::Keyword {
            style.font_weight = Some(FontWeight::SEMIBOLD);
        }
        if kind.is_italic() {
            style.font_style = Some(FontStyle::Italic);
        }
        (range.clone(), style)
    });

    div()
        .id(SharedString::from(format!("{id}-code")))
        .w_full()
        .px(rems(0.75))
        .py(rems(0.65))
        .border_1()
        .border_color(palette.border)
        .radius(Radius::Lg, cx)
        .bg(palette.card)
        .overflow_x_scroll()
        .font_family(mono)
        .text_size(rems(CODE_SIZE))
        .whitespace_nowrap()
        .child(StyledText::new(text.clone()).with_highlights(highlights.collect::<Vec<_>>()))
}

/// Tabla de GFM: columnas del mismo ancho, cabecera con fondo y filetes.
fn table(id: &str, alignments: &[Align], header: &[Inline], rows: &[Vec<Inline>], cx: &App) -> Div {
    let palette = *cx.palette();
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let row = |cells: &[Inline], row_index: usize, is_header: bool| {
        div()
            .flex()
            .w_full()
            .when(row_index > 0, |this| {
                this.border_t_1().border_color(palette.border)
            })
            .when(is_header, |this| {
                this.bg(mergeon_ui::theme::with_alpha(palette.muted, 0.5))
                    .font_weight(FontWeight::MEDIUM)
            })
            .children((0..columns).map(|column| {
                let cell = div()
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w_0()
                    .px(rems(0.6))
                    .py(rems(0.3))
                    .when(column > 0, |this| {
                        this.border_l_1().border_color(palette.border)
                    });
                let cell = aligned(cell, alignments.get(column).copied().unwrap_or_default());
                match cells.get(column) {
                    Some(content) => cell.child(inline_text(
                        &format!("{id}-cell-{row_index}-{column}"),
                        content,
                        cx,
                    )),
                    None => cell,
                }
            }))
    };

    div()
        .flex()
        .flex_col()
        .w_full()
        .overflow_hidden()
        .border_1()
        .border_color(palette.border)
        .radius(Radius::Lg, cx)
        .child(row(header, 0, true))
        .children(
            rows.iter()
                .enumerate()
                .map(|(index, cells)| row(cells, index + 1, false)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_lists_become_one_row_per_item() {
        let document = ReadmeDocument::new("# Title\n\n3. a\n4. b\n   - nested\n\n- x\n\ntext");
        let rows: Vec<String> = document
            .shared
            .rows
            .iter()
            .map(|row| match row {
                Row::Block(Block::Heading { .. }) => "heading".to_string(),
                Row::Block(Block::Paragraph { .. }) => "paragraph".to_string(),
                Row::Block(other) => format!("{other:?}"),
                Row::Item { marker, item, last } => {
                    format!("{marker:?} {} {last}", item.blocks.len())
                }
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                "heading",
                "Number(3) 1 false",
                "Number(4) 2 true",
                "Bullet 1 true",
                "paragraph",
            ]
        );
        assert_eq!(document.list_state().item_count(), 5);
        assert_eq!(
            document.hash(),
            readme_hash("# Title\n\n3. a\n4. b\n   - nested\n\n- x\n\ntext")
        );
    }

    #[test]
    fn code_labels_map_to_highlight_languages() {
        assert_eq!(code_language("sh"), Some(Language::Bash));
        assert_eq!(code_language("bash"), Some(Language::Bash));
        assert_eq!(code_language("ts"), Some(Language::TypeScript));
        assert_eq!(code_language("rust"), Some(Language::Rust));
        assert_eq!(code_language("json"), Some(Language::Json));
        assert_eq!(code_language("unknown-lang"), None);
    }
}
