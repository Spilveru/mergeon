//! Contrato de errores de las lecturas.
//!
//! En la app Electron las lecturas LANZAN (`createReadError`) y las mutaciones
//! devuelven siempre un objeto estructurado; aquí las lecturas devuelven
//! `Result<T, QueryError>` con el mismo mensaje `"<prefijo>: <mensaje>"`.

use std::thread::ScopedJoinHandle;

use mergeon_gitcmd::GitError;

/// Error de una lectura, con el mensaje ya listo para mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError {
    message: String,
}

impl QueryError {
    /// `createReadError(prefix, error)`: si el error no trae mensaje se repite
    /// el prefijo, igual que `toErrorMessage(error, prefix)`.
    pub fn read(prefix: &str, message: impl AsRef<str>) -> Self {
        let message = message.as_ref();
        let detail = if message.is_empty() { prefix } else { message };
        Self {
            message: format!("{prefix}: {detail}"),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for QueryError {}

pub type QueryResult<T> = Result<T, QueryError>;

/// Mensaje del error original, antes de ponerle el prefijo del canal.
#[derive(Debug)]
pub(crate) struct Cause(pub String);

impl From<GitError> for Cause {
    fn from(error: GitError) -> Self {
        Cause(error.message)
    }
}

impl From<String> for Cause {
    fn from(message: String) -> Self {
        Cause(message)
    }
}

impl From<&str> for Cause {
    fn from(message: &str) -> Self {
        Cause(message.to_string())
    }
}

pub(crate) type Inner<T> = Result<T, Cause>;

/// Resultado de un hilo lector lanzado con `std::thread::scope`.
pub(crate) fn join<T, E: Into<Cause>>(handle: ScopedJoinHandle<'_, Result<T, E>>) -> Inner<T> {
    match handle.join() {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(Cause("A git reader thread panicked.".to_string())),
    }
}

/// El `try { ... } catch (error) { throw createReadError(prefix, error) }` de
/// cada handler de lectura.
pub(crate) fn read<T>(prefix: &str, task: impl FnOnce() -> Inner<T>) -> QueryResult<T> {
    task().map_err(|cause| QueryError::read(prefix, cause.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_the_original_message() {
        let error = QueryError::read("Failed to get status", "fatal: not a git repository\n");
        assert_eq!(
            error.message(),
            "Failed to get status: fatal: not a git repository\n"
        );
        assert_eq!(error.to_string(), error.message());
    }

    #[test]
    fn repeats_the_prefix_when_the_error_has_no_message() {
        let error = QueryError::read("Failed to get tags", "");
        assert_eq!(error.message(), "Failed to get tags: Failed to get tags");
    }
}
