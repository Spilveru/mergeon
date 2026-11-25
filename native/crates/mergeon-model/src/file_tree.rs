//! Un listado de rutas convertido en filas de árbol, plegables
//! (`src/lib/fileTree.js`).
//!
//! Estaba escrito dos veces, palabra por palabra: en la lista de cambios locales
//! y en la del detalle de un commit. Dos copias idénticas de setenta líneas y sin
//! un solo test entre las dos, que es como un arreglo acaba aplicándose en un
//! lado y no en el otro.
//!
//! Solo devuelve las filas visibles: una carpeta plegada no aporta sus hijos.
//!
//! Diferencias con el original: las filas de carpeta no arrastran `children`
//! (el original los dejaba colgando del objeto aunque la vista no los usaba) y
//! el orden por nombre aproxima `localeCompare` (ver `collate`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::collate::locale_compare;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileTreeRowType {
    Folder,
    File,
}

impl FileTreeRowType {
    pub fn as_str(self) -> &'static str {
        match self {
            FileTreeRowType::Folder => "folder",
            FileTreeRowType::File => "file",
        }
    }
}

/// Fila visible del árbol. `T` es el archivo tal como lo trae la lista de
/// origen (estado, altas, bajas...), para que la fila pueda pintarlo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTreeRow<T> {
    #[serde(rename = "type")]
    pub kind: FileTreeRowType,
    /// Clave estable: `folder:<ruta>` o `file:<ruta>`.
    pub key: String,
    pub name: String,
    pub path: String,
    pub depth: usize,
    /// Solo en las filas de archivo.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file: Option<T>,
}

impl<T> FileTreeRow<T> {
    pub fn is_folder(&self) -> bool {
        self.kind == FileTreeRowType::Folder
    }
}

struct Node {
    kind: FileTreeRowType,
    name: String,
    path: String,
    children: Vec<usize>,
    /// Posición en la lista de entrada (solo archivos).
    file: Option<usize>,
}

/// Cómo se arman las filas (ver [`build_file_tree_rows_with`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FileTreeOptions {
    /// Una cadena de carpetas con un solo hijo cada una (`src/a/b/`) va en
    /// una sola fila, como en los editores. La fila lleva la ruta de la más
    /// honda: plegarla pliega esa.
    pub compact_folders: bool,
    /// Lista plana: sin carpetas, un archivo por fila ordenado por su ruta.
    pub flat: bool,
}

/// Construye las filas visibles a partir de una lista de archivos.
///
/// `path_of` da la ruta (con `/`) de cada archivo; los que no tienen ruta se
/// ignoran. `is_expanded` dice si una carpeta, por su ruta, está desplegada.
pub fn build_file_tree_rows<T, P, E>(files: &[T], path_of: P, is_expanded: E) -> Vec<FileTreeRow<T>>
where
    T: Clone,
    P: Fn(&T) -> &str,
    E: Fn(&str) -> bool,
{
    build_file_tree_rows_with(files, path_of, is_expanded, FileTreeOptions::default())
}

/// La carpeta de una ruta (`src/lib` de `src/lib/a.js`); vacía en la raíz.
pub fn parent_folder(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..at])
}

/// Lista plana: un archivo por fila, sin carpetas, por ruta (las de la raíz
/// primero no: el orden es el de la ruta entera, como `git status`).
fn build_flat_rows<T, P>(files: &[T], path_of: P) -> Vec<FileTreeRow<T>>
where
    T: Clone,
    P: Fn(&T) -> &str,
{
    let mut indexed: Vec<(usize, &str)> = files
        .iter()
        .enumerate()
        .map(|(index, file)| (index, path_of(file)))
        .filter(|(_, path)| !path.is_empty())
        .collect();
    indexed.sort_by(|(_, left), (_, right)| {
        locale_compare(parent_folder(left), parent_folder(right))
            .then_with(|| locale_compare(left, right))
    });
    indexed
        .into_iter()
        .map(|(index, path)| FileTreeRow {
            kind: FileTreeRowType::File,
            key: format!("file:{path}"),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            depth: 0,
            file: Some(files[index].clone()),
        })
        .collect()
}

/// [`build_file_tree_rows`] con opciones: carpetas compactas o lista plana.
pub fn build_file_tree_rows_with<T, P, E>(
    files: &[T],
    path_of: P,
    is_expanded: E,
    options: FileTreeOptions,
) -> Vec<FileTreeRow<T>>
where
    T: Clone,
    P: Fn(&T) -> &str,
    E: Fn(&str) -> bool,
{
    if options.flat {
        return build_flat_rows(files, path_of);
    }
    // Árbol en arena: el nodo 0 es la raíz y no se pinta.
    let mut nodes = vec![Node {
        kind: FileTreeRowType::Folder,
        name: String::new(),
        path: String::new(),
        children: Vec::new(),
        file: None,
    }];
    // (carpeta padre, ruta) → nodo. Equivale al `find` lineal del original.
    let mut folders: HashMap<(usize, String), usize> = HashMap::new();

    for (file_index, file) in files.iter().enumerate() {
        let file_path = path_of(file);
        if file_path.is_empty() {
            continue;
        }

        let segments: Vec<&str> = file_path.split('/').collect();
        // `split` siempre da al menos un segmento.
        let Some((file_name, folder_segments)) = segments.split_last() else {
            continue;
        };
        let mut parent = 0;
        let mut path = String::new();

        for segment in folder_segments {
            path = if path.is_empty() {
                (*segment).to_string()
            } else {
                format!("{path}/{segment}")
            };

            parent = match folders.get(&(parent, path.clone())) {
                Some(existing) => *existing,
                None => {
                    let folder = nodes.len();
                    nodes.push(Node {
                        kind: FileTreeRowType::Folder,
                        name: (*segment).to_string(),
                        path: path.clone(),
                        children: Vec::new(),
                        file: None,
                    });
                    nodes[parent].children.push(folder);
                    folders.insert((parent, path.clone()), folder);
                    folder
                }
            };
        }

        let node = nodes.len();
        nodes.push(Node {
            kind: FileTreeRowType::File,
            name: (*file_name).to_string(),
            path: file_path.to_string(),
            children: Vec::new(),
            file: Some(file_index),
        });
        nodes[parent].children.push(node);
    }

    // Carpetas primero y luego por nombre: es como se lee un árbol de archivos.
    for index in 0..nodes.len() {
        let mut children = std::mem::take(&mut nodes[index].children);
        children.sort_by(|left, right| {
            let (left, right) = (&nodes[*left], &nodes[*right]);
            (left.kind != FileTreeRowType::Folder)
                .cmp(&(right.kind != FileTreeRowType::Folder))
                .then_with(|| locale_compare(&left.name, &right.name))
        });
        nodes[index].children = children;
    }

    let mut rows = Vec::new();
    // Recorrido en profundidad sin recursión: un árbol muy hondo no agota la pila.
    let mut pending: Vec<(usize, usize)> = nodes[0]
        .children
        .iter()
        .rev()
        .map(|node| (*node, 0))
        .collect();

    while let Some((index, depth)) = pending.pop() {
        let node = &nodes[index];
        match node.kind {
            FileTreeRowType::Folder => {
                // Compactas: mientras la carpeta tenga como único hijo otra
                // carpeta, las dos van en la misma fila.
                let mut folder = node;
                let mut name = node.name.clone();
                if options.compact_folders {
                    while let [only] = folder.children.as_slice()
                        && nodes[*only].kind == FileTreeRowType::Folder
                    {
                        folder = &nodes[*only];
                        name = format!("{name}/{}", folder.name);
                    }
                }
                rows.push(FileTreeRow {
                    kind: FileTreeRowType::Folder,
                    key: format!("folder:{}", folder.path),
                    name,
                    path: folder.path.clone(),
                    depth,
                    file: None,
                });
                if is_expanded(&folder.path) {
                    pending.extend(
                        folder
                            .children
                            .iter()
                            .rev()
                            .map(|child| (*child, depth + 1)),
                    );
                }
            }
            FileTreeRowType::File => rows.push(FileTreeRow {
                kind: FileTreeRowType::File,
                key: format!("file:{}", node.path),
                name: node.name.clone(),
                path: node.path.clone(),
                depth,
                file: node.file.map(|file| files[file].clone()),
            }),
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
    struct TestFile {
        path: String,
        status: String,
        additions: u32,
    }

    fn files(paths: &[&str]) -> Vec<TestFile> {
        paths
            .iter()
            .map(|path| TestFile {
                path: (*path).to_string(),
                status: "M".to_string(),
                additions: 0,
            })
            .collect()
    }

    fn rows(paths: &[&str]) -> Vec<FileTreeRow<TestFile>> {
        build_file_tree_rows(&files(paths), |file| &file.path, |_| true)
    }

    fn rows_folding(paths: &[&str], folded: &str) -> Vec<FileTreeRow<TestFile>> {
        build_file_tree_rows(&files(paths), |file| &file.path, |path| path != folded)
    }

    fn shape(rows: &[FileTreeRow<TestFile>]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{}{}", "  ".repeat(row.depth), row.name))
            .collect()
    }

    #[test]
    fn turns_paths_into_a_tree() {
        let rows = rows(&["src/lib/diff.js", "src/App.vue", "package.json"]);

        assert_eq!(
            shape(&rows),
            ["src", "  lib", "    diff.js", "  App.vue", "package.json"]
        );
    }

    #[test]
    fn puts_folders_first_then_names_in_order() {
        let rows = rows(&["zeta.js", "alpha.js", "utils/helper.js"]);

        assert_eq!(
            shape(&rows),
            ["utils", "  helper.js", "alpha.js", "zeta.js"]
        );
    }

    // Plegar es dejar de devolver los hijos, no esconderlos en el DOM.
    #[test]
    fn leaves_out_what_is_inside_a_folded_folder() {
        let rows = rows_folding(&["src/lib/diff.js", "package.json"], "src");

        assert_eq!(shape(&rows), ["src", "package.json"]);
    }

    #[test]
    fn folds_only_the_folder_that_was_folded() {
        let rows = rows_folding(&["src/lib/diff.js", "src/App.vue"], "src/lib");

        assert_eq!(shape(&rows), ["src", "  lib", "  App.vue"]);
    }

    #[test]
    fn carries_the_file_along_so_the_row_can_show_its_state() {
        let file = TestFile {
            path: "a.js".to_string(),
            status: "A".to_string(),
            additions: 3,
        };
        let rows = build_file_tree_rows(std::slice::from_ref(&file), |file| &file.path, |_| true);

        assert_eq!(rows[0].file, Some(file));
    }

    #[test]
    fn gives_every_row_a_stable_key() {
        let rows = rows(&["src/a.js", "src/b.js"]);

        let keys: HashSet<&str> = rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys.len(), rows.len());
        assert_eq!(rows[0].key, "folder:src");
        assert_eq!(rows[1].key, "file:src/a.js");
    }

    // Dos archivos con el mismo nombre en carpetas distintas no son el mismo.
    #[test]
    fn does_not_merge_same_named_files_from_different_folders() {
        let rows = rows(&["a/index.js", "b/index.js"]);

        let paths: Vec<&str> = rows
            .iter()
            .filter(|row| row.kind == FileTreeRowType::File)
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(paths, ["a/index.js", "b/index.js"]);
    }

    #[test]
    fn handles_a_file_at_the_root() {
        let rows = rows(&["README.md"]);

        assert_eq!(rows[0].kind, FileTreeRowType::File);
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[0].name, "README.md");
    }

    #[test]
    fn ignores_entries_without_a_path_instead_of_breaking_the_list() {
        let rows = rows(&["a.js", ""]);

        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn has_nothing_to_show_for_nothing() {
        assert_eq!(rows(&[]), []);
    }

    #[test]
    fn folders_do_not_carry_a_file() {
        let rows = rows(&["src/a.js"]);

        assert!(rows[0].is_folder());
        assert_eq!(rows[0].file, None);
        assert_eq!(
            serde_json::to_value(&rows[0]).unwrap(),
            serde_json::json!({ "type": "folder", "key": "folder:src", "name": "src", "path": "src", "depth": 0 })
        );
        assert_eq!(
            serde_json::to_value(&rows[1]).unwrap()["file"]["status"],
            "M"
        );
    }

    // El original no fusiona carpetas de niveles distintos aunque compartan la
    // ruta calculada (rutas con `//` o que empiezan por `/`).
    #[test]
    fn keeps_odd_paths_apart_like_the_original() {
        let rows = rows(&["//x", "/y"]);

        assert_eq!(shape(&rows), ["", "  ", "    x", "  y"]);
    }

    fn rows_with(paths: &[&str], options: FileTreeOptions) -> Vec<FileTreeRow<TestFile>> {
        build_file_tree_rows_with(&files(paths), |file| &file.path, |_| true, options)
    }

    const COMPACT: FileTreeOptions = FileTreeOptions {
        compact_folders: true,
        flat: false,
    };

    #[test]
    fn compacts_chains_of_single_folders_into_one_row() {
        let rows = rows_with(
            &[
                "src/a/b/one.js",
                "src/a/b/two.js",
                "docs/guide/intro.md",
                "x.js",
            ],
            COMPACT,
        );

        assert_eq!(
            shape(&rows),
            [
                "docs/guide",
                "  intro.md",
                "src/a/b",
                "  one.js",
                "  two.js",
                "x.js"
            ]
        );
        // La fila es de la carpeta más honda: su ruta y su clave.
        assert_eq!(rows[2].path, "src/a/b");
        assert_eq!(rows[2].key, "folder:src/a/b");
    }

    #[test]
    fn stops_compacting_where_a_folder_has_more_than_one_child() {
        let rows = rows_with(&["src/lib/a.js", "src/lib/b/c.js", "src/x.js"], COMPACT);

        assert_eq!(
            shape(&rows),
            ["src", "  lib", "    b", "      c.js", "    a.js", "  x.js"]
        );
    }

    #[test]
    fn a_compact_row_folds_its_deepest_folder() {
        let rows = build_file_tree_rows_with(
            &files(&["src/a/b/one.js", "top.js"]),
            |file| &file.path,
            |path| path != "src/a/b",
            COMPACT,
        );

        assert_eq!(shape(&rows), ["src/a/b", "top.js"]);
    }

    #[test]
    fn a_flat_list_has_no_folders_and_sorts_by_path() {
        let rows = rows_with(
            &["src/lib/b.js", "README.md", "src/a.js", "src/lib/a.js"],
            FileTreeOptions {
                flat: true,
                ..Default::default()
            },
        );

        assert!(rows.iter().all(|row| !row.is_folder() && row.depth == 0));
        let paths: Vec<&str> = rows.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(
            paths,
            ["README.md", "src/a.js", "src/lib/a.js", "src/lib/b.js"]
        );
        assert_eq!(rows[2].name, "a.js");
        assert_eq!(rows[2].key, "file:src/lib/a.js");
        assert_eq!(parent_folder(&rows[2].path), "src/lib");
        assert_eq!(parent_folder("README.md"), "");
    }

    #[test]
    fn keeps_the_listing_order_between_names_that_sort_the_same() {
        let rows = rows(&["b.js", "a.js", "A.js"]);

        assert_eq!(shape(&rows), ["a.js", "A.js", "b.js"]);
    }
}
