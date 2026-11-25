//! Lógica pura del sidebar (`<script setup>` de `Sidebar.vue`): lo que no
//! necesita ventana ni modelo y se puede probar con `#[test]`.

use mergeon_model::repository_read_model::{BranchItem, RemoteItem};

/// Búsqueda de commits ya separada (`parseCommitSearchQuery`), con los
/// filtros de la sintaxis rápida (`path:`, `branch:`, `after:`, `before:`,
/// `code:`, `regex:`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitSearchQuery {
    pub text: String,
    pub author: String,
    pub hash: String,
    pub path: String,
    pub branch: String,
    /// Fecha para `--since` (ya traducida si era relativa: `7d` → `7 days ago`).
    pub after: String,
    pub before: String,
    pub code: String,
    /// `code` vino de `regex:` (se busca con `-G`).
    pub code_regex: bool,
}

impl CommitSearchQuery {
    /// `!parsed.text && !parsed.author && !parsed.hash` y sin filtros.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.author.is_empty()
            && self.hash.is_empty()
            && !self.has_filters()
    }

    /// Hay algún filtro de ruta, rama, fecha o contenido.
    pub fn has_filters(&self) -> bool {
        !(self.path.is_empty()
            && self.branch.is_empty()
            && self.after.is_empty()
            && self.before.is_empty()
            && self.code.is_empty())
    }
}

/// Las palabras clave de la sintaxis rápida, para el tooltip del cuadro.
pub const SEARCH_SYNTAX_HELP: &str = "Search the message by default. Combine with:\n\
author:name   commits by an author\n\
path:src/app   commits that touch a file or folder\n\
branch:main   only that branch's history\n\
after:2024-01-31 / before:… (or 7d, 2w, 3m, 1y)\n\
code:text   commits that add or remove that text\n\
regex:pattern   changed lines that match a regex\n\
Quote values with spaces: path:\"my docs\". A lone 7–40 digit hex is a commit hash.";

/// Claves de la sintaxis rápida.
const SEARCH_KEYS: [&str; 7] = [
    "author", "path", "branch", "after", "before", "code", "regex",
];

/// `/^[0-9a-f]{7,40}$/i`.
fn looks_like_hash(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Un trozo de la query: `clave:valor` (con la clave en minúsculas) o texto.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Filter { key: &'static str, value: String },
    Text(String),
}

/// Parte la query en palabras respetando las comillas de un valor
/// (`path:"my docs"`). Una comilla sin cerrar llega hasta el final.
fn tokenize(raw_query: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = raw_query.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }
        let mut word = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if c == '"' {
                quoted = !quoted;
                chars.next();
                continue;
            }
            if c.is_whitespace() && !quoted {
                break;
            }
            word.push(c);
            chars.next();
        }
        let filter = word.split_once(':').and_then(|(key, value)| {
            let key = SEARCH_KEYS
                .iter()
                .find(|known| known.eq_ignore_ascii_case(key))?;
            (!value.is_empty()).then(|| Token::Filter {
                key,
                value: value.to_string(),
            })
        });
        tokens.push(filter.unwrap_or(Token::Text(word)));
    }
    tokens
}

/// `7d`, `2w`, `3m`, `1y` → `7 days ago`...; lo demás, tal cual (git
/// entiende `2024-01-31`, `yesterday`...).
pub fn relative_date(value: &str) -> String {
    let value = value.trim();
    let unit = value.chars().last().map(|c| c.to_ascii_lowercase());
    let amount = &value[..value.len().saturating_sub(1)];
    let word = match unit {
        Some('d') => "days",
        Some('w') => "weeks",
        Some('m') => "months",
        Some('y') => "years",
        _ => return value.to_string(),
    };
    if amount.is_empty() || !amount.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_string();
    }
    format!("{amount} {word} ago")
}

/// La query se separa en la interfaz: `author:foo` filtra por autor, las
/// demás claves son filtros, un término que parece hash (7-40 hex, sin nada
/// más) intenta resolver ese commit y el resto busca en el mensaje. El
/// backend recibe las opciones ya separadas.
pub fn parse_commit_search_query(raw_query: &str) -> CommitSearchQuery {
    let mut author_terms = Vec::new();
    let mut text_terms = Vec::new();
    let mut query = CommitSearchQuery::default();

    for token in tokenize(raw_query) {
        match token {
            Token::Text(text) => text_terms.push(text),
            Token::Filter { key, value } => match key {
                "author" => author_terms.push(value),
                "path" => query.path = value,
                "branch" => query.branch = value,
                "after" => query.after = relative_date(&value),
                "before" => query.before = relative_date(&value),
                "code" => {
                    query.code = value;
                    query.code_regex = false;
                }
                _ => {
                    query.code = value;
                    query.code_regex = true;
                }
            },
        }
    }

    let text = text_terms.join(" ");
    query.author = author_terms.join(" ");
    if query.author.is_empty() && !query.has_filters() && looks_like_hash(&text) {
        query.hash = text;
    } else {
        query.text = text;
    }
    query
}

/// La query sin los filtros de esas claves (al quitar un chip).
pub fn remove_search_filters(raw_query: &str, keys: &[&str]) -> String {
    tokenize(raw_query)
        .into_iter()
        .filter_map(|token| match token {
            Token::Filter { key, .. } if keys.contains(&key) => None,
            Token::Filter { key, value } if value.contains(char::is_whitespace) => {
                Some(format!("{key}:\"{value}\""))
            }
            Token::Filter { key, value } => Some(format!("{key}:{value}")),
            Token::Text(text) => Some(text),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Atajos de fecha de "More filters".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DatePreset {
    #[default]
    AnyTime,
    Last7Days,
    Last30Days,
    /// Las fechas de los campos After / Before.
    Custom,
}

impl DatePreset {
    pub fn key(self) -> &'static str {
        match self {
            Self::AnyTime => "any",
            Self::Last7Days => "7d",
            Self::Last30Days => "30d",
            Self::Custom => "custom",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "7d" => Self::Last7Days,
            "30d" => Self::Last30Days,
            "custom" => Self::Custom,
            _ => Self::AnyTime,
        }
    }
}

/// Los campos de "More filters" (lo que no se escribió en el cuadro).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilterFields {
    pub path: String,
    pub branch: String,
    pub date: DatePreset,
    pub after: String,
    pub before: String,
    pub code: String,
    pub code_regex: bool,
}

/// Un filtro activo, como chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterKind {
    Path,
    Branch,
    Date,
    Code,
}

impl SearchFilterKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Branch => "branch",
            Self::Date => "date",
            Self::Code => "code",
        }
    }

    /// Las claves de la sintaxis rápida que lo escriben.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Path => &["path"],
            Self::Branch => &["branch"],
            Self::Date => &["after", "before"],
            Self::Code => &["code", "regex"],
        }
    }
}

/// Junta lo escrito en el cuadro con los campos de "More filters". Si un
/// filtro está en los dos sitios gana el cuadro (lo último que se escribe).
pub fn merge_search_filters(
    mut query: CommitSearchQuery,
    fields: &SearchFilterFields,
) -> CommitSearchQuery {
    let pick = |typed: &mut String, field: &str| {
        if typed.is_empty() {
            *typed = field.trim().to_string();
        }
    };
    pick(&mut query.path, &fields.path);
    pick(&mut query.branch, &fields.branch);
    if query.code.is_empty() && !fields.code.trim().is_empty() {
        query.code = fields.code.trim().to_string();
        query.code_regex = fields.code_regex;
    }
    // Cada extremo por separado: `before:` escrito no borra el "después
    // de" de los campos, ni `after:` su "antes de".
    let (field_after, field_before) = match fields.date {
        DatePreset::AnyTime => (String::new(), String::new()),
        DatePreset::Last7Days => ("7 days ago".to_string(), String::new()),
        DatePreset::Last30Days => ("30 days ago".to_string(), String::new()),
        DatePreset::Custom => (relative_date(&fields.after), relative_date(&fields.before)),
    };
    if query.after.is_empty() {
        query.after = field_after;
    }
    if query.before.is_empty() {
        query.before = field_before;
    }
    // Con filtros, el texto con forma de hash es texto.
    if query.has_filters() && !query.hash.is_empty() {
        query.text = std::mem::take(&mut query.hash);
    }
    query
}

/// Los chips de los filtros activos: tipo y texto.
pub fn search_filter_chips(
    query: &CommitSearchQuery,
    fields: &SearchFilterFields,
) -> Vec<(SearchFilterKind, String)> {
    let mut chips = Vec::new();
    if !query.branch.is_empty() {
        chips.push((SearchFilterKind::Branch, query.branch.clone()));
    }
    if !query.path.is_empty() {
        chips.push((SearchFilterKind::Path, query.path.clone()));
    }
    if !query.after.is_empty() || !query.before.is_empty() {
        let label = match (fields.date, query.after.as_str(), query.before.as_str()) {
            (DatePreset::Last7Days, "7 days ago", "") => "Last 7 days".to_string(),
            (DatePreset::Last30Days, "30 days ago", "") => "Last 30 days".to_string(),
            (_, after, "") => format!("After {after}"),
            (_, "", before) => format!("Before {before}"),
            (_, after, before) => format!("{after} \u{2013} {before}"),
        };
        chips.push((SearchFilterKind::Date, label));
    }
    if !query.code.is_empty() {
        let label = if query.code_regex {
            format!("/{}/", query.code)
        } else {
            query.code.clone()
        };
        chips.push((SearchFilterKind::Code, label));
    }
    chips
}

/// Un upstream partido en remoto y rama (`parseUpstreamReference`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamReference {
    pub remote_name: String,
    pub branch_name: String,
    pub full_name: String,
}

/// Parte `remoto/rama` con los remotos conocidos, que pueden llevar `/`
/// (`team/fork/feature` con el remoto `team/fork`): gana el más largo que
/// encaje. Sin ninguno que encaje, el primer `/`.
pub fn split_remote_reference<'a>(
    reference: &'a str,
    remotes: &[RemoteItem],
) -> Option<(&'a str, &'a str)> {
    remotes
        .iter()
        .map(|remote| remote.name.as_str())
        .filter(|name| {
            !name.is_empty()
                && reference.len() > name.len() + 1
                && reference.starts_with(name)
                && reference.as_bytes()[name.len()] == b'/'
        })
        .max_by_key(|name| name.len())
        .map(|name| (&reference[..name.len()], &reference[name.len() + 1..]))
        .or_else(|| reference.split_once('/'))
}

/// `parseUpstreamReference`: `origin/feature/login` → (`origin`, `feature/login`).
/// Con `remotes` reconoce remotos cuyo nombre lleva `/`.
pub fn parse_upstream_reference(
    upstream: Option<&str>,
    remotes: &[RemoteItem],
) -> Option<UpstreamReference> {
    let normalized = upstream.unwrap_or_default().trim();
    let (remote_name, branch_name) = split_remote_reference(normalized, remotes)?;
    Some(UpstreamReference {
        remote_name: remote_name.to_string(),
        branch_name: branch_name.to_string(),
        full_name: normalized.to_string(),
    })
}

/// `getBranchTrackingReference`: el upstream configurado o, si la rama tiene
/// copia remota con su nombre, `origin/<rama>`.
pub fn branch_tracking_reference(branch: &BranchItem) -> Option<String> {
    branch
        .upstream
        .clone()
        .filter(|upstream| !upstream.is_empty())
        .or_else(|| {
            branch
                .has_tracking
                .then(|| format!("origin/{}", branch.name))
        })
}

/// `normalizeRemoteUrl`: sin espacios ni barra final.
pub fn normalize_remote_url(url: &str) -> String {
    let trimmed = url.trim();
    trimmed.strip_suffix('/').unwrap_or(trimmed).to_string()
}

fn strip_git_suffix(text: &str) -> &str {
    let lower = text.to_ascii_lowercase();
    if lower.ends_with(".git") {
        &text[..text.len() - 4]
    } else {
        text
    }
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|start| start.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
}

/// `normalizeGitHubRemoteUrl`: la URL web del repositorio de GitHub de un
/// remoto https o ssh; `None` si no es de GitHub.
pub fn normalize_github_remote_url(remote_url: &str) -> Option<String> {
    let value = remote_url.trim();
    if value.is_empty() {
        return None;
    }

    // /^https:\/\/github\.com\/([^/]+)\/([^/]+?)(?:\.git)?$/i
    if let Some(rest) = strip_prefix_ignore_case(value, "https://github.com/") {
        let (owner, repository) = rest.split_once('/')?;
        if owner.is_empty() || repository.contains('/') {
            return None;
        }
        if repository.is_empty() {
            return None;
        }
        // `([^/]+?)` necesita al menos un carácter: `.git` a secas es el nombre.
        let stripped = strip_git_suffix(repository);
        let repository = if stripped.is_empty() {
            repository
        } else {
            stripped
        };
        return Some(format!("https://github.com/{owner}/{repository}"));
    }

    // /^git@github\.com:([^/]+)\/(.+?)(?:\.git)?$/i
    if let Some(rest) = strip_prefix_ignore_case(value, "git@github.com:") {
        let (owner, repository) = rest.split_once('/')?;
        if owner.is_empty() || repository.is_empty() {
            return None;
        }
        let stripped = strip_git_suffix(repository);
        let repository = if stripped.is_empty() {
            repository
        } else {
            stripped
        };
        return Some(format!("https://github.com/{owner}/{repository}"));
    }

    None
}

/// `encodeURIComponent`.
pub fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// `buildPullRequestUrl`: la página de GitHub que compara la rama actual con
/// la remota, lista para abrir un pull request.
pub fn build_pull_request_url(
    remote: &RemoteItem,
    branch_name: &str,
    current_branch: &str,
) -> Option<String> {
    let remote_url = if remote.refs.push.is_empty() {
        &remote.refs.fetch
    } else {
        &remote.refs.push
    };
    let normalized = normalize_github_remote_url(remote_url)?;
    if current_branch.is_empty() {
        return None;
    }

    Some(format!(
        "{normalized}/compare/{}...{}?expand=1",
        encode_uri_component(current_branch),
        encode_uri_component(branch_name)
    ))
}

/// `detachedHeadLabel`: la fila propia de HEAD desanclado en Branches.
pub fn detached_head_label(
    current_branch: Option<&str>,
    detached_head_ref: Option<&str>,
) -> String {
    if current_branch != Some("HEAD") {
        return String::new();
    }

    match detached_head_ref.filter(|reference| !reference.is_empty()) {
        Some(reference) => format!("Detached at {reference}"),
        None => "Detached at HEAD".to_string(),
    }
}

/// Una rama de la lista de Branches con su marca de destacada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRow {
    pub branch: BranchItem,
    pub is_starred: bool,
}

/// `filteredBranches`: las ramas locales cuyo nombre contiene el filtro, sin
/// distinguir mayúsculas.
pub fn filtered_branches(
    branches: &[BranchItem],
    starred_branches: &[String],
    filter_text: &str,
) -> Vec<BranchRow> {
    let query = filter_text.to_lowercase();
    branches
        .iter()
        .filter(|branch| query.is_empty() || branch.name.to_lowercase().contains(&query))
        .map(|branch| BranchRow {
            branch: branch.clone(),
            is_starred: starred_branches.contains(&branch.name),
        })
        .collect()
}

/// `localBranchName` de `CheckoutBranchDialog.vue`: `origin/feature/login` →
/// `feature/login`. Con `remotes` reconoce remotos cuyo nombre lleva `/`.
pub fn local_branch_name(remote_branch: &str, remotes: &[RemoteItem]) -> String {
    split_remote_reference(remote_branch, remotes)
        .map(|(_, branch)| branch.to_string())
        .unwrap_or_default()
}

/// El remoto de las acciones de tags: `origin` si existe; si no, el primero.
/// `None` sin remotos.
pub fn tag_remote(remotes: &[RemoteItem]) -> Option<&str> {
    remotes
        .iter()
        .find(|remote| remote.name == "origin")
        .or_else(|| remotes.first())
        .map(|remote| remote.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_model::repository_read_model::RemoteRefs;

    fn branch(name: &str) -> BranchItem {
        BranchItem {
            name: name.to_string(),
            active: false,
            is_remote: false,
            upstream: None,
            has_tracking: false,
            upstream_set: false,
            ahead: 0,
            behind: 0,
        }
    }

    #[test]
    fn splits_a_search_into_text_author_and_hash() {
        assert_eq!(
            parse_commit_search_query("  dashboard  "),
            CommitSearchQuery {
                text: "dashboard".into(),
                ..Default::default()
            }
        );
        assert_eq!(
            parse_commit_search_query("fix login author:Ana AUTHOR:Torres"),
            CommitSearchQuery {
                text: "fix login".into(),
                author: "Ana Torres".into(),
                ..Default::default()
            }
        );
        // Un término que parece hash busca ese commit y no el mensaje.
        assert_eq!(
            parse_commit_search_query("D6CF210"),
            CommitSearchQuery {
                hash: "D6CF210".into(),
                ..Default::default()
            }
        );
        // Con autor, el texto hexadecimal sigue siendo texto.
        assert_eq!(
            parse_commit_search_query("abcdef1 author:ana"),
            CommitSearchQuery {
                text: "abcdef1".into(),
                author: "ana".into(),
                ..Default::default()
            }
        );
        // `author:` a secas es texto, y seis caracteres no llegan a hash.
        assert_eq!(parse_commit_search_query("author:").text, "author:");
        assert_eq!(parse_commit_search_query("abcdef").hash, "");
        assert!(parse_commit_search_query("   ").is_empty());
    }

    #[test]
    fn parses_the_quick_filter_syntax() {
        let query = parse_commit_search_query(
            r#"fix Path:"src/my app" branch:main after:7d before:2024-06-30 code:parse_header login"#,
        );
        assert_eq!(
            query,
            CommitSearchQuery {
                text: "fix login".into(),
                path: "src/my app".into(),
                branch: "main".into(),
                after: "7 days ago".into(),
                before: "2024-06-30".into(),
                code: "parse_header".into(),
                ..Default::default()
            }
        );
        let regex = parse_commit_search_query("regex:fn\\s+main");
        assert_eq!(
            (regex.code.as_str(), regex.code_regex),
            ("fn\\s+main", true)
        );
        // Una clave desconocida o sin valor es texto.
        assert_eq!(
            parse_commit_search_query("foo:bar path:").text,
            "foo:bar path:"
        );
        // Con un filtro, un hex suelto es texto, no un hash.
        let with_filter = parse_commit_search_query("abcdef1 path:src");
        assert_eq!(
            (with_filter.text.as_str(), with_filter.hash.as_str()),
            ("abcdef1", "")
        );
        assert!(!parse_commit_search_query("path:src").is_empty());
    }

    #[test]
    fn translates_relative_dates() {
        assert_eq!(relative_date("7d"), "7 days ago");
        assert_eq!(relative_date("2W"), "2 weeks ago");
        assert_eq!(relative_date("3m"), "3 months ago");
        assert_eq!(relative_date("1y"), "1 years ago");
        assert_eq!(relative_date("2024-01-31"), "2024-01-31");
        assert_eq!(relative_date("yesterday"), "yesterday");
        assert_eq!(relative_date("d"), "d");
        assert_eq!(relative_date(""), "");
    }

    #[test]
    fn removes_the_filters_of_a_chip_from_the_query() {
        assert_eq!(
            remove_search_filters(
                r#"fix path:"my docs" after:7d before:1y author:ana"#,
                &["after", "before"]
            ),
            r#"fix path:"my docs" author:ana"#
        );
        assert_eq!(
            remove_search_filters("code:x regex:y z", &["code", "regex"]),
            "z"
        );
    }

    #[test]
    fn merges_the_fields_with_the_box_and_labels_the_chips() {
        let fields = SearchFilterFields {
            path: " src ".into(),
            branch: "dev".into(),
            date: DatePreset::Last7Days,
            code: "todo".into(),
            code_regex: true,
            ..Default::default()
        };
        let merged =
            merge_search_filters(parse_commit_search_query("branch:main deadbeef"), &fields);
        assert_eq!(merged.branch, "main", "the box wins");
        assert_eq!(merged.path, "src");
        assert_eq!(merged.after, "7 days ago");
        assert_eq!((merged.code.as_str(), merged.code_regex), ("todo", true));
        assert_eq!(
            (merged.text.as_str(), merged.hash.as_str()),
            ("deadbeef", "")
        );
        assert_eq!(
            search_filter_chips(&merged, &fields),
            [
                (SearchFilterKind::Branch, "main".to_string()),
                (SearchFilterKind::Path, "src".to_string()),
                (SearchFilterKind::Date, "Last 7 days".to_string()),
                (SearchFilterKind::Code, "/todo/".to_string()),
            ]
        );

        let custom = SearchFilterFields {
            date: DatePreset::Custom,
            after: "2024-01-01".into(),
            before: "2024-02-01".into(),
            ..Default::default()
        };
        let merged = merge_search_filters(CommitSearchQuery::default(), &custom);
        assert_eq!(
            (merged.after.as_str(), merged.before.as_str()),
            ("2024-01-01", "2024-02-01")
        );
        assert_eq!(
            search_filter_chips(&merged, &custom),
            [(
                SearchFilterKind::Date,
                "2024-01-01 \u{2013} 2024-02-01".to_string()
            )]
        );
        // `before:` escrito solo sustituye el "antes de" de los campos.
        let typed_before = merge_search_filters(parse_commit_search_query("before:3m"), &custom);
        assert_eq!(
            (typed_before.after.as_str(), typed_before.before.as_str()),
            ("2024-01-01", "3 months ago")
        );
        let typed_after = merge_search_filters(parse_commit_search_query("after:1w"), &custom);
        assert_eq!(typed_after.before, "2024-02-01");
        assert_ne!(typed_after.after, "2024-01-01");
        let before_only = merge_search_filters(
            parse_commit_search_query("before:3m"),
            &SearchFilterFields::default(),
        );
        assert_eq!(
            search_filter_chips(&before_only, &custom),
            [(SearchFilterKind::Date, "Before 3 months ago".to_string())]
        );
        assert_eq!(
            DatePreset::from_key(DatePreset::Last30Days.key()),
            DatePreset::Last30Days
        );
    }

    #[test]
    fn parses_upstream_references() {
        assert_eq!(parse_upstream_reference(None, &[]), None);
        assert_eq!(parse_upstream_reference(Some("main"), &[]), None);
        assert_eq!(
            parse_upstream_reference(Some(" origin/feature/login "), &[]),
            Some(UpstreamReference {
                remote_name: "origin".into(),
                branch_name: "feature/login".into(),
                full_name: "origin/feature/login".into(),
            })
        );
    }

    fn remote_named(name: &str) -> RemoteItem {
        RemoteItem {
            name: name.to_string(),
            refs: RemoteRefs::default(),
            branches: Vec::new(),
        }
    }

    #[test]
    fn upstream_references_respect_remotes_with_slashes() {
        let remotes = [remote_named("origin"), remote_named("team/fork")];
        assert_eq!(
            parse_upstream_reference(Some("team/fork/feature/login"), &remotes),
            Some(UpstreamReference {
                remote_name: "team/fork".into(),
                branch_name: "feature/login".into(),
                full_name: "team/fork/feature/login".into(),
            })
        );
        // Un remoto que no encaja cae al primer `/`.
        assert_eq!(
            parse_upstream_reference(Some("other/x"), &remotes).map(|parts| parts.remote_name),
            Some("other".to_string())
        );
    }

    #[test]
    fn tag_remote_prefers_origin_then_the_first_remote() {
        assert_eq!(tag_remote(&[]), None);
        assert_eq!(
            tag_remote(&[remote_named("fork"), remote_named("origin")]),
            Some("origin")
        );
        assert_eq!(
            tag_remote(&[remote_named("fork"), remote_named("upstream")]),
            Some("fork")
        );
    }

    #[test]
    fn tracking_reference_prefers_the_configured_upstream() {
        let mut item = branch("feature");
        assert_eq!(branch_tracking_reference(&item), None);

        item.has_tracking = true;
        assert_eq!(
            branch_tracking_reference(&item).as_deref(),
            Some("origin/feature")
        );

        item.upstream = Some("upstream/feature".into());
        assert_eq!(
            branch_tracking_reference(&item).as_deref(),
            Some("upstream/feature")
        );
    }

    #[test]
    fn normalizes_remote_urls() {
        assert_eq!(normalize_remote_url("  "), "");
        assert_eq!(
            normalize_remote_url(" https://github.com/demo/repo.git/ "),
            "https://github.com/demo/repo.git"
        );
    }

    #[test]
    fn builds_pull_request_urls_only_for_github_remotes() {
        let remote = |push: &str, fetch: &str| RemoteItem {
            name: "origin".into(),
            refs: RemoteRefs {
                fetch: fetch.into(),
                push: push.into(),
            },
            branches: Vec::new(),
        };

        assert_eq!(
            build_pull_request_url(
                &remote("https://github.com/demo/repo.git", ""),
                "feature/login",
                "main"
            )
            .as_deref(),
            Some("https://github.com/demo/repo/compare/main...feature%2Flogin?expand=1")
        );
        assert_eq!(
            build_pull_request_url(&remote("", "git@github.com:demo/repo.git"), "fix", "main")
                .as_deref(),
            Some("https://github.com/demo/repo/compare/main...fix?expand=1")
        );
        assert_eq!(
            build_pull_request_url(&remote("/tmp/origin.git", ""), "fix", "main"),
            None
        );
        // Sin rama actual no hay con qué comparar.
        assert_eq!(
            build_pull_request_url(&remote("https://github.com/demo/repo", ""), "fix", ""),
            None
        );
        assert_eq!(
            normalize_github_remote_url("https://github.com/demo/repo/extra"),
            None
        );
    }

    #[test]
    fn labels_a_detached_head() {
        assert_eq!(detached_head_label(Some("main"), Some("74d9526")), "");
        assert_eq!(
            detached_head_label(Some("HEAD"), Some("74d9526")),
            "Detached at 74d9526"
        );
        assert_eq!(detached_head_label(Some("HEAD"), None), "Detached at HEAD");
    }

    #[test]
    fn filters_branches_ignoring_case_and_marks_the_starred_ones() {
        let branches = [branch("main"), branch("Feature/Login"), branch("hotfix")];
        let starred = ["main".to_string()];

        let all = filtered_branches(&branches, &starred, "");
        assert_eq!(all.len(), 3);
        assert!(all[0].is_starred);
        assert!(!all[1].is_starred);

        let filtered = filtered_branches(&branches, &starred, "LOGIN");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].branch.name, "Feature/Login");
    }

    #[test]
    fn local_name_of_a_remote_branch_drops_the_remote() {
        assert_eq!(
            local_branch_name("origin/feature/login", &[]),
            "feature/login"
        );
        assert_eq!(local_branch_name("origin", &[]), "");
        let remotes = [remote_named("team/fork"), remote_named("team")];
        assert_eq!(
            local_branch_name("team/fork/feature/login", &remotes),
            "feature/login"
        );
        assert_eq!(local_branch_name("team/feature", &remotes), "feature");
    }
}
