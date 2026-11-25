//! Las dos versiones de una imagen cambiada (antes y después), para verlas
//! lado a lado en lugar del "Binary files differ" del parche.
//!
//! Se leen los bytes tal cual: de git con `cat-file` (`<rev>:<ruta>` o el
//! índice, `:<ruta>`) y del working tree desde disco. Cada lado se mide antes
//! de cargarlo y nada pasa de [`MAX_IMAGE_BYTES`]. Como el resto de lecturas,
//! se llama fuera del hilo de la interfaz.
//!
//! Idea tomada de `load_images` de gitcraft (MIT OR Apache-2.0): un commit
//! compara con su primer padre, lo preparado con HEAD y lo no preparado con el
//! índice.

use std::path::Path;

use mergeon_gitcmd::Git;

use crate::diff::rename_source;
use crate::error::{Inner, QueryResult, read};
use crate::exec;
use crate::fs_util::stat;
use crate::paths::{normalize_repo_relative_path, normalize_repository_file_path};

/// Una versión más pesada que esto no se carga: se dice su tamaño.
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// De una versión que no se carga se leen solo sus primeros bytes, donde
/// están las medidas (la cabecera de un PNG, el `SOFn` de un JPEG detrás de
/// sus metadatos, la raíz de un SVG).
pub const IMAGE_HEADER_BYTES: usize = 64 * 1024;

/// Qué dos versiones de la imagen se quieren.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImagePairRequest {
    /// Un archivo en un commit: antes, en su primer padre (con la ruta de
    /// origen si el commit lo renombró); después, en el commit.
    Commit {
        hash: String,
        path: String,
        /// La ruta en el padre, si ya se sabe (historial de un archivo).
        previous_path: Option<String>,
    },
    /// De una revisión a otra (la comparación de dos ramas).
    Range {
        from: String,
        to: String,
        path: String,
        old_path: Option<String>,
    },
    /// Cambios locales: lo preparado (HEAD → índice), lo no preparado
    /// (índice → disco) o un archivo sin seguir (solo el disco).
    WorkingTree {
        path: String,
        staged: bool,
        untracked: bool,
    },
}

/// Una versión de la imagen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageBlob {
    /// Los bytes del archivo; con `too_large`, solo los primeros
    /// ([`IMAGE_HEADER_BYTES`] como mucho), para medirla.
    pub bytes: Vec<u8>,
    /// Tamaño del archivo en bytes.
    pub size: u64,
    /// Pasa de [`MAX_IMAGE_BYTES`] y no se cargó.
    pub too_large: bool,
}

/// Antes y después. `None` en un lado: la imagen no existía (añadida) o ya
/// no existe (borrada).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImagePair {
    pub before: Option<ImageBlob>,
    pub after: Option<ImageBlob>,
}

/// De dónde sale una versión.
enum Side {
    /// `<rev>:<ruta>`, o el índice con `rev` vacío (`:<ruta>`).
    Object(String),
    /// El archivo en disco, relativo al repositorio.
    Disk(String),
}

/// Lee las dos versiones que pide `request`.
pub fn read_image_pair(repo_path: &Path, request: &ImagePairRequest) -> QueryResult<ImagePair> {
    read("Failed to read the image", || {
        let git = exec::open(repo_path)?;
        match request {
            ImagePairRequest::Commit {
                hash,
                path,
                previous_path,
            } => {
                let hash = revision(hash)?;
                let path = relative(path)?;
                let after = read_side(&git, repo_path, &object(hash, &path))?;
                let parent = format!("{hash}^1");
                let previous = match previous_path.as_deref().filter(|path| !path.is_empty()) {
                    Some(previous) => relative(previous)?,
                    None => path.clone(),
                };
                let mut before = read_side(&git, repo_path, &object(&parent, &previous))?;
                // Sin la ruta en el padre puede que el commit la renombrara.
                if before.is_none()
                    && previous_path.is_none()
                    && let Some(source) = rename_source(&git, hash, &path)?
                {
                    before = read_side(&git, repo_path, &object(&parent, &source))?;
                }
                Ok(ImagePair { before, after })
            }
            ImagePairRequest::Range {
                from,
                to,
                path,
                old_path,
            } => {
                let (from, to) = (revision(from)?, revision(to)?);
                let path = relative(path)?;
                let old = match old_path.as_deref().filter(|old| !old.is_empty()) {
                    Some(old) => relative(old)?,
                    None => path.clone(),
                };
                Ok(ImagePair {
                    before: read_side(&git, repo_path, &object(from, &old))?,
                    after: read_side(&git, repo_path, &object(to, &path))?,
                })
            }
            ImagePairRequest::WorkingTree {
                path,
                staged,
                untracked,
            } => {
                let path = normalize_repository_file_path(repo_path, path)
                    .map_err(|error| error.to_string())?;
                let (before, after) = if *untracked {
                    (None, Side::Disk(path))
                } else if *staged {
                    (Some(object("HEAD", &path)), object("", &path))
                } else {
                    (Some(object("", &path)), Side::Disk(path))
                };
                let before = match before {
                    Some(side) => read_side(&git, repo_path, &side)?,
                    None => None,
                };
                Ok(ImagePair {
                    before,
                    after: read_side(&git, repo_path, &after)?,
                })
            }
        }
    })
}

fn object(rev: &str, path: &str) -> Side {
    Side::Object(format!("{rev}:{path}"))
}

fn relative(path: &str) -> Inner<String> {
    normalize_repo_relative_path(path).map_err(|error| error.to_string().into())
}

/// Una revisión no puede parecer una opción de `cat-file`.
fn revision(rev: &str) -> Inner<&str> {
    let rev = rev.trim();
    if rev.is_empty() || rev.starts_with('-') {
        return Err(format!("Invalid revision: {rev}").into());
    }
    Ok(rev)
}

fn too_large(size: u64, mut header: Vec<u8>) -> ImageBlob {
    header.truncate(IMAGE_HEADER_BYTES);
    ImageBlob {
        bytes: header,
        size,
        too_large: true,
    }
}

/// Los primeros bytes de un archivo en disco (vacío si no se puede leer: la
/// cabecera es solo para las medidas).
fn disk_header(path: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let mut header = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file
            .take(IMAGE_HEADER_BYTES as u64)
            .read_to_end(&mut header);
    }
    header
}

fn read_side(git: &Git, repo_path: &Path, side: &Side) -> Inner<Option<ImageBlob>> {
    match side {
        Side::Disk(path) => {
            let absolute = repo_path.join(path);
            let Ok(metadata) = stat(&absolute) else {
                return Ok(None);
            };
            if !metadata.is_file() {
                return Ok(None);
            }
            let size = metadata.len();
            if size > MAX_IMAGE_BYTES {
                return Ok(Some(too_large(size, disk_header(&absolute))));
            }
            let bytes = std::fs::read(&absolute).map_err(|error| error.to_string())?;
            Ok(Some(ImageBlob {
                size: bytes.len() as u64,
                bytes,
                too_large: false,
            }))
        }
        Side::Object(spec) => {
            // Medir primero: si no existe en esa revisión, no hay versión.
            let sized = exec::exec(git, &["cat-file", "-s", spec.as_str()])?;
            if !sized.success() {
                return Ok(None);
            }
            let Ok(size) = sized.stdout_text().trim().parse::<u64>() else {
                return Ok(None);
            };
            if size > MAX_IMAGE_BYTES {
                let header = exec::bytes_head(
                    git,
                    &["cat-file", "blob", spec.as_str()],
                    IMAGE_HEADER_BYTES,
                );
                return Ok(Some(too_large(size, header)));
            }
            match exec::bytes_capped(git, &["cat-file", "blob", spec.as_str()], MAX_IMAGE_BYTES)? {
                exec::CappedBytes::Complete(bytes) => Ok(Some(ImageBlob {
                    size: bytes.len() as u64,
                    bytes,
                    too_large: false,
                })),
                exec::CappedBytes::TooLarge(size) => Ok(Some(too_large(size, Vec::new()))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    /// Un PNG de verdad no hace falta: se comparan bytes, no se decodifican.
    fn png(tag: u8, len: usize) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(std::iter::repeat_n(tag, len));
        bytes
    }

    fn commit_bytes(repo: &TestRepo, name: &str, bytes: &[u8], message: &str) -> String {
        repo.write(name, bytes);
        repo.git(&["add", "--", name]);
        repo.git(&["commit", "-m", message]);
        repo.head()
    }

    fn commit(hash: &str, path: &str) -> ImagePairRequest {
        ImagePairRequest::Commit {
            hash: hash.to_string(),
            path: path.to_string(),
            previous_path: None,
        }
    }

    fn bytes(blob: &Option<ImageBlob>) -> Option<&[u8]> {
        blob.as_ref().map(|blob| blob.bytes.as_slice())
    }

    #[test]
    fn a_commit_compares_the_image_with_its_parent() {
        let repo = TestRepo::new();
        let added = commit_bytes(&repo, "img/logo.png", &png(1, 64), "add");
        let changed = commit_bytes(&repo, "img/logo.png", &png(2, 80), "change");
        repo.git(&["rm", "-q", "img/logo.png"]);
        repo.git(&["commit", "-m", "remove"]);
        let removed = repo.head();

        // Añadida (y commit raíz): solo "después".
        let pair = read_image_pair(&repo.path, &commit(&added, "img/logo.png")).unwrap();
        assert_eq!(pair.before, None);
        assert_eq!(bytes(&pair.after), Some(png(1, 64).as_slice()));

        let pair = read_image_pair(&repo.path, &commit(&changed, "img/logo.png")).unwrap();
        assert_eq!(bytes(&pair.before), Some(png(1, 64).as_slice()));
        assert_eq!(bytes(&pair.after), Some(png(2, 80).as_slice()));
        assert_eq!(pair.after.as_ref().unwrap().size, 88);

        // Borrada: solo "antes".
        let pair = read_image_pair(&repo.path, &commit(&removed, "img/logo.png")).unwrap();
        assert_eq!(bytes(&pair.before), Some(png(2, 80).as_slice()));
        assert_eq!(pair.after, None);
    }

    #[test]
    fn a_renamed_image_is_found_under_its_old_name() {
        let repo = TestRepo::new();
        commit_bytes(&repo, "old name.png", &png(7, 200), "add");
        repo.git(&["mv", "old name.png", "new name.png"]);
        repo.git(&["commit", "-m", "rename"]);
        let renamed = repo.head();

        let pair = read_image_pair(&repo.path, &commit(&renamed, "new name.png")).unwrap();
        assert_eq!(bytes(&pair.before), Some(png(7, 200).as_slice()));
        assert_eq!(bytes(&pair.after), Some(png(7, 200).as_slice()));

        let request = ImagePairRequest::Commit {
            hash: renamed,
            path: "new name.png".into(),
            previous_path: Some("old name.png".into()),
        };
        let pair = read_image_pair(&repo.path, &request).unwrap();
        assert_eq!(bytes(&pair.before), Some(png(7, 200).as_slice()));
    }

    #[test]
    fn local_changes_read_head_the_index_and_the_disk() {
        let repo = TestRepo::new();
        commit_bytes(&repo, "a.png", &png(1, 10), "base");
        repo.write("a.png", png(2, 10));
        repo.git(&["add", "a.png"]);
        repo.write("a.png", png(3, 10));
        repo.write("new.png", png(4, 10));

        let working = |path: &str, staged, untracked| ImagePairRequest::WorkingTree {
            path: path.into(),
            staged,
            untracked,
        };
        let staged = read_image_pair(&repo.path, &working("a.png", true, false)).unwrap();
        assert_eq!(bytes(&staged.before), Some(png(1, 10).as_slice()));
        assert_eq!(bytes(&staged.after), Some(png(2, 10).as_slice()));

        let unstaged = read_image_pair(&repo.path, &working("a.png", false, false)).unwrap();
        assert_eq!(bytes(&unstaged.before), Some(png(2, 10).as_slice()));
        assert_eq!(bytes(&unstaged.after), Some(png(3, 10).as_slice()));

        let untracked = read_image_pair(&repo.path, &working("new.png", false, true)).unwrap();
        assert_eq!(untracked.before, None);
        assert_eq!(bytes(&untracked.after), Some(png(4, 10).as_slice()));

        // Borrada en disco sin preparar: solo "antes".
        std::fs::remove_file(repo.path.join("a.png")).unwrap();
        let deleted = read_image_pair(&repo.path, &working("a.png", false, false)).unwrap();
        assert_eq!(bytes(&deleted.before), Some(png(2, 10).as_slice()));
        assert_eq!(deleted.after, None);

        // Fuera del repositorio, no.
        assert!(read_image_pair(&repo.path, &working("../x.png", false, true)).is_err());
    }

    #[test]
    fn a_range_compares_two_revisions() {
        let repo = TestRepo::new();
        let from = commit_bytes(&repo, "a.png", &png(1, 5), "one");
        commit_bytes(&repo, "b.txt", b"x", "two");
        let to = commit_bytes(&repo, "a.png", &png(9, 5), "three");
        let request = ImagePairRequest::Range {
            from,
            to,
            path: "a.png".into(),
            old_path: None,
        };
        let pair = read_image_pair(&repo.path, &request).unwrap();
        assert_eq!(bytes(&pair.before), Some(png(1, 5).as_slice()));
        assert_eq!(bytes(&pair.after), Some(png(9, 5).as_slice()));

        let bad = ImagePairRequest::Range {
            from: "--output=x".into(),
            to: "HEAD".into(),
            path: "a.png".into(),
            old_path: None,
        };
        assert!(read_image_pair(&repo.path, &bad).is_err());
    }

    #[test]
    fn an_image_over_the_limit_is_measured_but_not_loaded() {
        let repo = TestRepo::new();
        let size = MAX_IMAGE_BYTES as usize + 1;
        let big = png(5, size - 8);
        let hash = commit_bytes(&repo, "big.png", &big, "big");

        // De lo que no se carga queda solo la cabecera, para medirla.
        let pair = read_image_pair(&repo.path, &commit(&hash, "big.png")).unwrap();
        let after = pair.after.unwrap();
        assert!(after.too_large);
        assert_eq!(after.bytes, big[..IMAGE_HEADER_BYTES]);
        assert_eq!(after.size, size as u64);

        let request = ImagePairRequest::WorkingTree {
            path: "big.png".into(),
            staged: false,
            untracked: false,
        };
        let pair = read_image_pair(&repo.path, &request).unwrap();
        let (before, after) = (pair.before.unwrap(), pair.after.unwrap());
        assert!(before.too_large);
        assert_eq!(before.bytes, big[..IMAGE_HEADER_BYTES]);
        assert!(after.too_large);
        assert_eq!(after.bytes, big[..IMAGE_HEADER_BYTES]);
    }
}
