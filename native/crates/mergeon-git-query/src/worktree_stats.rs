//! Cuánto cambió cada archivo con cambios locales: las cifras `+N −N` (y las
//! barritas) de la lista de "Local Changes".
//!
//! El estado de git no trae cifras, así que se leen aparte:
//! `git diff --numstat -z` (working tree frente al índice) y
//! `git diff --cached --numstat -z` (índice frente a HEAD). Lo que no se sigue
//! no sale en ninguno de los dos: se cuentan las líneas del archivo como
//! añadidas, solo si es texto y pequeño.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use crate::commit_tree::looks_binary;
use crate::error::{QueryResult, read};
use crate::exec;
use crate::paths::normalize_repo_relative_path;
use crate::text::parse_count;

/// Un archivo sin seguimiento más grande que esto se queda sin cifras: contar
/// sus líneas obligaría a leerlo entero.
pub const MAX_UNTRACKED_STATS_BYTES: u64 = 1024 * 1024;

/// Líneas añadidas y quitadas de un archivo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineStats {
    pub additions: u64,
    pub deletions: u64,
}

impl LineStats {
    pub fn new(additions: u64, deletions: u64) -> Self {
        Self {
            additions,
            deletions,
        }
    }
}

/// Las cifras de los cambios locales, por ruta (relativa a la raíz del
/// repositorio, como en el estado). Un archivo que no está (binario, sin
/// seguimiento grande o que no es texto) no tiene cifras.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeStats {
    /// Sin preparar: `git diff --numstat` y, de lo que no se sigue, sus líneas.
    pub unstaged: HashMap<String, LineStats>,
    /// Preparados: `git diff --cached --numstat`. En renombres, por la ruta de
    /// destino.
    pub staged: HashMap<String, LineStats>,
}

/*
 * `--numstat -z`: `<+>\t<->\t<ruta>\0`; en renombres y copias la ruta va vacía
 * y le siguen dos campos, `<origen>\0<destino>\0`. Los binarios traen `-` en
 * las dos cifras: no tienen cifras que enseñar y se dejan fuera.
 */
pub(crate) fn parse_numstat(output: &str) -> HashMap<String, LineStats> {
    let mut stats = HashMap::new();
    let mut fields = output.split('\0');
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let _source = fields.next();
            fields.next().unwrap_or_default()
        } else {
            path
        };
        if path.is_empty() || added == "-" || deleted == "-" {
            continue;
        }
        stats.insert(
            path.to_string(),
            LineStats::new(parse_count(added), parse_count(deleted)),
        );
    }
    stats
}

/// Las líneas de un archivo sin seguimiento, contadas como añadidas (lo que
/// enseñaría su diff): `None` si no es un archivo normal, si pasa de
/// [`MAX_UNTRACKED_STATS_BYTES`] o si parece binario.
fn untracked_line_count(repo_path: &Path, file_path: &str) -> Option<u64> {
    let relative = normalize_repo_relative_path(file_path).ok()?;
    let absolute = repo_path.join(relative);
    // Sin seguir enlaces: un enlace simbólico no es el archivo al que apunta.
    let metadata = std::fs::symlink_metadata(&absolute).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_UNTRACKED_STATS_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(&absolute)
        .ok()?
        // Por si creció entre medias: nunca más del límite (y uno de más para
        // saberlo).
        .take(MAX_UNTRACKED_STATS_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_UNTRACKED_STATS_BYTES || looks_binary(&bytes) {
        return None;
    }
    Some(count_lines(&bytes))
}

/// Líneas como las cuenta git: cada salto de línea, más la última si no lo trae.
fn count_lines(bytes: &[u8]) -> u64 {
    let breaks = bytes.iter().filter(|&&byte| byte == b'\n').count() as u64;
    breaks + u64::from(bytes.last().is_some_and(|&byte| byte != b'\n'))
}

/// Las cifras de los cambios locales. `untracked` son las rutas sin
/// seguimiento del estado (`??`), que git no cuenta.
///
/// Lee todo el working tree: llamarla en un hilo de fondo.
pub fn get_working_tree_stats(
    repo_path: &Path,
    untracked: &[String],
) -> QueryResult<WorkingTreeStats> {
    read("Failed to get change stats", || {
        // Sin el refresco oportunista del índice: esta lectura corre sola en
        // segundo plano y no debe quitarle el `index.lock` a un `git add`.
        let git = exec::open(repo_path)?.with_env("GIT_OPTIONAL_LOCKS", "0");
        let unstaged = exec::raw(&git, &["diff", "--numstat", "-z"])?;
        // En un repositorio sin commits `--cached` compara con el árbol vacío.
        let staged = exec::raw(&git, &["diff", "--cached", "--numstat", "-z"])?;

        let mut stats = WorkingTreeStats {
            unstaged: parse_numstat(&unstaged),
            staged: parse_numstat(&staged),
        };
        for path in untracked {
            if let Some(lines) = untracked_line_count(repo_path, path) {
                stats
                    .unstaged
                    .insert(path.clone(), LineStats::new(lines, 0));
            }
        }
        Ok(stats)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn stats(entries: &[(&str, u64, u64)]) -> HashMap<String, LineStats> {
        entries
            .iter()
            .map(|(path, added, deleted)| (path.to_string(), LineStats::new(*added, *deleted)))
            .collect()
    }

    #[test]
    fn parses_numstat_with_renames_binaries_and_odd_paths() {
        let output = "3\t1\tsrc/app.js\0\
                      -\t-\timage.png\0\
                      2\t0\t\0old name.txt\0new name.txt\0\
                      1\t1\ttab\there.txt\0\
                      0\t4\tcañón/ñ.md\0";
        assert_eq!(
            parse_numstat(output),
            stats(&[
                ("src/app.js", 3, 1),
                ("new name.txt", 2, 0),
                ("tab\there.txt", 1, 1),
                ("cañón/ñ.md", 0, 4),
            ])
        );
        assert!(parse_numstat("").is_empty());
    }

    #[test]
    fn counts_lines_like_git() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"one"), 1);
        assert_eq!(count_lines(b"one\n"), 1);
        assert_eq!(count_lines(b"one\ntwo"), 2);
        assert_eq!(count_lines(b"\n\n"), 2);
    }

    #[test]
    fn reads_unstaged_and_staged_stats_with_real_git() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\ntwo\nthree\n", "a");
        repo.commit_file("dir with space/ñandú é.txt", "uno\n", "unicode");
        repo.commit_file(
            "old name.txt",
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n",
            "rename me",
        );
        repo.commit_file("both.txt", "x\n", "both");

        // Sin preparar: dos líneas cambiadas y una nueva.
        repo.write("a.txt", "one\nTWO\nthree\nfour\n");
        // Ruta con espacios y Unicode, sin preparar.
        repo.write("dir with space/ñandú é.txt", "uno\ndos\n");
        // Renombre preparado con un cambio.
        repo.git(&["mv", "old name.txt", "new name.txt"]);
        repo.write("new name.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n");
        repo.git(&["add", "new name.txt"]);
        // A medio preparar: una línea preparada y otra sin preparar.
        repo.write("both.txt", "x\ny\n");
        repo.git(&["add", "both.txt"]);
        repo.write("both.txt", "x\ny\nz\n");
        // Binario preparado.
        repo.write("image.bin", [0u8, 1, 2, 3, 0, 255]);
        repo.git(&["add", "image.bin"]);

        let result = get_working_tree_stats(&repo.path, &[]).unwrap();

        assert_eq!(
            result.unstaged,
            stats(&[
                ("a.txt", 2, 1),
                ("dir with space/ñandú é.txt", 1, 0),
                ("both.txt", 1, 0),
            ])
        );
        assert_eq!(
            result.staged,
            stats(&[("new name.txt", 1, 1), ("both.txt", 1, 0)])
        );
    }

    #[test]
    fn counts_small_untracked_text_files_as_added() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "a\n", "a");
        repo.write("notes ñ.md", "one\ntwo\nthree");
        repo.write("empty.txt", "");
        repo.write("blob.bin", [0u8, 159, 146, 150]);
        let big = "line\n".repeat((MAX_UNTRACKED_STATS_BYTES as usize / 5) + 1);
        repo.write("big.log", big);
        let untracked: Vec<String> = ["notes ñ.md", "empty.txt", "blob.bin", "big.log", "gone.txt"]
            .iter()
            .map(|path| path.to_string())
            .collect();

        let result = get_working_tree_stats(&repo.path, &untracked).unwrap();

        // Ni el binario, ni el grande, ni el que ya no está tienen cifras.
        assert_eq!(
            result.unstaged,
            stats(&[("notes ñ.md", 3, 0), ("empty.txt", 0, 0)])
        );
        assert!(result.staged.is_empty());
    }

    #[test]
    fn an_untracked_path_outside_the_repository_is_not_read() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "a\n", "a");
        std::fs::write(repo.root().join("outside.txt"), "secret\n").unwrap();

        let result = get_working_tree_stats(&repo.path, &["../outside.txt".to_string()]).unwrap();

        assert!(result.unstaged.is_empty());
    }

    #[test]
    fn a_repository_without_commits_has_staged_stats() {
        let repo = TestRepo::new();
        repo.write("first.txt", "a\nb\n");
        repo.git(&["add", "first.txt"]);

        let result = get_working_tree_stats(&repo.path, &[]).unwrap();

        assert_eq!(result.staged, stats(&[("first.txt", 2, 0)]));
        assert!(result.unstaged.is_empty());
    }

    #[test]
    fn fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let error = get_working_tree_stats(dir.path(), &[]).unwrap_err();
        assert!(
            error.message().starts_with("Failed to get change stats: "),
            "{error}"
        );
    }
}
