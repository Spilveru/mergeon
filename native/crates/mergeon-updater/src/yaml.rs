//! Parser YAML mínimo: el subconjunto que emite electron-builder en
//! `latest*.yml` (mapas y listas en bloque, escalares planos, entrecomillados
//! y en bloque `|` / `>`). No pretende cubrir YAML entero.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Yaml {
    Null,
    Scalar(String),
    List(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

impl Yaml {
    pub(crate) fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Self::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar(value) => Some(value),
            _ => None,
        }
    }
}

struct Line {
    indent: usize,
    /// Contenido sin la sangría ni los espacios finales.
    text: String,
    /// Línea original: los escalares en bloque conservan su contenido tal cual.
    raw: String,
}

impl Line {
    fn is_blank(&self) -> bool {
        self.text.is_empty()
    }

    fn is_ignorable(&self) -> bool {
        self.text.is_empty() || self.text.starts_with('#')
    }
}

pub(crate) fn parse(input: &str) -> Result<Yaml, String> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut lines = Vec::new();
    let mut seen_content = false;

    for (index, raw) in input.lines().enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed_end = raw.trim_end();

        // Marcadores de documento y directivas: solo se lee el primer documento.
        if trimmed_end == "---" || trimmed_end.starts_with("--- ") {
            if seen_content {
                break;
            }
            continue;
        }
        if trimmed_end == "..." {
            break;
        }
        if !seen_content && trimmed_end.starts_with('%') {
            continue;
        }

        let indent_text: String = raw
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let text = trimmed_end.trim_start().to_string();
        if indent_text.contains('\t') && !text.is_empty() {
            return Err(format!(
                "line {}: tabs are not valid indentation",
                index + 1
            ));
        }
        if !text.is_empty() && !text.starts_with('#') {
            seen_content = true;
        }

        lines.push(Line {
            indent: indent_text.len(),
            text,
            raw: raw.to_string(),
        });
    }

    let mut parser = Parser { lines, pos: 0 };
    let value = parser.parse_node(0)?;
    parser.skip_ignorable();
    if parser.pos < parser.lines.len() {
        return Err(format!(
            "unexpected content: {}",
            parser.lines[parser.pos].text
        ));
    }
    Ok(value)
}

struct Parser {
    lines: Vec<Line>,
    pos: usize,
}

impl Parser {
    fn skip_ignorable(&mut self) {
        while self
            .lines
            .get(self.pos)
            .is_some_and(|line| line.is_ignorable())
        {
            self.pos += 1;
        }
    }

    fn parse_node(&mut self, min_indent: usize) -> Result<Yaml, String> {
        self.skip_ignorable();
        let Some(line) = self.lines.get(self.pos) else {
            return Ok(Yaml::Null);
        };
        if line.indent < min_indent {
            return Ok(Yaml::Null);
        }

        let indent = line.indent;
        if is_sequence_entry(&line.text) {
            self.parse_sequence(indent)
        } else if split_key(&line.text)?.is_some() {
            self.parse_mapping(indent)
        } else {
            let text = line.text.clone();
            self.pos += 1;
            self.parse_value(&text, indent)
        }
    }

    fn parse_mapping(&mut self, indent: usize) -> Result<Yaml, String> {
        let mut entries: Vec<(String, Yaml)> = Vec::new();

        loop {
            self.skip_ignorable();
            let Some(line) = self.lines.get(self.pos) else {
                break;
            };
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return Err(format!("unexpected indentation: {}", line.text));
            }
            if is_sequence_entry(&line.text) {
                break;
            }

            let Some((key, rest)) = split_key(&line.text)? else {
                return Err(format!("expected `key: value`: {}", line.text));
            };
            self.pos += 1;
            let value = self.parse_value(&rest, indent)?;

            // Clave repetida: gana la última, como en js-yaml con `json: true`.
            match entries.iter_mut().find(|(name, _)| *name == key) {
                Some(entry) => entry.1 = value,
                None => entries.push((key, value)),
            }
        }

        Ok(Yaml::Map(entries))
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Yaml, String> {
        let mut items = Vec::new();

        loop {
            self.skip_ignorable();
            let Some(line) = self.lines.get(self.pos) else {
                break;
            };
            if line.indent < indent || !is_sequence_entry(&line.text) {
                if line.indent > indent {
                    return Err(format!("unexpected indentation: {}", line.text));
                }
                break;
            }
            if line.indent > indent {
                return Err(format!("unexpected indentation: {}", line.text));
            }

            let after_dash = &line.text[1..];
            let content = after_dash.trim_start();
            let offset = 1 + (after_dash.len() - content.len());

            if !content.is_empty()
                && !content.starts_with('#')
                && (is_sequence_entry(content) || split_key(content)?.is_some())
            {
                // `- clave: valor`: el elemento es un mapa (o lista) cuya sangría
                // es la columna donde empieza el contenido tras el guion.
                let content = content.to_string();
                let line = &mut self.lines[self.pos];
                line.indent = indent + offset;
                line.text = content;
                items.push(self.parse_node(indent + offset)?);
            } else {
                let content = content.to_string();
                self.pos += 1;
                items.push(self.parse_value(&content, indent)?);
            }
        }

        Ok(Yaml::List(items))
    }

    /// Valor que sigue a `clave:` o a `- `, con `parent_indent` la sangría de esa línea.
    fn parse_value(&mut self, rest: &str, parent_indent: usize) -> Result<Yaml, String> {
        let rest = rest.trim();

        if rest.is_empty() || rest.starts_with('#') {
            self.skip_ignorable();
            let Some(next) = self.lines.get(self.pos) else {
                return Ok(Yaml::Null);
            };
            if next.indent > parent_indent {
                return self.parse_node(parent_indent + 1);
            }
            // Una lista puede ir a la misma sangría que su clave.
            if next.indent == parent_indent && is_sequence_entry(&next.text) {
                return self.parse_sequence(parent_indent);
            }
            return Ok(Yaml::Null);
        }

        if rest.starts_with('|') || rest.starts_with('>') {
            return self.parse_block_scalar(rest, parent_indent);
        }

        let (value, plain) = parse_inline(rest)?;
        let Yaml::Scalar(mut text) = value else {
            return Ok(value);
        };
        if plain {
            // Un escalar plano puede seguir en líneas más sangradas.
            while let Some(next) = self.lines.get(self.pos) {
                if next.is_blank() || next.indent <= parent_indent {
                    break;
                }
                text.push(' ');
                text.push_str(strip_comment(&next.text));
                self.pos += 1;
            }
        }
        Ok(Yaml::Scalar(text))
    }

    fn parse_block_scalar(&mut self, header: &str, parent_indent: usize) -> Result<Yaml, String> {
        let folded = header.starts_with('>');
        let mut chomp = ' ';
        let mut explicit_indent = None;
        for ch in strip_comment(&header[1..]).chars() {
            match ch {
                '-' | '+' => chomp = ch,
                '1'..='9' => explicit_indent = ch.to_digit(10).map(|digit| digit as usize),
                _ => return Err(format!("invalid block scalar header: {header}")),
            }
        }

        let mut block: Vec<&Line> = Vec::new();
        while let Some(line) = self.lines.get(self.pos) {
            if !line.is_blank() && line.indent <= parent_indent {
                break;
            }
            block.push(line);
            self.pos += 1;
        }

        let block_indent = explicit_indent
            .map(|extra| parent_indent + extra)
            .or_else(|| {
                block
                    .iter()
                    .find(|line| !line.is_blank())
                    .map(|line| line.indent)
            })
            .unwrap_or(parent_indent + 1);

        let mut content: Vec<&str> = block
            .iter()
            .map(|line| {
                if line.is_blank() {
                    ""
                } else {
                    line.raw.get(block_indent..).unwrap_or("").trim_end()
                }
            })
            .collect();

        let mut trailing_blank = 0;
        while content.last().is_some_and(|line| line.is_empty()) {
            content.pop();
            trailing_blank += 1;
        }

        let mut text = String::new();
        if folded {
            let mut previous_was_text = false;
            for line in &content {
                if line.is_empty() {
                    text.push('\n');
                    previous_was_text = false;
                } else {
                    if previous_was_text {
                        text.push(' ');
                    }
                    text.push_str(line);
                    previous_was_text = true;
                }
            }
        } else {
            text = content.join("\n");
        }

        match chomp {
            '-' => {}
            '+' => text.push_str(&"\n".repeat(trailing_blank + 1)),
            _ => {
                if !text.is_empty() {
                    text.push('\n');
                }
            }
        }

        Ok(Yaml::Scalar(text))
    }
}

fn is_sequence_entry(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// Quita un comentario final (` # ...`) de un escalar plano.
fn strip_comment(text: &str) -> &str {
    if text.starts_with('#') {
        return "";
    }
    match text.find(" #") {
        Some(index) => text[..index].trim_end(),
        None => text.trim_end(),
    }
}

/// Separa `clave: resto`. `None` si la línea no es una entrada de mapa.
fn split_key(text: &str) -> Result<Option<(String, String)>, String> {
    if text.starts_with('"') || text.starts_with('\'') {
        let (key, consumed) = parse_quoted(text)?;
        let after = text[consumed..].trim_start();
        return Ok(match after.strip_prefix(':') {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') => {
                Some((key, rest.to_string()))
            }
            _ => None,
        });
    }

    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b':' {
            continue;
        }
        // `:` solo separa si le sigue un espacio o el fin de línea
        // (así `https://...` no se confunde con una clave).
        let next = bytes.get(index + 1);
        if next.is_none() || next == Some(&b' ') || next == Some(&b'\t') {
            let key = text[..index].trim_end();
            if key.is_empty() {
                return Ok(None);
            }
            return Ok(Some((key.to_string(), text[index + 1..].to_string())));
        }
    }
    Ok(None)
}

/// Escalar (o colección de flujo vacía/simple) en una sola línea.
/// El booleano indica si era un escalar plano.
fn parse_inline(text: &str) -> Result<(Yaml, bool), String> {
    if text.starts_with('"') || text.starts_with('\'') {
        let (value, consumed) = parse_quoted(text)?;
        let after = text[consumed..].trim();
        if !after.is_empty() && !after.starts_with('#') {
            return Err(format!("unexpected content after quoted scalar: {after}"));
        }
        return Ok((Yaml::Scalar(value), false));
    }

    if text.starts_with('[') {
        let body = strip_comment(text);
        let Some(inner) = body
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        else {
            return Err(format!("unterminated flow sequence: {text}"));
        };
        if inner.trim().is_empty() {
            return Ok((Yaml::List(Vec::new()), false));
        }
        let items = inner
            .split(',')
            .map(|item| parse_inline(item.trim()).map(|(value, _)| value))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((Yaml::List(items), false));
    }

    if text.starts_with('{') {
        if strip_comment(text).replace(' ', "") == "{}" {
            return Ok((Yaml::Map(Vec::new()), false));
        }
        return Err(format!("flow mappings are not supported: {text}"));
    }

    let plain = strip_comment(text);
    if plain.is_empty() || matches!(plain, "~" | "null" | "Null" | "NULL") {
        return Ok((Yaml::Null, true));
    }
    Ok((Yaml::Scalar(plain.to_string()), true))
}

/// Lee un escalar entrecomillado al inicio de `text`. Devuelve el valor y
/// cuántos bytes ocupaba, comillas incluidas.
fn parse_quoted(text: &str) -> Result<(String, usize), String> {
    let mut chars = text.char_indices();
    let Some((_, quote)) = chars.next() else {
        return Err("empty quoted scalar".to_string());
    };
    let mut value = String::new();

    if quote == '\'' {
        let mut chars = chars.peekable();
        while let Some((index, ch)) = chars.next() {
            if ch != '\'' {
                value.push(ch);
                continue;
            }
            // `''` dentro de comillas simples es una comilla literal.
            if chars.peek().is_some_and(|(_, next)| *next == '\'') {
                chars.next();
                value.push('\'');
                continue;
            }
            return Ok((value, index + 1));
        }
        return Err(format!("unterminated quoted scalar: {text}"));
    }

    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => return Ok((value, index + 1)),
            '\\' => {
                let Some((_, escaped)) = chars.next() else {
                    break;
                };
                match escaped {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    '0' => value.push('\0'),
                    'x' | 'u' | 'U' => {
                        let digits = match escaped {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let hex: String = chars.by_ref().take(digits).map(|(_, ch)| ch).collect();
                        let decoded = u32::from_str_radix(&hex, 16)
                            .ok()
                            .filter(|_| hex.len() == digits)
                            .and_then(char::from_u32)
                            .ok_or_else(|| format!("invalid escape in quoted scalar: {text}"))?;
                        value.push(decoded);
                    }
                    other => value.push(other),
                }
            }
            other => value.push(other),
        }
    }
    Err(format!("unterminated quoted scalar: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(text: &str) -> Yaml {
        Yaml::Scalar(text.to_string())
    }

    fn map(entries: Vec<(&str, Yaml)>) -> Yaml {
        Yaml::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    #[test]
    fn parses_scalars_of_every_style() {
        let parsed = parse(
            "plain: Mergeon 1.0\nsingle: 'it''s here'\ndouble: \"tab\\there \\u00e9\"\nempty:\nnull_word: null\nurl: https://example.com/a:b\n",
        )
        .unwrap();

        assert_eq!(
            parsed,
            map(vec![
                ("plain", scalar("Mergeon 1.0")),
                ("single", scalar("it's here")),
                ("double", scalar("tab\there é")),
                ("empty", Yaml::Null),
                ("null_word", Yaml::Null),
                ("url", scalar("https://example.com/a:b")),
            ])
        );
    }

    #[test]
    fn parses_lists_of_maps_indented_or_flush_with_the_key() {
        let indented =
            "files:\n  - url: a.exe\n    size: 10\n  - url: b.exe\n    size: 20\npath: a.exe\n";
        let flush = "files:\n- url: a.exe\n  size: 10\n- url: b.exe\n  size: 20\npath: a.exe\n";
        let expected = map(vec![
            (
                "files",
                Yaml::List(vec![
                    map(vec![("url", scalar("a.exe")), ("size", scalar("10"))]),
                    map(vec![("url", scalar("b.exe")), ("size", scalar("20"))]),
                ]),
            ),
            ("path", scalar("a.exe")),
        ]);

        assert_eq!(parse(indented).unwrap(), expected);
        assert_eq!(parse(flush).unwrap(), expected);
    }

    #[test]
    fn parses_block_scalars_comments_and_document_markers() {
        let text = "# cabecera\n---\nversion: 1.2.3 # comentario\nreleaseNotes: |\n  Line one\n\n  # no es comentario\n    indented\nfolded: >-\n  one\n  two\n\n  three\nkeep: |+\n  text\n\nafter: yes\n...\nignored: true\n";
        let parsed = parse(text).unwrap();

        assert_eq!(parsed.get("version"), Some(&scalar("1.2.3")));
        assert_eq!(
            parsed.get("releaseNotes"),
            Some(&scalar("Line one\n\n# no es comentario\n  indented\n"))
        );
        assert_eq!(parsed.get("folded"), Some(&scalar("one two\nthree")));
        assert_eq!(parsed.get("keep"), Some(&scalar("text\n\n")));
        assert_eq!(parsed.get("after"), Some(&scalar("yes")));
        assert_eq!(parsed.get("ignored"), None);
    }

    #[test]
    fn parses_scalar_lists_nested_maps_and_flow_collections() {
        let text = "tags:\n  - one\n  - 'two'\n  -\n    nested: deep\nempty_list: []\nempty_map: {}\ninline: [a, 'b', 3]\nouter:\n  inner:\n    leaf: 1\n";
        let parsed = parse(text).unwrap();

        assert_eq!(
            parsed.get("tags"),
            Some(&Yaml::List(vec![
                scalar("one"),
                scalar("two"),
                map(vec![("nested", scalar("deep"))]),
            ]))
        );
        assert_eq!(parsed.get("empty_list"), Some(&Yaml::List(Vec::new())));
        assert_eq!(parsed.get("empty_map"), Some(&Yaml::Map(Vec::new())));
        assert_eq!(
            parsed.get("inline"),
            Some(&Yaml::List(vec![scalar("a"), scalar("b"), scalar("3")]))
        );
        assert_eq!(
            parsed.get("outer").and_then(|outer| outer.get("inner")),
            Some(&map(vec![("leaf", scalar("1"))]))
        );
    }

    #[test]
    fn folds_multi_line_plain_scalars_and_handles_crlf() {
        let parsed = parse("sha512: abc\r\n  def\r\nnext: 1\r\n").unwrap();
        assert_eq!(parsed.get("sha512"), Some(&scalar("abc def")));
        assert_eq!(parsed.get("next"), Some(&scalar("1")));
    }

    #[test]
    fn reports_malformed_documents() {
        assert!(parse("key: 'unterminated\n").is_err());
        assert!(parse("key: value\n   stray: indentation\nother\n").is_err());
        assert!(parse("a: 1\n\tb: 2\n").is_err());
        assert!(parse("a: {x: 1}\n").is_err());
        assert!(parse("a: 1\nnot a mapping entry\n").is_err());
        // Un documento vacío es nulo, no un error: lo rechaza quien lo interpreta.
        assert_eq!(parse("").unwrap(), Yaml::Null);
        assert_eq!(parse("# solo comentarios\n").unwrap(), Yaml::Null);
    }
}
