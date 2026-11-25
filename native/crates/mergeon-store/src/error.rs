use std::io;
use std::path::{Path, PathBuf};

/// Fallo de una operación del store.
#[derive(Debug)]
pub enum StoreError {
    /// No se pudo leer o escribir el archivo de configuración.
    Io { path: PathBuf, source: io::Error },
    /// El valor a guardar no cumple el esquema. El mensaje es el mismo que
    /// produce `conf`: `Config schema violation: ...`.
    Schema(String),
    /// El cifrado está disponible pero no pudo cifrar el token (en el original,
    /// `safeStorage.encryptString` lanza).
    Encryption,
    /// El llavero del sistema no está disponible y el cifrador no permite
    /// guardar tokens en claro sin consentimiento explícito.
    KeyringUnavailable,
    /// Un valor no se pudo convertir a JSON.
    Serialize(String),
    /// No se pudo determinar el directorio de datos de usuario.
    NoUserDataDir,
}

impl StoreError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(
                    f,
                    "Failed to access settings file {}: {source}",
                    path.display()
                )
            }
            Self::Schema(message) => f.write_str(message),
            Self::Encryption => f.write_str("Failed to encrypt the token"),
            Self::KeyringUnavailable => {
                f.write_str("System keyring is unavailable; refusing to store tokens in plain text")
            }
            Self::Serialize(message) => write!(f, "Failed to serialize settings: {message}"),
            Self::NoUserDataDir => f.write_str("Could not determine the user data directory"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
