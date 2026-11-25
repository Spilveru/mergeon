//! Ejecución de git para las lecturas.
//!
//! `mergeon_gitcmd::Git` no trae el filtro de argumentos peligrosos que
//! simple-git aplica a cada comando (`blockUnsafeOperationsPlugin`), y varias
//! lecturas pasan a git texto que viene de la interfaz (una URL, un ref, un
//! hash). Aquí se replica la parte de ese filtro que depende de los argumentos:
//! un `--upload-pack` colado donde se esperaba una URL haría que git ejecutara
//! un programa arbitrario.

use std::path::Path;

use mergeon_gitcmd::{Git, GitError, GitErrorKind, GitOutput, GitResult};

use crate::fs_util::node_error_message;

/// Lo que lanza simple-git al crear una instancia sobre un directorio que no existe.
pub(crate) const MISSING_DIRECTORY: &str =
    "Cannot use simple-git on a directory that does not exist";

const UNSAFE_PACK: &str =
    "Use of --upload-pack or --receive-pack is not permitted without enabling allowUnsafePack";
const UNSAFE_TEMPLATE: &str =
    "Use of --template is not permitted without enabling allowUnsafeTemplateDir";

/// Mensaje con el que simple-git rechazaría el comando, si lo rechazara.
/// `args[0]` es el subcomando; lo que va después de `--` son rutas.
pub(crate) fn unsafe_argument<S: AsRef<str>>(args: &[S]) -> Option<&'static str> {
    for argument in args.iter().skip(1).map(AsRef::as_ref) {
        if argument == "--" {
            break;
        }
        if !argument.starts_with('-') || argument.len() < 2 {
            continue;
        }

        // `--nombre=valor`: solo cuenta el nombre de la opción.
        let name = match argument.find('=') {
            Some(at) if argument.starts_with("--") && at > 2 => &argument[..at],
            _ => argument,
        };
        if name.contains("--upload-pack") || name.contains("--receive-pack") {
            return Some(UNSAFE_PACK);
        }
        if name.contains("--template") {
            return Some(UNSAFE_TEMPLATE);
        }
    }
    None
}

/// `getGit(repoPath)`: simple-git comprueba que el directorio exista antes de
/// nada, y ese es el error que ve el usuario al abrir un repo que ya no está.
pub(crate) fn open(repo_path: &Path) -> GitResult<Git> {
    match std::fs::metadata(repo_path) {
        Ok(metadata) if metadata.is_dir() => Ok(Git::new(repo_path)),
        Ok(_) => Err(GitError::message(MISSING_DIRECTORY)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(GitError::message(MISSING_DIRECTORY))
        }
        Err(error) => Err(GitError::message(node_error_message(
            &error, "stat", repo_path,
        ))),
    }
}

/// Para las lecturas que delegan en `mergeon_gitcmd`: si git no pudo arrancar
/// porque el directorio no existe, el error es el mismo que daría [`open`].
pub(crate) fn missing_directory_error(repo_path: &Path, error: GitError) -> GitError {
    if error.kind == GitErrorKind::Spawn && !repo_path.is_dir() {
        return GitError::message(MISSING_DIRECTORY);
    }
    error
}

/// `git.raw(args)` con el filtro de simple-git por delante.
pub(crate) fn raw<S: AsRef<str>>(git: &Git, args: &[S]) -> GitResult<String> {
    if let Some(message) = unsafe_argument(args) {
        return Err(GitError::message(message));
    }
    git.raw(args.iter().map(AsRef::as_ref))
}

/// `Git::exec` (para decidir por código de salida) con el mismo filtro delante.
pub(crate) fn exec<S: AsRef<str>>(git: &Git, args: &[S]) -> GitResult<GitOutput> {
    if let Some(message) = unsafe_argument(args) {
        return Err(GitError::message(message));
    }
    git.exec(args.iter().map(AsRef::as_ref))
}

/// Salida de [`raw_capped`].
pub(crate) enum Capped {
    Complete(String),
    /// Pasó del tope: git se mató y solo se sabe que sacó al menos estos bytes.
    TooLarge(u64),
}

/// `raw` (mismo filtro y criterio de error) sin cargar más de `limit` bytes.
pub(crate) fn raw_capped<S: AsRef<str>>(git: &Git, args: &[S], limit: u64) -> GitResult<Capped> {
    if let Some(message) = unsafe_argument(args) {
        return Err(GitError::message(message));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let capped = git.exec_capped(args.iter().map(AsRef::as_ref), limit)?;
    if capped.exceeded {
        return Ok(Capped::TooLarge(capped.output.stdout.len() as u64));
    }
    if capped.output.is_task_error() {
        return Err(GitError::from_output(&capped.output));
    }
    Ok(Capped::Complete(capped.output.stdout_text()))
}

/// Salida de [`bytes_capped`].
pub(crate) enum CappedBytes {
    Complete(Vec<u8>),
    /// Pasó del tope: git se mató y solo se sabe que sacó al menos estos bytes.
    TooLarge(u64),
}

/// [`raw_capped`] en bytes (contenido binario: imágenes).
pub(crate) fn bytes_capped<S: AsRef<str>>(
    git: &Git,
    args: &[S],
    limit: u64,
) -> GitResult<CappedBytes> {
    if let Some(message) = unsafe_argument(args) {
        return Err(GitError::message(message));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let capped = git.exec_capped(args.iter().map(AsRef::as_ref), limit)?;
    if capped.exceeded {
        return Ok(CappedBytes::TooLarge(capped.output.stdout.len() as u64));
    }
    if capped.output.is_task_error() {
        return Err(GitError::from_output(&capped.output));
    }
    Ok(CappedBytes::Complete(capped.output.stdout))
}

/// Los primeros `limit` bytes de la salida de git (contenido binario): se
/// deja de leer y se mata git en cuanto llegan. Para algo que solo se mira
/// por encima (la cabecera de una imagen): un error da vacío.
pub(crate) fn bytes_head<S: AsRef<str>>(git: &Git, args: &[S], limit: usize) -> Vec<u8> {
    if unsafe_argument(args).is_some() {
        return Vec::new();
    }
    match git.exec_capped(args.iter().map(AsRef::as_ref), limit) {
        Ok(capped) if capped.exceeded || !capped.output.is_task_error() => {
            let mut bytes = capped.output.stdout;
            bytes.truncate(limit);
            bytes
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_pack_and_template_options() {
        assert_eq!(
            unsafe_argument(&["ls-remote", "--heads", "--upload-pack=touch /tmp/x"]),
            Some(UNSAFE_PACK)
        );
        assert_eq!(
            unsafe_argument(&["ls-remote", "--receive-pack", "x"]),
            Some(UNSAFE_PACK)
        );
        assert_eq!(
            unsafe_argument(&["log", "--template=/tmp/x"]),
            Some(UNSAFE_TEMPLATE)
        );
    }

    #[test]
    fn allows_ordinary_arguments_and_paths() {
        assert_eq!(unsafe_argument(&["log", "--all", "-1", "main"]), None);
        // El valor de una opción no es una opción.
        assert_eq!(unsafe_argument(&["log", "--grep=--upload-pack"]), None);
        // Después de `--` solo hay rutas.
        assert_eq!(unsafe_argument(&["diff", "--", "--upload-pack"]), None);
    }

    #[test]
    fn refuses_a_directory_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open(dir.path()).is_ok());

        let missing = dir.path().join("gone");
        assert_eq!(open(&missing).unwrap_err().message, MISSING_DIRECTORY);

        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(open(&file).unwrap_err().message, MISSING_DIRECTORY);

        let spawn_error = Git::new(&missing).raw(["status"]).unwrap_err();
        assert_eq!(
            missing_directory_error(&missing, spawn_error).message,
            MISSING_DIRECTORY
        );
    }

    #[test]
    fn a_rejected_command_never_reaches_git() {
        let dir = tempfile::tempdir().unwrap();
        let error = raw(&Git::new(dir.path()), &["ls-remote", "--upload-pack=x"]).unwrap_err();
        assert_eq!(error.message, UNSAFE_PACK);
    }
}
