//! Coloreado de sintaxis de los visores (`src/lib/highlight.js`).
//!
//! Devuelve trozos de texto con su clase, no elementos: así se puede
//! **fusionar** con el resaltado de lo que cambió dentro de la línea.
//!
//! Se colorea línea a línea porque eso es lo que un diff tiene: trozos sueltos
//! de un archivo. La contrapartida es que una construcción que abarca varias
//! líneas —un comentario de bloque, una plantilla— puede colorearse mal en las
//! líneas intermedias. Es el precio conocido de no pedir el archivo entero.
//!
//! Diferencias con el original:
//!
//! - El análisis lo hace tree-sitter (el resaltador de gpui-kit) y no
//!   highlight.js, así que los límites de algún token pueden diferir. Las
//!   capturas de tree-sitter se agrupan en las mismas seis clases de color que
//!   el CSS de `FileDiffViewer.vue` daba a las `.hljs-*`.
//! - `xml` (html, vue, svg), `scss`/`less` e `ini` (toml) se analizan con la
//!   gramática más cercana de las que trae gpui-kit. `ini`/`toml`,
//!   `dockerfile`, `kotlin`, `swift` y `csharp`, que en gpui-kit 0.7 no tienen
//!   gramática o consultas de resaltado utilizables, con un reconocedor de
//!   línea propio (comentarios, cadenas, números, palabras clave y llamadas).
//! - Los rangos son offsets en bytes UTF-8 (ver `mergeon_model::diff`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::component::Rope;
use gpui_kit::component::highlighter::SyntaxHighlighter;
use gpui_kit::component::input::HighlightStyleResolver;
use gpui_kit::{App, HighlightStyle, Hsla};
use mergeon_model::diff::{DiffLine, DiffLineType, InlineRange};
use mergeon_ui::ActivePaletteExt as _;

/// Lenguaje con el que se colorea un archivo. [`Language::name`] es el nombre
/// que le daba el original (el de highlight.js).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Bash,
    C,
    Cpp,
    CSharp,
    Css,
    Dockerfile,
    Go,
    /// html, htm, vue: `xml` en el original.
    Html,
    Ini,
    Java,
    JavaScript,
    Json,
    Kotlin,
    Less,
    Markdown,
    Php,
    Python,
    Ruby,
    Rust,
    Scss,
    Sql,
    Swift,
    /// toml: `ini` en el original.
    Toml,
    /// tsx: `typescript` en el original.
    Tsx,
    TypeScript,
    /// xml, svg.
    Xml,
    Yaml,
}

impl Language {
    /// Nombre del lenguaje en el original (`languageForPath`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Css => "css",
            Self::Dockerfile => "dockerfile",
            Self::Go => "go",
            Self::Html | Self::Xml => "xml",
            Self::Ini | Self::Toml => "ini",
            Self::Java => "java",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Kotlin => "kotlin",
            Self::Less => "less",
            Self::Markdown => "markdown",
            Self::Php => "php",
            Self::Python => "python",
            Self::Ruby => "ruby",
            Self::Rust => "rust",
            Self::Scss => "scss",
            Self::Sql => "sql",
            Self::Swift => "swift",
            Self::Tsx | Self::TypeScript => "typescript",
            Self::Yaml => "yaml",
        }
    }

    /// Lenguaje por el nombre que usaba el original; `None` si no está entre
    /// los registrados (`lowlight.registered`).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "bash" => Self::Bash,
            "c" => Self::C,
            "cpp" => Self::Cpp,
            "csharp" => Self::CSharp,
            "css" => Self::Css,
            "dockerfile" => Self::Dockerfile,
            "go" => Self::Go,
            "ini" => Self::Ini,
            "java" => Self::Java,
            "javascript" => Self::JavaScript,
            "json" => Self::Json,
            "kotlin" => Self::Kotlin,
            "less" => Self::Less,
            "markdown" => Self::Markdown,
            "php" => Self::Php,
            "python" => Self::Python,
            "ruby" => Self::Ruby,
            "rust" => Self::Rust,
            "scss" => Self::Scss,
            "sql" => Self::Sql,
            "swift" => Self::Swift,
            "typescript" => Self::TypeScript,
            "xml" => Self::Xml,
            "yaml" => Self::Yaml,
            _ => return None,
        })
    }

    /// Gramática de tree-sitter (nombre en el registro de gpui-kit) con la que
    /// se analiza; `None` para los lenguajes con reconocedor propio.
    fn grammar(self) -> Option<&'static str> {
        Some(match self {
            Self::Bash => "bash",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Css | Self::Scss | Self::Less => "css",
            Self::Go => "go",
            Self::Html | Self::Xml => "html",
            Self::Java => "java",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Php => "php",
            Self::Python => "python",
            Self::Ruby => "ruby",
            Self::Rust => "rust",
            Self::Sql => "sql",
            Self::Tsx => "tsx",
            Self::TypeScript => "typescript",
            Self::Yaml => "yaml",
            // Sin consultas de resaltado en gpui-kit 0.7 (C#, Swift), con una
            // que no compila contra su gramática (Kotlin) o más fieles con un
            // reconocedor de línea (ini, toml, Dockerfile).
            Self::CSharp
            | Self::Kotlin
            | Self::Swift
            | Self::Toml
            | Self::Dockerfile
            | Self::Ini => return None,
        })
    }
}

fn extension_language(extension: &str) -> Option<Language> {
    Some(match extension {
        "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
        "ts" | "mts" | "cts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "vue" | "html" | "htm" => Language::Html,
        "xml" | "svg" => Language::Xml,
        "css" => Language::Css,
        "scss" | "sass" => Language::Scss,
        "less" => Language::Less,
        "json" | "jsonc" => Language::Json,
        "md" | "markdown" => Language::Markdown,
        "sh" | "bash" | "zsh" | "fish" => Language::Bash,
        "py" => Language::Python,
        "java" => Language::Java,
        "go" => Language::Go,
        "rs" => Language::Rust,
        "sql" => Language::Sql,
        "yml" | "yaml" => Language::Yaml,
        "php" => Language::Php,
        "rb" => Language::Ruby,
        "c" | "h" => Language::C,
        "cpp" | "cc" | "cxx" | "hpp" => Language::Cpp,
        "cs" => Language::CSharp,
        "kt" | "kts" => Language::Kotlin,
        "swift" => Language::Swift,
        "toml" => Language::Toml,
        "ini" | "cfg" | "conf" | "env" => Language::Ini,
        _ => return None,
    })
}

fn filename_language(name: &str) -> Option<Language> {
    Some(match name {
        "dockerfile" => Language::Dockerfile,
        "makefile" | ".gitignore" => Language::Bash,
        _ => return None,
    })
}

/// De la ruta sale el lenguaje con el que colorear: por nombre de archivo
/// (`Dockerfile`, `Makefile`) o por extensión. `None` si no se conoce.
pub fn language_for_path(path: &str) -> Option<Language> {
    let name = path.rsplit('/').next().unwrap_or_default().to_lowercase();
    if let Some(language) = filename_language(&name) {
        return Some(language);
    }

    let (_, extension) = name.rsplit_once('.')?;
    extension_language(extension)
}

/// Clase de color de un trozo de código. Son los grupos del CSS de
/// `FileDiffViewer.vue`: seis colores y parada, porque en un diff el color ya
/// significa algo —alta y baja— y colorear de más compite con lo que hay que ver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `.hljs-keyword`, `built_in`, `literal`, nombre de etiqueta, `selector-tag`.
    Keyword,
    /// `.hljs-string`, `regexp`, `char`, `symbol`.
    String,
    /// `.hljs-number`, `attr`, `attribute`, `selector-attr`, `selector-pseudo`.
    Number,
    /// `.hljs-title` (funciones, clases), `section`.
    Title,
    /// `.hljs-type`, `variable`, `template-variable`, `params`.
    Variable,
    /// `.hljs-comment`, `quote`, `meta`. Va en cursiva.
    Comment,
    /// `.hljs-deletion`.
    Deletion,
    /// `.hljs-addition`.
    Addition,
}

impl TokenKind {
    /// Clase de highlight.js representativa del grupo.
    pub fn hljs_class(self) -> &'static str {
        match self {
            Self::Keyword => "hljs-keyword",
            Self::String => "hljs-string",
            Self::Number => "hljs-number",
            Self::Title => "hljs-title",
            Self::Variable => "hljs-variable",
            Self::Comment => "hljs-comment",
            Self::Deletion => "hljs-deletion",
            Self::Addition => "hljs-addition",
        }
    }

    /// Los comentarios van en cursiva.
    pub fn is_italic(self) -> bool {
        self == Self::Comment
    }
}

/// Colores de sintaxis del tema activo: la paleta de la app, no la de un editor
/// ajeno.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntaxColors {
    pub keyword: Hsla,
    pub string: Hsla,
    pub number: Hsla,
    pub title: Hsla,
    pub variable: Hsla,
    pub comment: Hsla,
    pub deletion: Hsla,
    pub addition: Hsla,
}

impl SyntaxColors {
    /// Sintaxis monocroma de Señal: palabras clave en tinta (y en 600, ver
    /// `syntax_runs`), cadenas en `text-2` y comentarios en `text-3` cursiva,
    /// para que el único color del diff sea el del cambio.
    pub fn new(cx: &App) -> Self {
        let palette = cx.palette();
        Self {
            keyword: palette.text_1.into(),
            string: palette.text_2.into(),
            number: palette.text_1.into(),
            title: palette.text_1.into(),
            variable: palette.text_1.into(),
            comment: palette.text_3.into(),
            deletion: palette.diff_del_sign.into(),
            addition: palette.diff_add_sign.into(),
        }
    }

    pub fn color(&self, kind: TokenKind) -> Hsla {
        match kind {
            TokenKind::Keyword => self.keyword,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Title => self.title,
            TokenKind::Variable => self.variable,
            TokenKind::Comment => self.comment,
            TokenKind::Deletion => self.deletion,
            TokenKind::Addition => self.addition,
        }
    }
}

/// Trozo coloreado de una línea: rango en bytes y su clase.
pub type Token = (Range<usize>, TokenKind);

/// Lo que una captura de tree-sitter significa para nosotros antes de mirar su
/// texto y su contexto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Kind(TokenKind),
    /// Sin color propio, y además apaga el de la captura que lo envuelve (la
    /// interpolación dentro de una plantilla).
    Plain,
    /// Nombre de propiedad: `attr` si se declara (`clave: valor`), sin color
    /// si solo se accede a ella.
    Property,
    /// Identificador suelto de JS/TS: una clase (`title.class_`) si va en
    /// PascalCase, nada si no.
    Identifier,
    /// Constante del lenguaje: número si empieza por dígito, literal si no.
    Literal,
}

const CAPTURES: [Capture; 12] = [
    Capture::Kind(TokenKind::Keyword),
    Capture::Kind(TokenKind::String),
    Capture::Kind(TokenKind::Number),
    Capture::Kind(TokenKind::Title),
    Capture::Kind(TokenKind::Variable),
    Capture::Kind(TokenKind::Comment),
    Capture::Kind(TokenKind::Deletion),
    Capture::Kind(TokenKind::Addition),
    Capture::Plain,
    Capture::Property,
    Capture::Identifier,
    Capture::Literal,
];

/// El resaltador de gpui-kit devuelve estilos, no nombres de captura: la clase
/// viaja codificada en el matiz del color y se recupera al leer los rangos.
fn encode(capture: Capture) -> HighlightStyle {
    let index = CAPTURES
        .iter()
        .position(|candidate| *candidate == capture)
        .unwrap_or_default();
    HighlightStyle {
        color: Some(Hsla {
            h: index as f32,
            s: 0.,
            l: 0.,
            a: 1.,
        }),
        ..Default::default()
    }
}

fn decode(style: &HighlightStyle) -> Option<Capture> {
    let color = style.color?;
    CAPTURES.get(color.h.round() as usize).copied()
}

struct CaptureResolver {
    language: Language,
}

impl HighlightStyleResolver for CaptureResolver {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        classify_capture(self.language, name).map(encode)
    }
}

fn is_script(language: Language) -> bool {
    matches!(
        language,
        Language::JavaScript | Language::TypeScript | Language::Tsx
    )
}

fn is_stylesheet(language: Language) -> bool {
    matches!(language, Language::Css | Language::Scss | Language::Less)
}

/// A qué grupo de color va una captura de tree-sitter. Las reglas siguen lo que
/// highlight.js marcaba con cada clase; lo que allí no tenía color (operadores,
/// puntuación, identificadores sueltos) aquí tampoco.
fn classify_capture(language: Language, name: &str) -> Option<Capture> {
    use Capture::{Identifier, Kind, Literal, Plain, Property};
    use TokenKind::*;

    let script = is_script(language);
    let stylesheet = is_stylesheet(language);
    let markup = matches!(language, Language::Html | Language::Xml);

    match name {
        // `true`, `null`, `undefined`, `None`; en Rust, también los números.
        "constant.builtin" => return Some(Literal),
        // `string`, `number`, `int`, `require`: `built_in` en el original.
        "boolean" | "type.builtin" | "function.builtin" => return Some(Kind(Keyword)),
        "variable.builtin" | "variable.special" | "variable.parameter" => {
            return Some(Kind(Variable));
        }
        "tag.doctype" => return Some(Kind(Comment)),
        // El nombre de una etiqueta sale como `hljs-name` suelto (el resaltado
        // se aplana a una clase por trozo), y `.hljs-tag .hljs-name` no lo
        // alcanza: en el original va sin color.
        "tag" | "tag.builtin" => return None,
        "link_text" | "text.reference" => return Some(Kind(String)),
        // El código en línea de Markdown (`.hljs-code`) no tenía color.
        "text.literal" | "text.code.span" | "text.uri" | "punctuation.list_marker" => {
            return None;
        }
        // La almohadilla de un título va con su título (`.hljs-section`).
        "punctuation.special" => {
            return (language == Language::Markdown).then_some(Kind(Title));
        }
        // Colores `#fff` de CSS: `number` en el original.
        "string.special" if stylesheet => return Some(Kind(Number)),
        // `var(`, `calc(`: `built_in`.
        "function" if stylesheet => return Some(Kind(Keyword)),
        // Atributos de Rust (`#[derive]`) y anotaciones: `meta` en el original.
        "attribute" if !markup && !stylesheet => return Some(Kind(Comment)),
        // La consulta de variables locales de la gramática se queda con los
        // identificadores antes que la de resaltado.
        "variable" | "local.reference" | "local.definition" => {
            return script.then_some(Identifier);
        }
        _ => {}
    }

    match name.split('.').next().unwrap_or(name) {
        "keyword" | "include" | "conditional" | "repeat" | "exception" | "storageclass" => {
            Some(Kind(Keyword))
        }
        "string" | "character" | "label" | "escape" => Some(Kind(String)),
        "number" | "float" | "attribute" => Some(Kind(Number)),
        "property" => Some(Property),
        "function" | "method" | "constructor" | "title" => Some(Kind(Title)),
        // Un identificador de tipo en JS/TS es una clase (`title.class_`); en
        // los lenguajes tipados, un tipo.
        "type" if script => Some(Kind(Title)),
        "type" | "enum" | "variant" | "parameter" => Some(Kind(Variable)),
        "comment" | "preproc" => Some(Kind(Comment)),
        "embedded" => Some(Plain),
        _ => None,
    }
}

/// La clase definitiva de una captura, ya con su texto delante.
fn resolve_capture(
    capture: Capture,
    code: &str,
    range: &Range<usize>,
    language: Language,
) -> Option<TokenKind> {
    let text = code.get(range.clone())?;
    let after = code.get(range.end..).unwrap_or_default().trim_start();
    let before = code.get(..range.start).unwrap_or_default().trim_end();

    match capture {
        // Las claves de JSON son cadenas para tree-sitter y `attr` en el original.
        Capture::Kind(TokenKind::String)
            if language == Language::Json && after.starts_with(':') =>
        {
            Some(TokenKind::Number)
        }
        Capture::Kind(kind) => Some(kind),
        Capture::Plain => None,
        // `{ prop: 1 }`, `prop: valor` de YAML, `color: red` de CSS; no
        // `obj.prop`, ni `.clase:hover` o `#id:hover` de un selector.
        Capture::Property => {
            (after.starts_with(':') && !before.ends_with('.') && !before.ends_with('#'))
                .then_some(TokenKind::Number)
        }
        Capture::Identifier => is_class_reference(text).then_some(TokenKind::Title),
        Capture::Literal => Some(if text.starts_with(|c: char| c.is_ascii_digit()) {
            TokenKind::Number
        } else {
            TokenKind::Keyword
        }),
    }
}

/// `Error`, `Date`, `HttpClient`: mayúscula inicial y alguna minúscula. Las
/// constantes en mayúsculas (`NODE_ENV`) no son clases.
fn is_class_reference(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_uppercase())
        && text.contains(|c: char| c.is_ascii_lowercase())
        && !text.contains('_')
}

thread_local! {
    /// Un resaltador por gramática: compilar sus consultas es lo caro, analizar
    /// una línea suelta no.
    static HIGHLIGHTERS: RefCell<HashMap<&'static str, SyntaxHighlighter>> =
        RefCell::new(HashMap::new());
}

fn tree_sitter_tokens(code: &str, language: Language, grammar: &'static str) -> Vec<Token> {
    let styles = HIGHLIGHTERS.with(|highlighters| {
        let mut highlighters = highlighters.borrow_mut();
        let highlighter = highlighters
            .entry(grammar)
            .or_insert_with(|| SyntaxHighlighter::new(grammar));
        highlighter.update(None, &Rope::from(code), None);
        highlighter.styles(&(0..code.len()), &CaptureResolver { language })
    });

    let mut tokens: Vec<Token> = Vec::new();
    for (range, style) in styles {
        if range.is_empty() || range.end > code.len() || !code.is_char_boundary(range.start) {
            continue;
        }
        let Some(kind) =
            decode(&style).and_then(|capture| resolve_capture(capture, code, &range, language))
        else {
            continue;
        };

        match tokens.last_mut() {
            Some((last, last_kind)) if *last_kind == kind && last.end == range.start => {
                last.end = range.end;
            }
            _ => tokens.push((range, kind)),
        }
    }

    tokens
}

/// Dónde empieza el comentario que cierra una línea (`# …`, `; …`), fuera de
/// comillas.
fn trailing_comment(code: &str, markers: &[char]) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (at, character) in code.char_indices() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if markers.contains(&character) && previous.is_whitespace() => return Some(at),
            None => {}
        }
        previous = character;
    }
    None
}

/// Reconocedor de línea para `ini`/`cfg`/`conf`/`env`/`toml`: comentarios,
/// secciones, claves y valores literales.
fn ini_tokens(code: &str) -> Vec<Token> {
    let comment = trailing_comment(code, &['#', ';']);
    let body = &code[..comment.unwrap_or(code.len())];
    let mut tokens = Vec::new();

    let trimmed = body.trim_start();
    let indent = body.len() - trimmed.len();
    if trimmed.starts_with('[') {
        let end = trimmed
            .rfind(']')
            .map_or(body.trim_end().len(), |at| indent + at + 1);
        tokens.push((indent..end, TokenKind::Title));
    } else if let Some(separator) = body.find('=') {
        let key = body[..separator].trim_end();
        if key.len() > indent {
            tokens.push((indent..key.len(), TokenKind::Number));
        }

        let raw_value = &body[separator + 1..];
        let value = raw_value.trim();
        let start = separator + 1 + (raw_value.len() - raw_value.trim_start().len());
        let range = start..start + value.len();
        let quoted = value.len() >= 2
            && (value.starts_with('"') && value.ends_with('"')
                || value.starts_with('\'') && value.ends_with('\''));
        if quoted {
            tokens.push((range, TokenKind::String));
        } else if !value.is_empty() && value.parse::<f64>().is_ok() {
            tokens.push((range, TokenKind::Number));
        } else if matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "on" | "off" | "yes" | "no" | "null"
        ) {
            tokens.push((range, TokenKind::Keyword));
        }
    }

    if let Some(at) = comment {
        tokens.push((at..code.len(), TokenKind::Comment));
    }
    tokens
}

const DOCKERFILE_INSTRUCTIONS: [&str; 18] = [
    "FROM",
    "RUN",
    "CMD",
    "LABEL",
    "MAINTAINER",
    "EXPOSE",
    "ENV",
    "ADD",
    "COPY",
    "ENTRYPOINT",
    "VOLUME",
    "USER",
    "WORKDIR",
    "ARG",
    "ONBUILD",
    "STOPSIGNAL",
    "HEALTHCHECK",
    "SHELL",
];

/// Reconocedor de línea para `Dockerfile`: comentarios, la instrucción que
/// abre la línea y las cadenas entre comillas.
fn dockerfile_tokens(code: &str) -> Vec<Token> {
    let trimmed = code.trim_start();
    let indent = code.len() - trimmed.len();
    if trimmed.starts_with('#') {
        return vec![(indent..code.len(), TokenKind::Comment)];
    }

    let mut tokens = Vec::new();
    let word_end = trimmed
        .find(char::is_whitespace)
        .map_or(code.len(), |at| indent + at);
    let mut cursor = indent;
    if DOCKERFILE_INSTRUCTIONS.contains(&&code[indent..word_end]) {
        tokens.push((indent..word_end, TokenKind::Keyword));
        cursor = word_end;
    }

    let mut open: Option<(usize, char)> = None;
    for (offset, character) in code[cursor..].char_indices() {
        let at = cursor + offset;
        match open {
            Some((start, quote)) if character == quote => {
                tokens.push((start..at + 1, TokenKind::String));
                open = None;
            }
            None if character == '"' || character == '\'' => open = Some((at, character)),
            _ => {}
        }
    }

    tokens
}

const KOTLIN_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "break",
    "by",
    "catch",
    "class",
    "companion",
    "const",
    "constructor",
    "continue",
    "data",
    "do",
    "else",
    "enum",
    "false",
    "final",
    "finally",
    "for",
    "fun",
    "if",
    "import",
    "in",
    "init",
    "inline",
    "interface",
    "internal",
    "is",
    "lateinit",
    "null",
    "object",
    "open",
    "override",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "sealed",
    "super",
    "suspend",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "val",
    "var",
    "when",
    "while",
];

const SWIFT_KEYWORDS: &[&str] = &[
    "as",
    "associatedtype",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "continue",
    "default",
    "defer",
    "deinit",
    "do",
    "else",
    "enum",
    "extension",
    "false",
    "fileprivate",
    "final",
    "for",
    "func",
    "guard",
    "if",
    "import",
    "in",
    "init",
    "internal",
    "is",
    "lazy",
    "let",
    "mutating",
    "nil",
    "open",
    "override",
    "private",
    "protocol",
    "public",
    "repeat",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "switch",
    "throw",
    "throws",
    "true",
    "try",
    "typealias",
    "unowned",
    "var",
    "weak",
    "where",
    "while",
];

const CSHARP_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "false",
    "finally",
    "float",
    "for",
    "foreach",
    "get",
    "if",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sealed",
    "set",
    "short",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "using",
    "var",
    "virtual",
    "void",
    "while",
];

/// Reconocedor de línea para lenguajes de la familia de C sin consultas de
/// resaltado: comentarios, cadenas, números, palabras clave y llamadas.
fn c_like_tokens(code: &str, keywords: &[&str]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut at = 0;

    while let Some(character) = code[at..].chars().next() {
        let rest = &code[at..];
        if rest.starts_with("//") {
            tokens.push((at..code.len(), TokenKind::Comment));
            break;
        }
        if rest.starts_with("/*") {
            let end = rest.find("*/").map_or(code.len(), |close| at + close + 2);
            tokens.push((at..end, TokenKind::Comment));
            at = end;
            continue;
        }
        if character == '"' || character == '\'' {
            let mut end = code.len();
            let mut escaped = false;
            for (offset, next) in rest.char_indices().skip(1) {
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == character {
                    end = at + offset + next.len_utf8();
                    break;
                }
            }
            tokens.push((at..end, TokenKind::String));
            at = end;
            continue;
        }
        if character.is_ascii_digit() {
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
                .unwrap_or(rest.len());
            tokens.push((at..at + len, TokenKind::Number));
            at += len;
            continue;
        }
        if character.is_alphabetic() || character == '_' {
            let len = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let word = &rest[..len];
            if keywords.contains(&word) {
                tokens.push((at..at + len, TokenKind::Keyword));
            } else if rest[len..].trim_start().starts_with('(') {
                tokens.push((at..at + len, TokenKind::Title));
            }
            at += len;
            continue;
        }
        at += character.len_utf8();
    }

    tokens
}

/// Los trozos con color de una línea, ordenados y sin solaparse. Lo que queda
/// entre ellos va con el color del texto.
pub fn highlight_tokens(code: &str, language: Option<Language>) -> Vec<Token> {
    let Some(language) = language else {
        return Vec::new();
    };
    if code.is_empty() {
        return Vec::new();
    }

    // Una cita de Markdown (`.hljs-quote`) va entera como un comentario.
    if language == Language::Markdown && code.trim_start().starts_with('>') {
        return vec![(0..code.len(), TokenKind::Comment)];
    }

    match language {
        Language::Ini | Language::Toml => ini_tokens(code),
        Language::Dockerfile => dockerfile_tokens(code),
        Language::Kotlin => c_like_tokens(code, KOTLIN_KEYWORDS),
        Language::Swift => c_like_tokens(code, SWIFT_KEYWORDS),
        Language::CSharp => c_like_tokens(code, CSHARP_KEYWORDS),
        // Una línea minificada (JS, JSON, CSS de una sola línea) de decenas de
        // KB congelaba la interfaz en tree-sitter: va sin color.
        _ if has_line_longer_than(code, MAX_HIGHLIGHT_LINE_BYTES) => Vec::new(),
        _ => match language.grammar() {
            Some(grammar) => tree_sitter_tokens(code, language, grammar),
            None => Vec::new(),
        },
    }
}

/// Bytes a partir de los cuales una línea no pasa por tree-sitter.
pub const MAX_HIGHLIGHT_LINE_BYTES: usize = 16_384;

/// Si alguna línea de `code` pasa de `limit` bytes.
fn has_line_longer_than(code: &str, limit: usize) -> bool {
    code.len() > limit && code.split('\n').any(|line| line.len() > limit)
}

/// Trozo de una línea con su clase de sintaxis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSegment {
    pub text: String,
    pub token: Option<TokenKind>,
}

impl SyntaxSegment {
    pub fn new(text: impl Into<String>, token: Option<TokenKind>) -> Self {
        Self {
            text: text.into(),
            token,
        }
    }
}

/// El texto de la línea partido en trozos con su clase de sintaxis.
pub fn highlight_segments(code: &str, language: Option<Language>) -> Vec<SyntaxSegment> {
    if code.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut cursor = 0;
    for (range, kind) in highlight_tokens(code, language) {
        // Pase lo que pase, la línea tiene que salir entera: el color es un extra.
        if range.start < cursor || range.end > code.len() {
            continue;
        }
        if range.start > cursor {
            segments.push(SyntaxSegment::new(&code[cursor..range.start], None));
        }
        segments.push(SyntaxSegment::new(&code[range.clone()], Some(kind)));
        cursor = range.end;
    }
    if cursor < code.len() {
        segments.push(SyntaxSegment::new(&code[cursor..], None));
    }

    segments
}

/// Lo que el visor pinta: un trozo coloreado y, si toca, marcado como cambiado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSegment {
    pub text: String,
    pub token: Option<TokenKind>,
    pub changed: bool,
}

/// Parte los trozos de sintaxis por los bordes de lo que cambió, para que un
/// mismo trozo no tenga que ser a la vez coloreado y resaltado.
pub fn apply_inline_range(
    segments: &[SyntaxSegment],
    range: Option<InlineRange>,
) -> Vec<LineSegment> {
    let Some([from, to]) = range else {
        return segments
            .iter()
            .map(|segment| LineSegment {
                text: segment.text.clone(),
                token: segment.token,
                changed: false,
            })
            .collect();
    };

    let mut pieces = Vec::new();
    let mut cursor = 0;
    for segment in segments {
        let start = cursor;
        let end = start + segment.text.len();
        cursor = end;

        let mut boundaries = vec![start];
        for boundary in [from.clamp(start, end), to.clamp(start, end), end] {
            if !boundaries.contains(&boundary) {
                boundaries.push(boundary);
            }
        }

        for pair in boundaries.windows(2) {
            let (piece_start, piece_end) = (pair[0], pair[1]);
            let Some(text) = segment.text.get(piece_start - start..piece_end - start) else {
                continue;
            };
            pieces.push(LineSegment {
                text: text.to_string(),
                token: segment.token,
                changed: piece_start >= from && piece_end <= to,
            });
        }
    }

    pieces
}

/// Una línea ya coloreada y con su cambio marcado.
pub fn build_line_segments(line: &DiffLine, language: Option<Language>) -> Vec<LineSegment> {
    let code = line.content.as_str();

    // Las cabeceras de hunk y las notas de git no son código del archivo.
    if code.is_empty() || matches!(line.kind, DiffLineType::Hunk | DiffLineType::Note) {
        return vec![LineSegment {
            text: code.to_string(),
            token: None,
            changed: false,
        }];
    }

    apply_inline_range(&highlight_segments(code, language), line.inline)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(segments: &[SyntaxSegment]) -> String {
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    fn line_text(segments: &[LineSegment]) -> String {
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    fn name_for(path: &str) -> Option<&'static str> {
        language_for_path(path).map(Language::name)
    }

    #[test]
    fn a_minified_line_skips_tree_sitter_but_keeps_its_text() {
        let line = format!(
            "var a={};",
            "[1,2,3],".repeat(MAX_HIGHLIGHT_LINE_BYTES / 8 + 1)
        );
        assert!(line.len() > MAX_HIGHLIGHT_LINE_BYTES);
        assert!(highlight_tokens(&line, Some(Language::JavaScript)).is_empty());
        assert_eq!(
            text_of(&highlight_segments(&line, Some(Language::JavaScript))),
            line
        );
        // Una línea normal sigue coloreada, y un bloque largo de líneas cortas también.
        assert!(!highlight_tokens("const a = 1;", Some(Language::JavaScript)).is_empty());
        let block = "const a = 1;\n".repeat(2_000);
        assert!(block.len() > MAX_HIGHLIGHT_LINE_BYTES);
        assert!(!highlight_tokens(&block, Some(Language::JavaScript)).is_empty());
    }

    fn diff_line(kind: DiffLineType, content: &str, inline: Option<InlineRange>) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
            old_num: None,
            new_num: None,
            inline,
        }
    }

    // --- picking the language -------------------------------------------

    #[test]
    fn reads_the_language_from_the_extension() {
        assert_eq!(name_for("src/main.js"), Some("javascript"));
        assert_eq!(name_for("electron/store.ts"), Some("typescript"));
        assert_eq!(name_for("styles/globals.css"), Some("css"));
    }

    // Un .vue es plantilla, script y estilos: xml es lo que mejor se porta.
    #[test]
    fn treats_a_vue_component_as_markup() {
        assert_eq!(name_for("src/App.vue"), Some("xml"));
    }

    #[test]
    fn knows_files_that_have_no_extension() {
        assert_eq!(name_for("Dockerfile"), Some("dockerfile"));
        assert_eq!(name_for("deploy/Makefile"), Some("bash"));
        assert_eq!(name_for(".gitignore"), Some("bash"));
    }

    #[test]
    fn says_nothing_about_what_it_does_not_know() {
        assert_eq!(name_for("assets/logo.png"), None);
        assert_eq!(name_for(""), None);
        assert_eq!(name_for("LICENSE"), None);
    }

    #[test]
    fn keeps_the_extension_table_of_the_original() {
        let table = [
            ("a.jsx", "javascript"),
            ("a.mjs", "javascript"),
            ("a.cjs", "javascript"),
            ("a.tsx", "typescript"),
            ("a.mts", "typescript"),
            ("a.cts", "typescript"),
            ("a.html", "xml"),
            ("a.htm", "xml"),
            ("a.xml", "xml"),
            ("a.svg", "xml"),
            ("a.scss", "scss"),
            ("a.sass", "scss"),
            ("a.less", "less"),
            ("a.json", "json"),
            ("a.jsonc", "json"),
            ("a.md", "markdown"),
            ("a.markdown", "markdown"),
            ("a.sh", "bash"),
            ("a.zsh", "bash"),
            ("a.fish", "bash"),
            ("a.py", "python"),
            ("a.java", "java"),
            ("a.go", "go"),
            ("a.rs", "rust"),
            ("a.sql", "sql"),
            ("a.yml", "yaml"),
            ("a.yaml", "yaml"),
            ("a.php", "php"),
            ("a.rb", "ruby"),
            ("a.c", "c"),
            ("a.h", "c"),
            ("a.cpp", "cpp"),
            ("a.cc", "cpp"),
            ("a.cxx", "cpp"),
            ("a.hpp", "cpp"),
            ("a.cs", "csharp"),
            ("a.kt", "kotlin"),
            ("a.kts", "kotlin"),
            ("a.swift", "swift"),
            ("a.ini", "ini"),
            ("a.toml", "ini"),
            ("a.cfg", "ini"),
            ("a.conf", "ini"),
            ("a.env", "ini"),
            ("SRC/MAIN.JS", "javascript"),
        ];
        for (path, expected) in table {
            assert_eq!(name_for(path), Some(expected), "{path}");
        }
    }

    // --- colouring a line -----------------------------------------------

    #[test]
    fn separates_the_parts_of_the_code() {
        let segments = highlight_segments("const total = 1", Some(Language::JavaScript));

        assert_eq!(
            segments[0],
            SyntaxSegment::new("const", Some(TokenKind::Keyword))
        );
        assert_eq!(
            segments[0].token.map(TokenKind::hljs_class),
            Some("hljs-keyword")
        );
        assert_eq!(text_of(&segments), "const total = 1");
    }

    // Pase lo que pase, la línea tiene que salir entera: el color es un extra.
    #[test]
    fn never_loses_text() {
        for code in ["const a = \"sin cerrar", "}", "   ", "ñ á é 漢字"] {
            assert_eq!(
                text_of(&highlight_segments(code, Some(Language::JavaScript))),
                code
            );
        }
    }

    #[test]
    fn leaves_it_plain_when_the_language_is_unknown() {
        assert_eq!(
            highlight_segments("cualquier cosa", None),
            vec![SyntaxSegment::new("cualquier cosa", None)]
        );
        assert_eq!(
            highlight_segments("cualquier cosa", Language::from_name("brainfuck")),
            vec![SyntaxSegment::new("cualquier cosa", None)]
        );
    }

    #[test]
    fn groups_the_tokens_like_the_original_stylesheet() {
        let kinds = |code: &str, language: Language| -> Vec<(String, TokenKind)> {
            highlight_segments(code, Some(language))
                .into_iter()
                .filter_map(|segment| segment.token.map(|token| (segment.text, token)))
                .collect()
        };
        let has = |tokens: &[(String, TokenKind)], text: &str, kind: TokenKind| {
            assert!(
                tokens
                    .iter()
                    .any(|(token_text, token)| token_text == text && *token == kind),
                "{text:?} should be {kind:?} in {tokens:?}"
            );
        };

        let js = kinds(
            "export function login(user) { return { token: \"demo\", n: 42 } } // fin",
            Language::JavaScript,
        );
        has(&js, "export", TokenKind::Keyword);
        has(&js, "function", TokenKind::Keyword);
        has(&js, "login", TokenKind::Title);
        has(&js, "return", TokenKind::Keyword);
        has(&js, "token", TokenKind::Number);
        has(&js, "\"demo\"", TokenKind::String);
        has(&js, "42", TokenKind::Number);
        has(&js, "// fin", TokenKind::Comment);

        // Acceder a una propiedad no es declararla.
        let access = kinds("const mode = process.env.NODE_ENV", Language::JavaScript);
        assert!(!access.iter().any(|(text, _)| text == "env"), "{access:?}");

        // La interpolación de una plantilla vuelve al color del texto.
        let template = highlight_segments("log(`on ${today}`)", Some(Language::JavaScript));
        let today = template
            .iter()
            .find(|segment| segment.text.contains("today"))
            .expect("the interpolation is its own piece");
        assert_eq!(today.token, None, "{template:?}");

        let css = kinds("  margin: 0;", Language::Css);
        has(&css, "margin", TokenKind::Number);

        // `new Error`, `new Date`: referencias a clases, no constantes.
        let classes = kinds("throw new Error(NODE_ENV)", Language::JavaScript);
        has(&classes, "Error", TokenKind::Title);
        assert!(
            !classes.iter().any(|(text, _)| text == "NODE_ENV"),
            "{classes:?}"
        );

        // Una propiedad abreviada no es una clave.
        let shorthand = kinds("return { user, token: 1 }", Language::JavaScript);
        assert!(
            !shorthand.iter().any(|(text, _)| text == "user"),
            "{shorthand:?}"
        );

        let json = kinds("  \"name\": \"demo\", \"ok\": true", Language::Json);
        has(&json, "\"name\"", TokenKind::Number);
        has(&json, "\"demo\"", TokenKind::String);
        has(&json, "true", TokenKind::Keyword);

        let selector = kinds(".button:hover { color: #fff; }", Language::Css);
        assert!(
            !selector.iter().any(|(text, _)| text == "button"),
            "{selector:?}"
        );
        has(&selector, "color", TokenKind::Number);
        has(&selector, "#fff", TokenKind::Number);

        let heading = kinds("# Demo project", Language::Markdown);
        has(&heading, "Demo project", TokenKind::Title);
        let quote = kinds("> quoted", Language::Markdown);
        has(&quote, "> quoted", TokenKind::Comment);

        let yaml = kinds("name: demo # c", Language::Yaml);
        has(&yaml, "name", TokenKind::Number);
        has(&yaml, "# c", TokenKind::Comment);

        let ini = kinds("name = \"demo\" # why", Language::Ini);
        has(&ini, "name", TokenKind::Number);
        has(&ini, "\"demo\"", TokenKind::String);
        has(&ini, "# why", TokenKind::Comment);
        has(
            &kinds("[package]", Language::Toml),
            "[package]",
            TokenKind::Title,
        );

        let kotlin = kinds("fun main() { val a = \"x\" } // fin", Language::Kotlin);
        has(&kotlin, "fun", TokenKind::Keyword);
        has(&kotlin, "main", TokenKind::Title);
        has(&kotlin, "\"x\"", TokenKind::String);
        has(&kotlin, "// fin", TokenKind::Comment);

        let docker = kinds("FROM node:22 # base", Language::Dockerfile);
        has(&docker, "FROM", TokenKind::Keyword);
    }

    #[test]
    fn leaves_tag_names_uncoloured_like_the_flattened_hljs_output() {
        // `highlightSegments` del original: `<` `hljs-tag`, `a` `hljs-name`
        // (sin regla), `href` `hljs-attr`, `"/docs"` `hljs-string`.
        let segments = highlight_segments("  <a href=\"/docs\">Docs</a>", Some(Language::Html));
        let token = |text: &str| {
            segments
                .iter()
                .find(|segment| segment.text == text)
                .and_then(|segment| segment.token)
        };
        assert_eq!(token("a"), None);
        assert_eq!(token("href"), Some(TokenKind::Number));
        let docs = segments
            .iter()
            .find(|segment| segment.text.contains("/docs"));
        assert_eq!(
            docs.and_then(|segment| segment.token),
            Some(TokenKind::String),
            "{segments:?}"
        );
        for (language, code) in [
            (Language::Html, "<link rel=\"x\" />"),
            (Language::Tsx, "const a = <div className=\"x\" />"),
        ] {
            let segments = highlight_segments(code, Some(language));
            assert!(
                segments
                    .iter()
                    .filter(|segment| segment.text == "link" || segment.text == "div")
                    .all(|segment| segment.token.is_none()),
                "{language:?}: {segments:?}"
            );
        }
    }

    #[test]
    fn colours_every_registered_language_without_losing_text() {
        let samples = [
            (Language::Bash, "if [ -f \"$1\" ]; then echo ok; fi # x"),
            (Language::C, "int main(void) { return 0; } // x"),
            (Language::Cpp, "std::string name = \"x\"; // y"),
            (Language::CSharp, "public class A { int b = 1; }"),
            (Language::Css, ".button { padding: 4px 12px; }"),
            (Language::Scss, "$gap: 4px; .a { &:hover { color: red; } }"),
            (Language::Less, "@gap: 4px; .a { color: @gap; }"),
            (Language::Dockerfile, "RUN npm install"),
            (Language::Go, "func main() { fmt.Println(\"x\") }"),
            (Language::Html, "<div class=\"a\">hola</div>"),
            (Language::Xml, "<svg width=\"1\"><!-- x --></svg>"),
            (Language::Ini, "[core]"),
            (Language::Toml, "name = \"demo\""),
            (Language::Java, "public static void main(String[] a) {}"),
            (Language::JavaScript, "const a = () => 1"),
            (Language::Json, "{ \"a\": [1, true, null] }"),
            (Language::Kotlin, "fun main() { val a = 1 }"),
            (Language::Markdown, "# Demo project"),
            (Language::Php, "<?php echo \"x\"; ?>"),
            (Language::Python, "def main(): return None  # x"),
            (Language::Ruby, "def main; puts 'x'; end"),
            (Language::Rust, "fn main() { let a = 1; }"),
            (Language::Sql, "SELECT a FROM b WHERE c = 1;"),
            (Language::Swift, "func main() { let a = 1 }"),
            (Language::Tsx, "const a = <div className=\"x\" />"),
            (Language::TypeScript, "const a: number = 1"),
            (Language::Yaml, "name: demo # x"),
        ];
        for (language, code) in samples {
            assert_eq!(
                text_of(&highlight_segments(code, Some(language))),
                code,
                "{language:?}"
            );
        }
    }

    // --- merging colour with the change ---------------------------------

    // Sin esto, un trozo tendría que ser a la vez coloreado y resaltado.
    #[test]
    fn cuts_the_coloured_pieces_at_the_edges_of_the_change() {
        let segments = apply_inline_range(
            &[
                SyntaxSegment::new("const ", None),
                SyntaxSegment::new("total", Some(TokenKind::Title)),
            ],
            Some([6, 11]),
        );

        assert_eq!(
            segments,
            vec![
                LineSegment {
                    text: "const ".into(),
                    token: None,
                    changed: false
                },
                LineSegment {
                    text: "total".into(),
                    token: Some(TokenKind::Title),
                    changed: true
                },
            ]
        );
    }

    #[test]
    fn splits_a_single_piece_that_is_only_partly_changed() {
        let segments = apply_inline_range(
            &[SyntaxSegment::new("abcdef", Some(TokenKind::String))],
            Some([2, 4]),
        );

        let expected: Vec<LineSegment> = [("ab", false), ("cd", true), ("ef", false)]
            .into_iter()
            .map(|(text, changed)| LineSegment {
                text: text.into(),
                token: Some(TokenKind::String),
                changed,
            })
            .collect();
        assert_eq!(segments, expected);
    }

    #[test]
    fn keeps_the_whole_text_when_nothing_changed() {
        let segments = apply_inline_range(&[SyntaxSegment::new("igual", None)], None);

        assert_eq!(
            segments,
            vec![LineSegment {
                text: "igual".into(),
                token: None,
                changed: false
            }]
        );
    }

    #[test]
    fn does_not_lose_text_when_cutting() {
        let original = [
            SyntaxSegment::new("call(", None),
            SyntaxSegment::new("alpha", Some(TokenKind::Variable)),
            SyntaxSegment::new(")", None),
        ];

        assert_eq!(
            line_text(&apply_inline_range(&original, Some([5, 10]))),
            "call(alpha)"
        );
    }

    // --- what the viewer paints -----------------------------------------

    #[test]
    fn colours_the_code_and_marks_what_changed_at_once() {
        let segments = build_line_segments(
            &diff_line(DiffLineType::Del, "const total = 1", Some([12, 15])),
            Some(Language::JavaScript),
        );

        assert_eq!(segments[0].text, "const");
        assert_eq!(segments[0].token, Some(TokenKind::Keyword));
        assert!(!segments[0].changed);
        let last = segments.last().expect("the line has pieces");
        assert_eq!(last.text, "1");
        assert!(last.changed);
        assert_eq!(line_text(&segments), "const total = 1");
    }

    // No son código del archivo: colorearlas sería inventar.
    #[test]
    fn leaves_hunk_headers_and_git_notes_alone() {
        let plain = |text: &str| {
            vec![LineSegment {
                text: text.into(),
                token: None,
                changed: false,
            }]
        };

        assert_eq!(
            build_line_segments(
                &diff_line(DiffLineType::Hunk, "@@ -1,2 +1,2 @@", None),
                Some(Language::JavaScript)
            ),
            plain("@@ -1,2 +1,2 @@")
        );
        assert_eq!(
            build_line_segments(
                &diff_line(DiffLineType::Note, "No newline at end of file", None),
                Some(Language::JavaScript)
            ),
            plain("No newline at end of file")
        );
    }

    #[test]
    fn handles_an_empty_line() {
        assert_eq!(
            build_line_segments(
                &diff_line(DiffLineType::Ctx, "", None),
                Some(Language::JavaScript)
            ),
            vec![LineSegment {
                text: String::new(),
                token: None,
                changed: false
            }]
        );
    }
}
