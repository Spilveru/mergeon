//! El README del inicio: Markdown de GitHub (CommonMark + tablas, tachado y
//! listas de tareas, con `pulldown-cmark`) y el HTML que suelen llevar los
//! README reales (`<p align="center">`, logos, badges, `<details>`…).
//!
//! El resultado es una lista de [`Block`] ya resuelta: el texto de cada
//! párrafo va en una sola cadena con sus tramos de formato ([`Inline`]), lista
//! para pintarse como un único texto con estilos. Nada de lo que se pinta
//! conserva etiquetas HTML: las conocidas se traducen y las demás se descartan
//! dejando su texto.

use std::ops::Range;
use std::sync::LazyLock;

use gpui_kit::SharedString;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag,
    TagEnd,
};
use regex::Regex;

/// Alineación horizontal de un bloque (`align="center"` del HTML, columnas de
/// una tabla).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Formato de un tramo de texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marks {
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub strike: bool,
    /// Texto alternativo de una imagen grande (logo, captura): `[alt]` atenuado.
    pub image: bool,
    /// Texto alternativo de un badge dentro de una frase.
    pub badge: bool,
}

/// Un tramo con formato o enlace dentro de un [`Inline`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Rango en bytes dentro de [`Inline::text`].
    pub range: Range<usize>,
    pub marks: Marks,
    /// Destino del enlace, ya saneado (`#` si el protocolo no se admite).
    pub link: Option<String>,
}

/// Texto de un párrafo, título o celda: una sola cadena y los tramos que
/// tienen formato o enlace (lo que no está en ningún tramo va sin formato).
/// Los `\n` son saltos de línea forzados (`<br>`, dos espacios al final).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inline {
    pub text: SharedString,
    pub runs: Vec<Run>,
}

impl Inline {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Los enlaces, con su rango, en orden.
    pub fn links(&self) -> impl Iterator<Item = (Range<usize>, &str)> {
        self.runs
            .iter()
            .filter_map(|run| run.link.as_deref().map(|link| (run.range.clone(), link)))
    }
}

/// Una imagen de un bloque que solo tiene imágenes (logo, fila de badges).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub alt: String,
    pub src: String,
    /// Destino del enlace que la envuelve, ya saneado.
    pub link: Option<String>,
    /// Badge (shields.io, estado de CI…): se pinta como una pastilla con el
    /// texto alternativo. Si no, es una imagen grande y se pinta `[alt]`.
    pub badge: bool,
}

/// Un elemento de una lista.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListItem {
    /// `Some` en una lista de tareas: si está marcada.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// Un bloque del README ya interpretado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        align: Align,
        content: Inline,
    },
    Paragraph {
        align: Align,
        content: Inline,
    },
    /// Un párrafo hecho solo de imágenes: un logo o una fila de badges.
    Images {
        align: Align,
        images: Vec<Image>,
    },
    List {
        /// Número del primer elemento en una lista ordenada.
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    Rule,
    Code {
        language: Option<String>,
        text: SharedString,
    },
    Table {
        alignments: Vec<Align>,
        header: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    /// `<details>`: el `<summary>` y el contenido, siempre desplegado.
    Details {
        summary: Inline,
        blocks: Vec<Block>,
    },
}

/// `sanitizeMarkdownUrl`: solo http(s), mailto y rutas relativas; lo demás
/// queda en `#`.
pub fn sanitize_markdown_url(value: &str) -> String {
    let candidate = value.trim();
    let lower = candidate.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:") {
        return candidate.to_string();
    }
    if candidate.starts_with('#')
        || candidate.starts_with('/')
        || candidate.starts_with("./")
        || candidate.starts_with("../")
    {
        return candidate.to_string();
    }
    "#".to_string()
}

/// Interpreta el README entero.
pub fn render_readme(markdown: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut builder = Builder::default();
    for event in Parser::new_ext(markdown, options) {
        builder.event(event);
    }
    builder.finish()
}

// ---------------------------------------------------------------------------
// HTML incrustado

/// Una pieza de un fragmento de HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HtmlToken {
    Text(String),
    Open {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Close(String),
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Parte un fragmento de HTML en texto y etiquetas. Los comentarios, las
/// declaraciones y el contenido de `<script>` / `<style>` desaparecen; un `<`
/// que no abre una etiqueta es texto.
fn tokenize_html(html: &str) -> Vec<HtmlToken> {
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut rest = html;

    while let Some(position) = rest.find('<') {
        text.push_str(&rest[..position]);
        rest = &rest[position..];

        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }

        let closing = rest.starts_with("</");
        let name_start = if closing { 2 } else { 1 };
        let name_len = rest[name_start..]
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
            .unwrap_or(rest.len() - name_start);
        let starts_with_letter = rest[name_start..].starts_with(|c: char| c.is_ascii_alphabetic());
        if !starts_with_letter || name_len == 0 {
            text.push('<');
            rest = &rest[1..];
            continue;
        }
        let name = rest[name_start..name_start + name_len].to_ascii_lowercase();
        let (attrs, after) = parse_attributes(&rest[name_start + name_len..]);

        if !text.is_empty() {
            tokens.push(HtmlToken::Text(std::mem::take(&mut text)));
        }
        rest = after;
        if closing {
            tokens.push(HtmlToken::Close(name));
        } else if matches!(name.as_str(), "script" | "style" | "template") {
            // Su contenido no es texto del documento.
            let end = format!("</{name}");
            let lower = rest.to_ascii_lowercase();
            rest = match lower.find(&end) {
                Some(index) => rest[index..]
                    .find('>')
                    .map_or("", |close| &rest[index + close + 1..]),
                None => "",
            };
        } else {
            tokens.push(HtmlToken::Open { name, attrs });
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        tokens.push(HtmlToken::Text(text));
    }
    tokens
}

/// Lee los atributos de una etiqueta hasta su `>` y devuelve lo que queda
/// detrás. Los nombres van en minúsculas y los valores sin entidades.
fn parse_attributes(source: &str) -> (Vec<(String, String)>, &str) {
    let mut attrs = Vec::new();
    let mut rest = source;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if rest.is_empty() {
            return (attrs, rest);
        }
        if let Some(after) = rest.strip_prefix('>') {
            return (attrs, after);
        }
        let name_len = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/'))
            .unwrap_or(rest.len());
        let name = rest[..name_len].to_ascii_lowercase();
        rest = rest[name_len..].trim_start();
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            if let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let body = &after[1..];
                let end = body.find(quote).unwrap_or(body.len());
                value = decode_entities(&body[..end]);
                rest = body.get(end + 1..).unwrap_or("");
            } else {
                let mut end = after
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(after.len());
                // `width=20/>`: la barra es del cierre, no del valor.
                if after[..end].ends_with('/') && after[end..].starts_with('>') {
                    end -= 1;
                }
                value = decode_entities(&after[..end]);
                rest = &after[end..];
            }
        }
        if !name.is_empty() {
            attrs.push((name, value));
        }
    }
}

/// Las entidades de HTML más comunes y las numéricas. Las desconocidas se
/// dejan como están.
fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(position) = rest.find('&') {
        decoded.push_str(&rest[..position]);
        rest = &rest[position..];
        let end = rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .map(|end| end + 1);
        let entity = end
            .filter(|end| rest[*end..].starts_with(';'))
            .map(|end| &rest[1..end]);
        let character = entity.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "trade" => Some('™'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "middot" => Some('·'),
            "bull" => Some('•'),
            "rarr" => Some('→'),
            "larr" => Some('←'),
            "laquo" => Some('«'),
            "raquo" => Some('»'),
            "times" => Some('×'),
            "check" => Some('✓'),
            _ => {
                let number = name.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (character, entity) {
            (Some(character), Some(name)) => {
                decoded.push(character);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// Si una imagen es un badge: la URL de un servicio de badges, o una imagen
/// pequeña dentro de un enlace (la fila típica de `[![CI](…)](…)`).
fn is_badge(src: &str, alt: &str, in_link: bool, size: Option<u32>) -> bool {
    let src = src.to_ascii_lowercase();
    const SERVICES: [&str; 9] = [
        "shields.io",
        "badge",
        "badgen.net",
        "travis-ci.",
        "circleci.com",
        "codecov.io",
        "coveralls.io",
        "/workflows/",
        "github.com/sponsors",
    ];
    if SERVICES.iter().any(|service| src.contains(service)) {
        return true;
    }
    if size.is_some_and(|size| size > 48) {
        return false;
    }
    let alt = alt.to_ascii_lowercase();
    let looks_large = ["logo", "banner", "screenshot", "preview", "demo", "hero"]
        .iter()
        .any(|word| alt.contains(word) || src.contains(word));
    in_link && !looks_large || size.is_some_and(|size| size <= 32)
}

// ---------------------------------------------------------------------------
// Construcción de bloques

static BARE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[^\s<>]+").expect("regex de URL"));

/// Una pieza del párrafo en construcción.
#[derive(Debug, Clone)]
enum Piece {
    Text {
        text: String,
        marks: Marks,
        link: Option<String>,
    },
    Image(Image),
    Break,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineKind {
    Paragraph,
    Heading(u8),
    Summary,
    Cell,
}

#[derive(Debug)]
struct InlineBuffer {
    kind: InlineKind,
    align: Align,
    pieces: Vec<Piece>,
}

#[derive(Debug)]
enum FrameKind {
    Root,
    Quote,
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        checked: Option<bool>,
    },
    Details {
        summary: Inline,
    },
    Table {
        alignments: Vec<Align>,
        header: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
        row: Vec<Inline>,
    },
}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    blocks: Vec<Block>,
}

/// Imagen de Markdown en curso: su texto alternativo llega como texto.
#[derive(Debug)]
struct PendingImage {
    src: String,
    alt: String,
}

#[derive(Debug, Default)]
struct Counters {
    strong: usize,
    emphasis: usize,
    code: usize,
    strike: usize,
}

#[derive(Debug)]
struct Builder {
    frames: Vec<Frame>,
    inline: Option<InlineBuffer>,
    /// Alineaciones de los `<p>` / `<div>` abiertos, con su etiqueta.
    aligns: Vec<(String, Align)>,
    counters: Counters,
    /// Enlaces abiertos; `None` para un `<a>` sin `href`.
    links: Vec<Option<String>>,
    image: Option<PendingImage>,
    code_block: Option<(Option<String>, String)>,
    html_block: Option<String>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            frames: vec![Frame {
                kind: FrameKind::Root,
                blocks: Vec::new(),
            }],
            inline: None,
            aligns: Vec::new(),
            counters: Counters::default(),
            links: Vec::new(),
            image: None,
            code_block: None,
            html_block: None,
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn parse_align(value: Option<&str>) -> Option<Align> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "center" | "middle" => Some(Align::Center),
        "right" => Some(Align::Right),
        "left" => Some(Align::Left),
        _ => None,
    }
}

impl Builder {
    fn current_align(&self) -> Align {
        self.aligns.last().map_or(Align::Left, |(_, align)| *align)
    }

    fn current_link(&self) -> Option<String> {
        self.links.iter().rev().find_map(Clone::clone)
    }

    fn marks(&self) -> Marks {
        Marks {
            strong: self.counters.strong > 0,
            emphasis: self.counters.emphasis > 0,
            code: self.counters.code > 0,
            strike: self.counters.strike > 0,
            ..Marks::default()
        }
    }

    fn top(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("siempre queda la raíz")
    }

    fn push_block(&mut self, block: Block) {
        let frame = self.top();
        match &mut frame.kind {
            // Texto suelto dentro de un `<ul>` sin `<li>`.
            FrameKind::List { items, .. } => items.push(ListItem {
                checked: None,
                blocks: vec![block],
            }),
            _ => frame.blocks.push(block),
        }
    }

    fn open_inline(&mut self, kind: InlineKind, align: Align) {
        self.flush();
        self.inline = Some(InlineBuffer {
            kind,
            align,
            pieces: Vec::new(),
        });
    }

    fn ensure_inline(&mut self) -> &mut InlineBuffer {
        if self.inline.is_none() {
            let align = self.current_align();
            self.inline = Some(InlineBuffer {
                kind: InlineKind::Paragraph,
                align,
                pieces: Vec::new(),
            });
        }
        self.inline.as_mut().expect("recién abierto")
    }

    fn push_text(&mut self, text: &str, marks: Marks, link: Option<String>) {
        if text.is_empty() || (self.inline.is_none() && text.trim().is_empty()) {
            return;
        }
        let buffer = self.ensure_inline();
        match buffer.pieces.last_mut() {
            Some(Piece::Text {
                text: last,
                marks: last_marks,
                link: last_link,
            }) if *last_marks == marks && *last_link == link => last.push_str(text),
            _ => buffer.pieces.push(Piece::Text {
                text: text.to_string(),
                marks,
                link,
            }),
        }
    }

    /// Texto de Markdown: las URL sueltas pasan a ser enlaces, como en GitHub.
    fn push_markdown_text(&mut self, text: &str) {
        let marks = self.marks();
        let link = self.current_link();
        if link.is_some() || marks.code {
            self.push_text(text, marks, link);
            return;
        }
        let mut last = 0;
        for found in BARE_URL.find_iter(text) {
            let mut url = found.as_str();
            loop {
                let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"', '*']);
                let trimmed = if trimmed.ends_with(')')
                    && trimmed.matches('(').count() < trimmed.matches(')').count()
                {
                    &trimmed[..trimmed.len() - 1]
                } else {
                    trimmed
                };
                if trimmed.len() == url.len() {
                    break;
                }
                url = trimmed;
            }
            let start = found.start();
            self.push_text(&text[last..start], marks, None);
            self.push_text(url, marks, Some(sanitize_markdown_url(url)));
            last = start + url.len();
        }
        self.push_text(&text[last..], marks, None);
    }

    fn push_break(&mut self) {
        if let Some(buffer) = self.inline.as_mut() {
            buffer.pieces.push(Piece::Break);
        }
    }

    fn push_image(&mut self, alt: String, src: &str, size: Option<u32>) {
        let link = self.current_link();
        let badge = is_badge(src, &alt, link.is_some(), size);
        self.ensure_inline().pieces.push(Piece::Image(Image {
            alt: collapse_whitespace(&alt).trim().to_string(),
            src: sanitize_markdown_url(src),
            link,
            badge,
        }));
    }

    /// Cierra el párrafo (o título, o celda) en curso.
    fn flush(&mut self) {
        let Some(buffer) = self.inline.take() else {
            return;
        };
        let only_images = buffer
            .pieces
            .iter()
            .any(|piece| matches!(piece, Piece::Image(_)))
            && buffer.pieces.iter().all(|piece| match piece {
                Piece::Image(_) | Piece::Break => true,
                Piece::Text { text, .. } => text.trim().is_empty(),
            });

        match buffer.kind {
            InlineKind::Paragraph | InlineKind::Heading(_) if only_images => {
                let images = buffer
                    .pieces
                    .into_iter()
                    .filter_map(|piece| match piece {
                        Piece::Image(image) => Some(image),
                        _ => None,
                    })
                    .collect();
                self.push_block(Block::Images {
                    align: buffer.align,
                    images,
                });
            }
            InlineKind::Paragraph => {
                let content = build_inline(buffer.pieces);
                if !content.is_empty() {
                    self.push_block(Block::Paragraph {
                        align: buffer.align,
                        content,
                    });
                }
            }
            InlineKind::Heading(level) => {
                let content = build_inline(buffer.pieces);
                if !content.is_empty() {
                    self.push_block(Block::Heading {
                        level,
                        align: buffer.align,
                        content,
                    });
                }
            }
            InlineKind::Summary => {
                let content = build_inline(buffer.pieces);
                match self
                    .frames
                    .iter_mut()
                    .rev()
                    .find_map(|frame| match &mut frame.kind {
                        FrameKind::Details { summary } => Some(summary),
                        _ => None,
                    }) {
                    Some(summary) => *summary = content,
                    None if !content.is_empty() => self.push_block(Block::Paragraph {
                        align: buffer.align,
                        content,
                    }),
                    None => {}
                }
            }
            InlineKind::Cell => {
                let content = build_inline(buffer.pieces);
                if let FrameKind::Table { row, .. } = &mut self.top().kind {
                    row.push(content);
                }
            }
        }
    }

    fn push_frame(&mut self, kind: FrameKind) {
        self.flush();
        self.frames.push(Frame {
            kind,
            blocks: Vec::new(),
        });
    }

    /// Cierra el marco de arriba y entrega su bloque al de abajo.
    fn pop_frame(&mut self) {
        self.flush();
        if self.frames.len() <= 1 {
            return;
        }
        let Frame { kind, blocks } = self.frames.pop().expect("hay más de un marco");
        match kind {
            FrameKind::Root => {}
            FrameKind::Quote => self.push_block(Block::Quote(blocks)),
            FrameKind::List { start, items } => {
                if !items.is_empty() {
                    self.push_block(Block::List { start, items });
                }
            }
            FrameKind::Item { checked } => match &mut self.top().kind {
                FrameKind::List { items, .. } => items.push(ListItem { checked, blocks }),
                _ => blocks.into_iter().for_each(|block| self.push_block(block)),
            },
            FrameKind::Details { summary } => self.push_block(Block::Details { summary, blocks }),
            FrameKind::Table {
                alignments,
                header,
                rows,
                ..
            } => self.push_block(Block::Table {
                alignments,
                header,
                rows,
            }),
        }
    }

    /// Cierra hasta el marco más cercano que cumpla `matches`, si lo hay.
    fn close_frame(&mut self, matches: impl Fn(&FrameKind) -> bool) {
        let Some(index) = self.frames.iter().rposition(|frame| matches(&frame.kind)) else {
            return;
        };
        if index == 0 {
            return;
        }
        while self.frames.len() > index {
            self.pop_frame();
        }
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(html) = self.html_block.as_mut() {
            match event {
                Event::Html(text) | Event::InlineHtml(text) | Event::Text(text) => {
                    html.push_str(&text);
                    return;
                }
                Event::End(TagEnd::HtmlBlock) => {
                    let html = self.html_block.take().unwrap_or_default();
                    self.html(&html);
                    self.flush();
                    return;
                }
                _ => {
                    let html = self.html_block.take().unwrap_or_default();
                    self.html(&html);
                }
            }
        }
        if let Some((_, code)) = self.code_block.as_mut() {
            match event {
                Event::Text(text) => {
                    code.push_str(&text);
                    return;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let (language, mut code) = self.code_block.take().unwrap_or_default();
                    if code.ends_with('\n') {
                        code.pop();
                    }
                    self.push_block(Block::Code {
                        language,
                        text: code.replace('\t', "    ").into(),
                    });
                    return;
                }
                _ => {}
            }
        }
        if let Some(image) = self.image.as_mut() {
            match event {
                Event::Text(text) | Event::Code(text) => {
                    image.alt.push_str(&text);
                    return;
                }
                Event::End(TagEnd::Image) => {
                    let PendingImage { src, alt } = self.image.take().expect("imagen abierta");
                    self.push_image(alt, &src, None);
                    return;
                }
                Event::SoftBreak | Event::HardBreak => {
                    image.alt.push(' ');
                    return;
                }
                _ => return,
            }
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_markdown_text(&text),
            Event::Code(text) => {
                let marks = Marks {
                    code: true,
                    ..self.marks()
                };
                let link = self.current_link();
                self.push_text(&text.replace('\n', " "), marks, link);
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                let marks = Marks {
                    code: true,
                    ..self.marks()
                };
                self.push_text(&text, marks, None);
            }
            Event::Html(html) | Event::InlineHtml(html) => self.html(&html),
            Event::FootnoteReference(label) => {
                let marks = self.marks();
                self.push_text(&format!("[{label}]"), marks, None);
            }
            Event::SoftBreak => {
                let marks = self.marks();
                let link = self.current_link();
                self.push_text(" ", marks, link);
            }
            Event::HardBreak => self.push_break(),
            Event::Rule => {
                self.flush();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(frame) = self
                    .frames
                    .iter_mut()
                    .rev()
                    .find(|frame| matches!(frame.kind, FrameKind::Item { .. }))
                    && let FrameKind::Item { checked: slot } = &mut frame.kind
                {
                    *slot = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                let align = self.current_align();
                self.open_inline(InlineKind::Paragraph, align);
            }
            Tag::Heading { level, .. } => {
                let align = self.current_align();
                self.open_inline(InlineKind::Heading(heading_level(level)), align);
            }
            Tag::BlockQuote(kind) => {
                self.push_frame(FrameKind::Quote);
                if let Some(kind) = kind {
                    let label = match kind {
                        BlockQuoteKind::Note => "Note",
                        BlockQuoteKind::Tip => "Tip",
                        BlockQuoteKind::Important => "Important",
                        BlockQuoteKind::Warning => "Warning",
                        BlockQuoteKind::Caution => "Caution",
                    };
                    let marks = Marks {
                        strong: true,
                        ..Marks::default()
                    };
                    self.push_text(label, marks, None);
                    self.flush();
                }
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                        .next()
                        .filter(|language| !language.is_empty())
                        .map(str::to_ascii_lowercase),
                    CodeBlockKind::Indented => None,
                };
                self.code_block = Some((language, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html_block = Some(String::new());
            }
            Tag::List(start) => self.push_frame(FrameKind::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => self.push_frame(FrameKind::Item { checked: None }),
            Tag::Table(alignments) => self.push_frame(FrameKind::Table {
                alignments: alignments
                    .into_iter()
                    .map(|alignment| match alignment {
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                        Alignment::Left | Alignment::None => Align::Left,
                    })
                    .collect(),
                header: Vec::new(),
                rows: Vec::new(),
                row: Vec::new(),
            }),
            Tag::TableHead | Tag::TableRow => {
                if let FrameKind::Table { row, .. } = &mut self.top().kind {
                    row.clear();
                }
            }
            Tag::TableCell => self.open_inline(InlineKind::Cell, Align::Left),
            Tag::Emphasis => self.counters.emphasis += 1,
            Tag::Strong => self.counters.strong += 1,
            Tag::Strikethrough => self.counters.strike += 1,
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                let url = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.to_string(),
                };
                self.links.push(Some(sanitize_markdown_url(&url)));
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some(PendingImage {
                    src: dest_url.to_string(),
                    alt: String::new(),
                });
            }
            Tag::FootnoteDefinition(label) => {
                let align = self.current_align();
                self.open_inline(InlineKind::Paragraph, align);
                let marks = self.marks();
                self.push_text(&format!("[{label}] "), marks, None);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.flush(),
            TagEnd::BlockQuote(_) => self.close_frame(|kind| matches!(kind, FrameKind::Quote)),
            TagEnd::List(_) => self.close_frame(|kind| matches!(kind, FrameKind::List { .. })),
            TagEnd::Item => self.close_frame(|kind| matches!(kind, FrameKind::Item { .. })),
            TagEnd::TableCell => self.flush(),
            TagEnd::TableHead => {
                if let FrameKind::Table { header, row, .. } = &mut self.top().kind {
                    *header = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let FrameKind::Table { rows, row, .. } = &mut self.top().kind {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => self.close_frame(|kind| matches!(kind, FrameKind::Table { .. })),
            TagEnd::Emphasis => self.counters.emphasis = self.counters.emphasis.saturating_sub(1),
            TagEnd::Strong => self.counters.strong = self.counters.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.counters.strike = self.counters.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::FootnoteDefinition => self.flush(),
            _ => {}
        }
    }

    /// Un fragmento de HTML (un bloque entero o una etiqueta dentro de un
    /// párrafo).
    fn html(&mut self, html: &str) {
        for token in tokenize_html(html) {
            match token {
                HtmlToken::Text(text) => {
                    let text = collapse_whitespace(&decode_entities(&text));
                    let marks = self.marks();
                    let link = self.current_link();
                    self.push_text(&text, marks, link);
                }
                HtmlToken::Open { name, attrs } => self.open_tag(&name, &attrs),
                HtmlToken::Close(name) => self.close_tag(&name),
            }
        }
    }

    fn open_tag(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "p" | "div" | "center" | "section" | "article" | "header" | "footer" | "main"
            | "nav" | "figure" | "figcaption" | "pre" | "address" => {
                self.flush();
                let align = if name == "center" {
                    Align::Center
                } else {
                    parse_align(attr(attrs, "align")).unwrap_or_else(|| self.current_align())
                };
                self.aligns.push((name.to_string(), align));
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse().unwrap_or(1);
                let align =
                    parse_align(attr(attrs, "align")).unwrap_or_else(|| self.current_align());
                self.open_inline(InlineKind::Heading(level), align);
            }
            "a" => {
                let href = attr(attrs, "href").map(sanitize_markdown_url);
                self.links.push(href);
            }
            "img" => {
                let alt = attr(attrs, "alt")
                    .or_else(|| attr(attrs, "title"))
                    .unwrap_or_default()
                    .to_string();
                let src = attr(attrs, "src").unwrap_or_default().to_string();
                let size = ["width", "height"]
                    .iter()
                    .filter_map(|key| {
                        let value = attr(attrs, key)?;
                        let digits: String =
                            value.chars().take_while(char::is_ascii_digit).collect();
                        digits.parse::<u32>().ok()
                    })
                    .max();
                self.push_image(alt, &src, size);
            }
            "br" => self.push_break(),
            "hr" => {
                self.flush();
                self.push_block(Block::Rule);
            }
            "b" | "strong" => self.counters.strong += 1,
            "i" | "em" | "cite" => self.counters.emphasis += 1,
            "code" | "kbd" | "tt" | "samp" | "var" => self.counters.code += 1,
            "s" | "del" | "strike" => self.counters.strike += 1,
            "details" => self.push_frame(FrameKind::Details {
                summary: Inline::default(),
            }),
            "summary" => {
                let align = self.current_align();
                self.open_inline(InlineKind::Summary, align);
            }
            "ul" | "ol" => {
                let start = (name == "ol").then(|| {
                    attr(attrs, "start")
                        .and_then(|start| start.trim().parse().ok())
                        .unwrap_or(1)
                });
                self.push_frame(FrameKind::List {
                    start,
                    items: Vec::new(),
                });
            }
            "li" => {
                if matches!(self.top().kind, FrameKind::Item { .. }) {
                    self.pop_frame();
                }
                self.push_frame(FrameKind::Item { checked: None });
            }
            "blockquote" => self.push_frame(FrameKind::Quote),
            "table" | "thead" | "tbody" | "tfoot" | "tr" => self.flush(),
            "td" | "th" if self.inline.is_some() => {
                let marks = self.marks();
                self.push_text(" ", marks, None);
            }
            _ => {}
        }
    }

    fn close_tag(&mut self, name: &str) {
        match name {
            "p" | "div" | "center" | "section" | "article" | "header" | "footer" | "main"
            | "nav" | "figure" | "figcaption" | "pre" | "address" => {
                self.flush();
                if let Some(index) = self.aligns.iter().rposition(|(tag, _)| tag == name) {
                    self.aligns.truncate(index);
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "summary" => self.flush(),
            "a" => {
                self.links.pop();
            }
            "b" | "strong" => self.counters.strong = self.counters.strong.saturating_sub(1),
            "i" | "em" | "cite" => {
                self.counters.emphasis = self.counters.emphasis.saturating_sub(1)
            }
            "code" | "kbd" | "tt" | "samp" | "var" => {
                self.counters.code = self.counters.code.saturating_sub(1)
            }
            "s" | "del" | "strike" => self.counters.strike = self.counters.strike.saturating_sub(1),
            "details" => self.close_frame(|kind| matches!(kind, FrameKind::Details { .. })),
            "ul" | "ol" => self.close_frame(|kind| matches!(kind, FrameKind::List { .. })),
            "li" => self.close_frame(|kind| matches!(kind, FrameKind::Item { .. })),
            "blockquote" => self.close_frame(|kind| matches!(kind, FrameKind::Quote)),
            "table" | "thead" | "tbody" | "tfoot" | "tr" => self.flush(),
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        if let Some(html) = self.html_block.take() {
            self.html(&html);
        }
        self.flush();
        while self.frames.len() > 1 {
            self.pop_frame();
        }
        self.frames
            .pop()
            .map(|frame| frame.blocks)
            .unwrap_or_default()
    }
}

/// Los espacios seguidos (y los saltos de línea) cuentan como uno, como en
/// HTML.
fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        // `&nbsp;` no se colapsa.
        if character.is_whitespace() && character != '\u{a0}' {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(character);
            in_space = false;
        }
    }
    collapsed
}

/// Junta las piezas de un párrafo en un [`Inline`]: colapsa el espacio en
/// blanco como el navegador (sin espacios al principio, al final ni junto a un
/// salto), las imágenes sueltas pasan a ser su texto alternativo y los tramos
/// contiguos con el mismo formato se funden.
fn build_inline(pieces: Vec<Piece>) -> Inline {
    let mut text = String::new();
    let mut runs: Vec<Run> = Vec::new();

    for piece in pieces {
        match piece {
            Piece::Text {
                text: piece,
                marks,
                link,
            } => push_piece(&mut text, &mut runs, &piece, marks, link),
            Piece::Image(image) => {
                let (label, marks) = if image.badge {
                    let label = if image.alt.is_empty() {
                        "badge".to_string()
                    } else {
                        image.alt
                    };
                    let marks = Marks {
                        badge: true,
                        ..Marks::default()
                    };
                    (label, marks)
                } else {
                    let alt = if image.alt.is_empty() {
                        "image"
                    } else {
                        &image.alt
                    };
                    let marks = Marks {
                        image: true,
                        ..Marks::default()
                    };
                    (format!("[{alt}]"), marks)
                };
                if !text.is_empty() && !text.ends_with([' ', '\n']) {
                    push_piece(&mut text, &mut runs, " ", Marks::default(), None);
                }
                push_piece(&mut text, &mut runs, &label, marks, image.link);
            }
            Piece::Break => {
                // Sin espacio antes del salto.
                if text.ends_with(' ') {
                    text.pop();
                    if let Some(last) = runs.last_mut() {
                        last.range.end = last.range.end.min(text.len());
                    }
                }
                text.push('\n');
            }
        }
    }

    // Ni espacios ni saltos al final.
    let trimmed = text.trim_end_matches([' ', '\n']).len();
    text.truncate(trimmed);
    let length = text.len();
    runs.retain_mut(|run| {
        run.range.end = run.range.end.min(length);
        run.range.start < run.range.end
    });

    Inline {
        text: text.into(),
        runs,
    }
}

/// Añade un trozo de texto al párrafo en construcción: colapsa el espacio
/// (salvo en el código), quita el espacio de principio de línea y el doble, y
/// alarga el último tramo si tiene el mismo formato.
fn push_piece(
    text: &mut String,
    runs: &mut Vec<Run>,
    piece: &str,
    marks: Marks,
    link: Option<String>,
) {
    let mut piece = if marks.code {
        piece.to_string()
    } else {
        collapse_whitespace(piece)
    };
    let at_line_start = text.is_empty() || text.ends_with('\n');
    if (at_line_start || text.ends_with(' ')) && piece.starts_with(' ') {
        piece.remove(0);
    }
    if piece.is_empty() {
        return;
    }
    let start = text.len();
    text.push_str(&piece);
    let end = text.len();
    if marks == Marks::default() && link.is_none() {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.range.end == start && last.marks == marks && last.link == link => {
            last.range.end = end;
        }
        _ => runs.push(Run {
            range: start..end,
            marks,
            link,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El texto pintado de un bloque y de todo lo que contiene.
    fn painted(blocks: &[Block]) -> String {
        let mut out = String::new();
        for block in blocks {
            match block {
                Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
                    out.push_str(&content.text);
                }
                Block::Images { images, .. } => {
                    for image in images {
                        out.push_str(&image.alt);
                        out.push(' ');
                    }
                }
                Block::List { items, .. } => {
                    for item in items {
                        out.push_str(&painted(&item.blocks));
                    }
                }
                Block::Quote(blocks) => out.push_str(&painted(blocks)),
                Block::Rule => {}
                Block::Code { text, .. } => out.push_str(text),
                Block::Table { header, rows, .. } => {
                    for cell in header.iter().chain(rows.iter().flatten()) {
                        out.push_str(&cell.text);
                        out.push(' ');
                    }
                }
                Block::Details { summary, blocks } => {
                    out.push_str(&summary.text);
                    out.push_str(&painted(blocks));
                }
            }
            out.push('\n');
        }
        out
    }

    fn paragraph(text: &str) -> Block {
        Block::Paragraph {
            align: Align::Left,
            content: Inline {
                text: text.to_string().into(),
                runs: Vec::new(),
            },
        }
    }

    /// El tramo que cubre exactamente `needle`.
    fn run<'a>(inline: &'a Inline, needle: &str) -> &'a Run {
        inline
            .runs
            .iter()
            .find(|run| &inline.text[run.range.clone()] == needle)
            .unwrap_or_else(|| panic!("no hay un tramo «{needle}» en {inline:?}"))
    }

    fn only_inline(blocks: &[Block]) -> &Inline {
        match blocks {
            [Block::Paragraph { content, .. }] | [Block::Heading { content, .. }] => content,
            other => panic!("se esperaba un párrafo: {other:?}"),
        }
    }

    const KANEO: &str = r#"<p align="center">
  <a href="https://kaneo.app">
    <img src="https://assets.kaneo.app/logo-mono-rounded.png" alt="Kaneo's logo" width="300" />
  </a>
</p>

<div align="center">

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![CI](https://github.com/usekaneo/kaneo/actions/workflows/ci.yml/badge.svg)](https://github.com/usekaneo/kaneo/actions/workflows/ci.yml)
[![Discord](https://img.shields.io/discord/1326250681530843178?color=7389D8&label&logo=discord&logoColor=ffffff)](https://discord.gg/rU4tSyhXXU)

</div>

<h1 align="center">Kaneo</h1>

<h3 align="center">
  <a href="https://kaneo.app/quick-start">Quick Start</a>
  <span> | </span>
  <a href="https://kaneo.app">Website</a>
  <span> | </span>
  <a href="https://discord.gg/rU4tSyhXXU">Discord</a>
</h3>

<!-- A comment that must not show -->

An open source project management platform focused on simplicity &amp; efficiency.<br>
Second line after a <b>br</b>.

<details>
<summary>Table of contents</summary>

- [Features](#features)
  - [Nested](#nested)
- [Getting started](#getting-started)

</details>

## Features

| Feature | Status |
|:--|:-:|
| Kanban | ✅ |
| `API` | **beta** |

1. First
2. Second
   - [x] done
   - [ ] todo

> [!NOTE]
> Run it with ~~npm~~ pnpm.

```bash
pnpm install
```

---

Visit <https://kaneo.app> or https://github.com/usekaneo/kaneo.
"#;

    #[test]
    fn a_kaneo_style_readme_paints_no_markup() {
        let blocks = render_readme(KANEO);
        let text = painted(&blocks);

        assert!(!text.contains('<'), "quedó HTML crudo: {text}");
        assert!(!text.contains("!["), "quedó una imagen cruda: {text}");
        assert!(!text.contains("&amp;"), "quedó una entidad: {text}");
        assert!(!text.contains("comment"), "se pintó un comentario: {text}");
        assert!(
            !text.contains("[!NOTE]"),
            "se pintó la marca de aviso: {text}"
        );

        // El logo: una imagen grande, centrada y con su enlace.
        assert_eq!(
            blocks[0],
            Block::Images {
                align: Align::Center,
                images: vec![Image {
                    alt: "Kaneo's logo".to_string(),
                    src: "https://assets.kaneo.app/logo-mono-rounded.png".to_string(),
                    link: Some("https://kaneo.app".to_string()),
                    badge: false,
                }],
            }
        );

        // Los tres badges en una sola fila, centrada, con sus enlaces.
        let Block::Images { align, images } = &blocks[1] else {
            panic!("se esperaba la fila de badges: {:?}", blocks[1]);
        };
        assert_eq!(*align, Align::Center);
        assert_eq!(
            images
                .iter()
                .map(|image| (image.alt.as_str(), image.badge, image.link.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("License: MIT", true, Some("#")),
                (
                    "CI",
                    true,
                    Some("https://github.com/usekaneo/kaneo/actions/workflows/ci.yml")
                ),
                ("Discord", true, Some("https://discord.gg/rU4tSyhXXU")),
            ]
        );

        // `<h1>` y `<h3>` centrados; el `<span> | </span>` queda como texto.
        assert!(matches!(
            &blocks[2],
            Block::Heading { level: 1, align: Align::Center, content } if &*content.text == "Kaneo"
        ));
        let Block::Heading {
            level: 3,
            align: Align::Center,
            content,
        } = &blocks[3]
        else {
            panic!("se esperaba el h3: {:?}", blocks[3]);
        };
        assert_eq!(&*content.text, "Quick Start | Website | Discord");
        assert_eq!(
            run(content, "Website").link.as_deref(),
            Some("https://kaneo.app")
        );
        assert_eq!(
            run(content, "Quick Start").link.as_deref(),
            Some("https://kaneo.app/quick-start")
        );

        // `<br>` es un salto; `&amp;` se decodifica; `<b>` es negrita.
        let Block::Paragraph { align, content } = &blocks[4] else {
            panic!("se esperaba el párrafo: {:?}", blocks[4]);
        };
        assert_eq!(*align, Align::Left, "el div centrado ya se cerró");
        assert_eq!(
            &*content.text,
            "An open source project management platform focused on simplicity & efficiency.\nSecond line after a br."
        );
        assert!(run(content, "br").marks.strong);

        // `<details>` con su `<summary>` y la lista anidada dentro.
        let Block::Details {
            summary,
            blocks: inner,
        } = &blocks[5]
        else {
            panic!("se esperaba details: {:?}", blocks[5]);
        };
        assert_eq!(&*summary.text, "Table of contents");
        let [Block::List { start: None, items }] = inner.as_slice() else {
            panic!("se esperaba la lista: {inner:?}");
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[0].blocks.as_slice(),
            [Block::Paragraph { .. }, Block::List { .. }]
        ));
    }

    #[test]
    fn gfm_tables_tasks_quotes_code_and_rules() {
        let blocks = render_readme(KANEO);
        let rest = &blocks[6..];

        assert!(
            matches!(&rest[0], Block::Heading { level: 2, content, .. } if &*content.text == "Features")
        );

        let Block::Table {
            alignments,
            header,
            rows,
        } = &rest[1]
        else {
            panic!("se esperaba la tabla: {:?}", rest[1]);
        };
        assert_eq!(alignments, &vec![Align::Left, Align::Center]);
        assert_eq!(
            header.iter().map(|cell| &*cell.text).collect::<Vec<_>>(),
            vec!["Feature", "Status"]
        );
        assert_eq!(rows.len(), 2);
        assert!(run(&rows[1][0], "API").marks.code);
        assert!(run(&rows[1][1], "beta").marks.strong);

        let Block::List {
            start: Some(1),
            items,
        } = &rest[2]
        else {
            panic!("se esperaba la lista ordenada: {:?}", rest[2]);
        };
        let [_, Block::List { items: tasks, .. }] = items[1].blocks.as_slice() else {
            panic!("se esperaba la sublista: {:?}", items[1]);
        };
        assert_eq!(
            tasks.iter().map(|task| task.checked).collect::<Vec<_>>(),
            vec![Some(true), Some(false)]
        );
        assert_eq!(painted(&tasks[0].blocks), "done\n");

        let Block::Quote(quote) = &rest[3] else {
            panic!("se esperaba la cita: {:?}", rest[3]);
        };
        assert!(run(only_inline(&quote[..1]), "Note").marks.strong);
        let note = only_inline(&quote[1..]);
        assert_eq!(&*note.text, "Run it with npm pnpm.");
        assert!(run(note, "npm").marks.strike);

        assert_eq!(
            rest[4],
            Block::Code {
                language: Some("bash".to_string()),
                text: "pnpm install".into(),
            }
        );
        assert_eq!(rest[5], Block::Rule);

        // Autolink y URL suelta.
        let links: Vec<_> = only_inline(&rest[6..])
            .links()
            .map(|(_, link)| link)
            .collect();
        assert_eq!(
            links,
            vec!["https://kaneo.app", "https://github.com/usekaneo/kaneo"]
        );
        assert_eq!(rest.len(), 7);
    }

    #[test]
    fn headings_have_six_levels_and_formats_nest() {
        let blocks =
            render_readme("#### Four\n###### Six\n####### Seven\n\n**use `git`** and *this*");
        assert!(matches!(&blocks[0], Block::Heading { level: 4, .. }));
        assert!(matches!(&blocks[1], Block::Heading { level: 6, .. }));
        assert_eq!(blocks[2], paragraph("####### Seven"));

        let inline = only_inline(&blocks[3..]);
        assert_eq!(&*inline.text, "use git and this");
        assert!(run(inline, "use ").marks.strong);
        let code = run(inline, "git").marks;
        assert!(code.code && code.strong);
        assert!(run(inline, "this").marks.emphasis);
    }

    #[test]
    fn loose_text_wraps_like_html_whitespace() {
        let blocks = render_readme("One   two\nthree  \nfour");
        // Dos espacios al final de línea: salto forzado.
        assert_eq!(blocks, vec![paragraph("One two three\nfour")]);
    }

    #[test]
    fn badges_inside_a_sentence_stay_inline() {
        let blocks = render_readme(
            "Built with [![Rust](https://img.shields.io/badge/rust-1.85-orange)](https://rust-lang.org) and ![a screenshot](shot.png).",
        );
        let inline = only_inline(&blocks);
        assert_eq!(&*inline.text, "Built with Rust and [a screenshot].");
        let badge = run(inline, "Rust");
        assert!(badge.marks.badge);
        assert_eq!(badge.link.as_deref(), Some("https://rust-lang.org"));
        assert!(run(inline, "[a screenshot]").marks.image);
    }

    #[test]
    fn unknown_tags_keep_their_text_and_scripts_vanish() {
        let blocks = render_readme(
            "<div><custom-tag data-x=\"1\">Kept <sup>1</sup> <kbd>Ctrl</kbd></custom-tag></div>\n<script>alert(1)</script>\n\nAfter <u>under</u> 3 < 4.",
        );
        assert_eq!(painted(&blocks), "Kept 1 Ctrl\nAfter under 3 < 4.\n");
        let Block::Paragraph { content, .. } = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert!(run(content, "Ctrl").marks.code);
    }

    #[test]
    fn html_lists_and_alignment_nest() {
        let blocks = render_readme(
            "<div align=\"right\">\n<ul>\n<li>one</li>\n<li>two <a href=\"javascript:alert(1)\">bad</a></li>\n</ul>\n</div>",
        );
        let [Block::List { start: None, items }] = blocks.as_slice() else {
            panic!("{blocks:?}");
        };
        assert_eq!(items.len(), 2);
        let Block::Paragraph { align, content } = &items[1].blocks[0] else {
            panic!("{items:?}");
        };
        assert_eq!(*align, Align::Right);
        assert_eq!(run(content, "bad").link.as_deref(), Some("#"));
    }

    #[test]
    fn unsafe_link_protocols_are_neutralised() {
        assert_eq!(
            sanitize_markdown_url(" https://example.com "),
            "https://example.com"
        );
        assert_eq!(
            sanitize_markdown_url("MAILTO:ana@example.com"),
            "MAILTO:ana@example.com"
        );
        assert_eq!(sanitize_markdown_url("./docs/guide.md"), "./docs/guide.md");
        assert_eq!(sanitize_markdown_url("#usage"), "#usage");
        assert_eq!(sanitize_markdown_url("javascript:alert(1)"), "#");
        assert_eq!(sanitize_markdown_url("file:///etc/passwd"), "#");

        let blocks = render_readme("[x](javascript:alert) <mail@example.com> [d](./docs/a.md)");
        let inline = only_inline(&blocks);
        assert_eq!(run(inline, "x").link.as_deref(), Some("#"));
        assert_eq!(
            run(inline, "mail@example.com").link.as_deref(),
            Some("mailto:mail@example.com")
        );
        assert_eq!(run(inline, "d").link.as_deref(), Some("./docs/a.md"));
    }

    #[test]
    fn entities_and_attributes_are_decoded() {
        assert_eq!(
            decode_entities("a &amp; b &#169; &#x2014; &bogus; &"),
            "a & b © — &bogus; &"
        );
        assert_eq!(
            tokenize_html("<IMG SRC='x.png' alt=\"A &quot;B&quot;\" width=20/>"),
            vec![HtmlToken::Open {
                name: "img".to_string(),
                attrs: vec![
                    ("src".to_string(), "x.png".to_string()),
                    ("alt".to_string(), "A \"B\"".to_string()),
                    ("width".to_string(), "20".to_string()),
                ],
            }]
        );
    }

    #[test]
    fn unclosed_code_blocks_and_tags_still_render() {
        assert_eq!(
            render_readme("```\nlet a = 1\n\tindented"),
            vec![Block::Code {
                language: None,
                text: "let a = 1\n    indented".into(),
            }]
        );
        let blocks = render_readme("<details><summary>Open</summary>\n\nBody");
        assert_eq!(painted(&blocks), "OpenBody\n\n");
    }
}
