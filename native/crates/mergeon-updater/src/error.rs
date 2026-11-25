use std::fmt;

/// Fallo del actualizador.
///
/// `Display` da un mensaje corto y sin datos variables (URL, rutas, hashes): es
/// el texto que pasa por `summarize_updater_error` y termina en un aviso de la
/// interfaz. El detalle completo queda en `Debug`, que es lo que va al log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdaterError {
    /// La petición no obtuvo respuesta (DNS, conexión, TLS, timeout...).
    Network {
        url: String,
        detail: String,
    },
    /// El servidor respondió con un estado que no es 2xx.
    HttpStatus {
        status: u16,
        url: String,
    },
    /// El manifiesto del canal no es el YAML que publica electron-builder.
    InvalidManifest {
        detail: String,
    },
    /// El canal o un artefacto del manifiesto no usa HTTPS.
    InsecureUrl {
        url: String,
    },
    /// La firma Ed25519 del manifiesto falta, no es válida o no hay clave pública.
    InvalidSignature {
        detail: String,
    },
    /// La versión del manifiesto no es un semver válido.
    InvalidVersion {
        version: String,
    },
    /// La versión de la propia app no es un semver válido.
    InvalidCurrentVersion {
        version: String,
    },
    /// El manifiesto no trae `files` ni `path`.
    NoFiles,
    /// Ningún archivo del manifiesto sirve para esta plataforma.
    NoSuitableArtifact {
        wanted: String,
    },
    /// El archivo elegido no trae sha512: no se instala nada sin verificar.
    MissingChecksum {
        file: String,
    },
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    SizeMismatch {
        expected: u64,
        actual: u64,
    },
    Io {
        context: String,
        detail: String,
    },
    Install {
        detail: String,
    },
    /// El actualizador se cerró mientras trabajaba.
    Cancelled,
}

impl UpdaterError {
    pub(crate) fn io(context: &str, error: &std::io::Error) -> Self {
        Self::Io {
            context: context.to_string(),
            detail: error.to_string(),
        }
    }

    pub(crate) fn install(detail: impl Into<String>) -> Self {
        Self::Install {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for UpdaterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // "network" es lo que `summarize_updater_error` reconoce como fallo
            // de conexión (en Node eran los códigos ENOTFOUND, ECONNREFUSED...).
            Self::Network { detail, .. } => write!(f, "network error: {detail}"),
            Self::HttpStatus { status, .. } => {
                write!(f, "El servidor de actualizaciones respondió {status}.")
            }
            Self::InvalidManifest { .. } => f.write_str(
                "El canal de actualizaciones devolvió un manifiesto que no se puede leer.",
            ),
            Self::InsecureUrl { .. } => {
                f.write_str("El canal de actualizaciones debe usar una conexión segura (HTTPS).")
            }
            Self::InvalidSignature { .. } => {
                f.write_str("No se pudo verificar la firma criptográfica de la actualización.")
            }
            Self::InvalidVersion { .. } => {
                f.write_str("La versión publicada en el canal de actualizaciones no es válida.")
            }
            Self::InvalidCurrentVersion { .. } => {
                f.write_str("La versión de esta aplicación no es válida y no se puede comparar.")
            }
            Self::NoFiles => f.write_str("El manifiesto de actualización no incluye archivos."),
            Self::NoSuitableArtifact { .. } => f.write_str(
                "El canal de actualizaciones no tiene un instalador para esta plataforma.",
            ),
            Self::MissingChecksum { .. } => {
                f.write_str("El manifiesto de actualización no incluye el sha512 del instalador.")
            }
            Self::ChecksumMismatch { .. } => {
                f.write_str("La descarga de la actualización está dañada (el sha512 no coincide).")
            }
            Self::SizeMismatch { .. } => {
                f.write_str("La descarga de la actualización quedó incompleta.")
            }
            Self::Io { context, detail } => write!(f, "{context}: {detail}"),
            Self::Install { detail } => {
                write!(f, "No se pudo instalar la actualización: {detail}")
            }
            Self::Cancelled => f.write_str("Actualización cancelada."),
        }
    }
}

impl std::error::Error for UpdaterError {}
