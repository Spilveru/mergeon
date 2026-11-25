//! El árbol del repositorio tal como estaba en un commit
//! (`electron/services/commit-tree.js` y sus dos canales).
//!
//! Se lee por niveles (`git ls-tree` sin `-r`), no entero: un repositorio grande
//! no cabe de una vez, y por niveles cada expansión cuesta solo lo que tenga esa
//! carpeta. Con `-z` la salida va terminada en NUL y git no escapa los nombres,
//! así que las rutas con acentos o espacios llegan tal cual.
//!
//! Formato de cada entrada, con `--long`:
//!   100644 blob <sha>   28687\tsrc/App.js
//!   040000 tree <sha>       -\tsrc/components

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

use crate::collate::locale_compare;
use crate::error::{QueryResult, read};
use crate::exec;
use crate::paths::normalize_repository_file_path;
use crate::text::js_trim;

static ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^([0-9]{6}) (blob|tree|commit) ([0-9a-f]+)\s+(\S+)\t(.*)$").unwrap()
});

pub const TREE_ENTRY_LIMIT: usize = 2000;

/// Un archivo más grande que esto no se lee para mirarlo: se anuncia su tamaño.
pub const MAX_VIEWABLE_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeEntryType {
    Blob,
    Tree,
    /// Un submódulo es un commit dentro del árbol; se enseña, no se abre.
    Commit,
}

impl TreeEntryType {
    pub fn as_str(self) -> &'static str {
        match self {
            TreeEntryType::Blob => "blob",
            TreeEntryType::Tree => "tree",
            TreeEntryType::Commit => "commit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: TreeEntryType,
    pub mode: String,
    pub hash: String,
    /// Bytes del archivo; `None` en carpetas y submódulos.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntries {
    pub entries: Vec<TreeEntry>,
    /// Entradas que no entraron por pasar del límite.
    pub omitted: usize,
}

/// `parseTreeEntries`: carpetas primero, después por nombre.
pub fn parse_tree_entries(output: &str, limit: usize) -> TreeEntries {
    let mut entries = Vec::new();
    let mut omitted = 0;

    for row in output.split('\0').filter(|row| !row.is_empty()) {
        let Some(captures) = ENTRY.captures(row) else {
            continue;
        };

        if entries.len() >= limit {
            omitted += 1;
            continue;
        }

        let path = &captures[5];
        let raw_size = &captures[4];

        entries.push(TreeEntry {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            kind: match &captures[2] {
                "tree" => TreeEntryType::Tree,
                "commit" => TreeEntryType::Commit,
                _ => TreeEntryType::Blob,
            },
            mode: captures[1].to_string(),
            hash: captures[3].to_string(),
            size: if raw_size == "-" {
                None
            } else {
                raw_size.parse().ok()
            },
        });
    }

    // Carpetas primero y después por nombre. El comparador del original no era
    // un orden total cuando se mezclaban archivos y submódulos (decía que cada
    // uno iba detrás del otro); aquí ambos se ordenan juntos por nombre.
    entries.sort_by(|left, right| {
        let left_is_tree = left.kind == TreeEntryType::Tree;
        let right_is_tree = right.kind == TreeEntryType::Tree;
        right_is_tree
            .cmp(&left_is_tree)
            .then_with(|| locale_compare(&left.name, &right.name))
    });

    TreeEntries { entries, omitted }
}

const BINARY_PROBE_LENGTH: usize = 8000;

/*
 * Decidir si lo que devolvió git es texto. Un NUL en los primeros kilobytes es
 * la misma señal que usa el propio git, y sirve igual sobre los bytes que sobre
 * la cadena ya decodificada, porque el NUL sobrevive a la decodificación.
 */
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE_LENGTH)].contains(&0)
}

/// [`looks_binary`] sobre texto ya decodificado (mira las primeras 8000
/// unidades UTF-16, como el `slice` del original).
pub fn looks_binary_text(text: &str) -> bool {
    let mut units = 0;
    for character in text.chars() {
        if units >= BINARY_PROBE_LENGTH {
            break;
        }
        if character == '\0' {
            return true;
        }
        units += character.len_utf16();
    }
    false
}

/*
 * Un nivel del árbol del repositorio en ese commit, no el árbol entero: en un
 * repositorio grande no cabe de una vez, y por niveles cada expansión cuesta
 * solo lo que tenga esa carpeta.
 */
/// Canal `git:getTreeEntries`. `directory` vacío es la raíz.
pub fn get_tree_entries(
    repo_path: &Path,
    commit_hash: &str,
    directory: &str,
) -> QueryResult<TreeEntries> {
    read("Failed to read the repository tree", || {
        let git = exec::open(repo_path)?;
        let mut args = vec![
            "ls-tree".to_string(),
            "-z".to_string(),
            "--long".to_string(),
            commit_hash.to_string(),
        ];

        if !directory.is_empty() {
            let normalized_directory = normalize_repository_file_path(repo_path, directory)?;
            args.push("--".to_string());
            args.push(format!("{normalized_directory}/"));
        }

        let output = exec::raw(&git, &args)?;
        Ok(parse_tree_entries(&output, TREE_ENTRY_LIMIT))
    })
}

/// Resultado de `git:getFileAtCommit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAtCommit {
    pub path: String,
    pub size: u64,
    /// Vacío si el archivo es binario o demasiado grande.
    pub content: String,
    pub binary: bool,
    pub too_large: bool,
}

/// Canal `git:getFileAtCommit`: el contenido de un archivo tal como estaba en
/// ese commit.
pub fn get_file_at_commit(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
) -> QueryResult<FileAtCommit> {
    read("Failed to read the file at that commit", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;
        let reference = format!("{commit_hash}:{normalized_path}");

        // Preguntar el tamaño antes que el contenido evita traerse un archivo
        // enorme a memoria solo para descubrir que no se va a poder enseñar.
        let size = js_trim(&exec::raw(&git, &["cat-file", "-s", &reference])?)
            .parse::<u64>()
            .unwrap_or(0);

        if size > MAX_VIEWABLE_FILE_BYTES {
            return Ok(FileAtCommit {
                path: normalized_path,
                size,
                content: String::new(),
                binary: false,
                too_large: true,
            });
        }

        let content = exec::raw(&git, &["show", &reference])?;
        let binary = looks_binary_text(&content);

        Ok(FileAtCommit {
            path: normalized_path,
            size,
            content: if binary { String::new() } else { content },
            binary,
            too_large: false,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    // Salida real de `git ls-tree -z --long`, con NUL entre entradas.
    fn entry(mode: &str, kind: &str, hash: &str, size: &str, path: &str) -> String {
        format!("{mode} {kind} {hash}   {size}\t{path}")
    }

    fn output(rows: &[String]) -> String {
        format!("{}\0", rows.join("\0"))
    }

    fn parse(rows: &[String]) -> TreeEntries {
        parse_tree_entries(&output(rows), TREE_ENTRY_LIMIT)
    }

    fn names(listing: &TreeEntries) -> Vec<&str> {
        listing
            .entries
            .iter()
            .map(|item| item.name.as_str())
            .collect()
    }

    #[test]
    fn reads_a_file_with_its_size() {
        let listing = parse(&[entry(
            "100644",
            "blob",
            "994ee0625c15e5bd37629dbb19ec60127fcfb930",
            "28687",
            "src/App.test.js",
        )]);

        assert_eq!(
            listing.entries[0],
            TreeEntry {
                name: "App.test.js".into(),
                path: "src/App.test.js".into(),
                kind: TreeEntryType::Blob,
                mode: "100644".into(),
                hash: "994ee0625c15e5bd37629dbb19ec60127fcfb930".into(),
                size: Some(28687),
            }
        );
    }

    // Los directorios traen un guion donde iría el tamaño.
    #[test]
    fn reads_a_directory_as_having_no_size() {
        let listing = parse(&[entry(
            "040000",
            "tree",
            "0be7983c50061acaf2f084b2a9f060aa8552c448",
            "-",
            "src/components",
        )]);

        let item = &listing.entries[0];
        assert_eq!(item.name, "components");
        assert_eq!(item.kind, TreeEntryType::Tree);
        assert_eq!(item.size, None);
    }

    #[test]
    fn puts_folders_first_then_names_in_order() {
        let listing = parse(&[
            entry("100644", "blob", "aaa", "10", "src/zeta.js"),
            entry("100644", "blob", "bbb", "10", "src/alpha.js"),
            entry("040000", "tree", "ccc", "-", "src/utils"),
        ]);

        assert_eq!(names(&listing), ["utils", "alpha.js", "zeta.js"]);
    }

    // Con -z git no escapa nada, así que llegan tal cual.
    #[test]
    fn keeps_names_with_spaces_and_accents() {
        let listing = parse(&[entry(
            "100644",
            "blob",
            "aaa",
            "12",
            "docs/año de diseño.md",
        )]);

        assert_eq!(listing.entries[0].name, "año de diseño.md");
    }

    #[test]
    fn shows_a_submodule_instead_of_pretending_it_is_a_folder() {
        let listing = parse(&[entry("160000", "commit", "ddd", "-", "vendor/libreria")]);

        let item = &listing.entries[0];
        assert_eq!(item.kind, TreeEntryType::Commit);
        assert_eq!(item.size, None);
    }

    // Truncar en silencio haría creer que la carpeta está entera.
    #[test]
    fn says_how_many_it_left_out_when_there_are_too_many() {
        let rows: Vec<String> = (0..12)
            .map(|index| {
                entry(
                    "100644",
                    "blob",
                    "aaa",
                    "1",
                    &format!("src/file-{index}.js"),
                )
            })
            .collect();

        let listing = parse_tree_entries(&output(&rows), 10);

        assert_eq!(listing.entries.len(), 10);
        assert_eq!(listing.omitted, 2);
    }

    #[test]
    fn has_nothing_to_show_for_an_empty_directory() {
        assert_eq!(
            parse_tree_entries("", TREE_ENTRY_LIMIT),
            TreeEntries::default()
        );
    }

    #[test]
    fn skips_a_line_it_cannot_read_instead_of_inventing_an_entry() {
        let listing = parse(&[
            "basura".to_string(),
            entry("100644", "blob", "aaa", "5", "ok.txt"),
        ]);

        assert_eq!(names(&listing), ["ok.txt"]);
    }

    // La misma señal que usa git: un NUL en los primeros kilobytes.
    #[test]
    fn calls_a_file_with_a_nul_byte_binary() {
        assert!(looks_binary(&[0x89, 0x50, 0x00, 0x4e]));
    }

    #[test]
    fn leaves_normal_text_alone() {
        assert!(!looks_binary("const total = 1\nconst año = 2\n".as_bytes()));
    }

    // Lo que llega de `git show` ya viene decodificado como texto.
    #[test]
    fn works_the_same_on_the_decoded_string() {
        assert!(looks_binary_text("PNG\u{0}\u{1}"));
        assert!(!looks_binary_text("const total = 1"));
    }

    #[test]
    fn only_looks_at_the_beginning_not_the_whole_file() {
        let mut big = vec![0x61u8; 8000];
        big.push(0x00);

        assert!(!looks_binary(&big));
        assert!(!looks_binary_text(&String::from_utf8(big).unwrap()));
    }

    #[test]
    fn serializes_entries_with_the_wire_names() {
        let listing = parse(&[entry("160000", "commit", "ddd", "-", "vendor/libreria")]);
        let json = serde_json::to_value(&listing).unwrap();
        assert_eq!(json["entries"][0]["type"], "commit");
        assert_eq!(json["entries"][0]["size"], serde_json::Value::Null);
        assert_eq!(json["omitted"], 0);
        assert_eq!(TreeEntryType::Tree.as_str(), "tree");
    }

    fn repo_with_tree() -> (TestRepo, String) {
        let repo = TestRepo::new();
        repo.write("README.md", "# Demo\n");
        repo.write("src/app.js", "const total = 1\n");
        repo.write("src/lib/año de diseño.md", "texto\n");
        repo.write("src/logo.bin", [0x89u8, 0x50, 0x00, 0x4e]);
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "-m", "tree"]);
        let head = repo.head();
        (repo, head)
    }

    #[test]
    fn reads_one_level_of_the_tree_at_a_commit() {
        let (repo, head) = repo_with_tree();

        let root = get_tree_entries(&repo.path, &head, "").unwrap();
        assert_eq!(names(&root), ["src", "README.md"]);
        assert_eq!(root.entries[0].kind, TreeEntryType::Tree);
        assert_eq!(root.entries[1].size, Some(7));
        assert_eq!(root.omitted, 0);

        let src = get_tree_entries(&repo.path, &head, "src").unwrap();
        assert_eq!(names(&src), ["lib", "app.js", "logo.bin"]);
        assert_eq!(src.entries[1].path, "src/app.js");

        let nested = get_tree_entries(&repo.path, &head, "src\\lib").unwrap();
        assert_eq!(nested.entries[0].path, "src/lib/año de diseño.md");
    }

    #[test]
    fn the_tree_of_an_unknown_commit_fails_with_the_read_prefix() {
        let (repo, _) = repo_with_tree();

        let error = get_tree_entries(&repo.path, "0000000", "").unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to read the repository tree: fatal:"),
            "{error}"
        );

        let error = get_tree_entries(&repo.path, "HEAD", "../outside").unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to read the repository tree: The file path must stay inside the repository."
        );
    }

    #[test]
    fn reads_a_file_as_it_was_at_a_commit() {
        let (repo, first) = repo_with_tree();
        repo.commit_file("src/app.js", "const total = 2\n", "change");

        let old = get_file_at_commit(&repo.path, &first, "src/app.js").unwrap();
        assert_eq!(
            old,
            FileAtCommit {
                path: "src/app.js".into(),
                size: 16,
                content: "const total = 1\n".into(),
                binary: false,
                too_large: false,
            }
        );

        let new = get_file_at_commit(&repo.path, "HEAD", "./src/app.js").unwrap();
        assert_eq!(new.content, "const total = 2\n");
    }

    #[test]
    fn a_binary_file_is_announced_instead_of_shown() {
        let (repo, head) = repo_with_tree();

        let file = get_file_at_commit(&repo.path, &head, "src/logo.bin").unwrap();

        assert!(file.binary);
        assert!(!file.too_large);
        assert_eq!(file.content, "");
        assert_eq!(file.size, 4);
    }

    #[test]
    fn a_file_above_the_limit_is_not_read() {
        let repo = TestRepo::new();
        let size = MAX_VIEWABLE_FILE_BYTES as usize + 10;
        repo.commit_file("big.txt", &"x".repeat(size), "big");

        let file = get_file_at_commit(&repo.path, "HEAD", "big.txt").unwrap();

        assert!(file.too_large);
        assert!(!file.binary);
        assert_eq!(file.content, "");
        assert_eq!(file.size, size as u64);

        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(json["tooLarge"], true);
    }

    #[test]
    fn a_missing_file_fails_with_the_read_prefix() {
        let (repo, head) = repo_with_tree();

        let error = get_file_at_commit(&repo.path, &head, "nope.txt").unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to read the file at that commit: fatal:"),
            "{error}"
        );
    }
}
