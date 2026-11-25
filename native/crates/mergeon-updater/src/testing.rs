//! Dobles de prueba: un transporte HTTP en memoria y un instalador que solo
//! registra las llamadas. Sirven para probar el actualizador (y la app que lo
//! usa) sin red y sin instalar nada.

use std::io::{self, Cursor, Read};
use std::sync::{Arc, Mutex, PoisonError};

use crate::download::DownloadedUpdate;
use crate::error::UpdaterError;
use crate::installer::{InstallOptions, InstallOutcome, Installer};
use crate::signature::{sign_manifest_bytes, signature_file_name};
use crate::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};

/// Respuesta que arma un manejador de `FakeTransport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Corta la conexión tras entregar estos bytes del cuerpo.
    pub fail_after: Option<usize>,
}

impl FakeResponse {
    /// 200 con `Content-Length`.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status: 200,
            headers: vec![("Content-Length".to_string(), body.len().to_string())],
            body,
            fail_after: None,
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            fail_after: None,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn failing_after(mut self, bytes: usize) -> Self {
        self.fail_after = Some(bytes);
        self
    }

    /// Sirve `data` respetando la cabecera `Range` de la petición, como un
    /// servidor de archivos estáticos: 206 con `Content-Range`, o 416.
    pub fn file(request: &HttpRequest, data: &[u8]) -> Self {
        let start = request
            .header("range")
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|range| range.split('-').next())
            .and_then(|start| start.parse::<usize>().ok());

        match start {
            None => Self::ok(data),
            Some(start) if start >= data.len() => {
                Self::status(416).with_header("Content-Range", &format!("bytes */{}", data.len()))
            }
            Some(start) => {
                let mut response = Self::ok(&data[start..]);
                response.status = 206;
                response.with_header(
                    "Content-Range",
                    &format!("bytes {start}-{}/{}", data.len() - 1, data.len()),
                )
            }
        }
    }
}

/// Entrega unos bytes y luego falla, como una conexión que se corta.
struct FailingReader {
    data: Cursor<Vec<u8>>,
    remaining: usize,
}

impl Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection reset by peer",
            ));
        }
        let limit = buffer.len().min(self.remaining);
        let read = self.data.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection reset by peer",
            ));
        }
        self.remaining -= read;
        Ok(read)
    }
}

type Handler = Arc<dyn Fn(&HttpRequest) -> Result<FakeResponse, TransportError> + Send + Sync>;

/// Transporte en memoria: rutas por sufijo de URL y registro de peticiones.
/// Lo que no coincide con ninguna ruta responde 404.
#[derive(Default)]
pub struct FakeTransport {
    routes: Mutex<Vec<(String, Handler)>>,
    requests: Mutex<Vec<HttpRequest>>,
    signing_key: Mutex<Option<String>>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configura una clave privada Ed25519 (base64) para que `serve("*.yml", ...)`
    /// publique también automáticamente su firma `<archivo>.sig`.
    pub fn with_signing_key(self, private_key_b64: impl Into<String>) -> Self {
        *self
            .signing_key
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(private_key_b64.into());
        self
    }

    /// Registra un manejador para las URL cuya ruta (sin query) termina en
    /// `suffix`. Si varias rutas coinciden, gana la registrada más tarde.
    pub fn route(
        &self,
        suffix: &str,
        handler: impl Fn(&HttpRequest) -> Result<FakeResponse, TransportError> + Send + Sync + 'static,
    ) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((suffix.to_string(), Arc::new(handler)));
    }

    /// Responde siempre 200 con `body`. Si se configuró `with_signing_key` y
    /// `suffix` termina en `.yml`, publica también su firma `.sig`.
    pub fn serve(&self, suffix: &str, body: impl Into<Vec<u8>>) {
        let body = body.into();
        let signing_key = self
            .signing_key
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if suffix.ends_with(".yml")
            && let Some(private_key) = signing_key
            && let Ok(sig) = sign_manifest_bytes(&body, &private_key)
        {
            let sig_suffix = signature_file_name(suffix);
            self.route(&sig_suffix, move |_| Ok(FakeResponse::ok(sig.clone())));
        }
        self.route(suffix, move |_| Ok(FakeResponse::ok(body.clone())));
    }

    /// Sirve un archivo con soporte de `Range`.
    pub fn serve_file(&self, suffix: &str, data: impl Into<Vec<u8>>) {
        let data = data.into();
        self.route(suffix, move |request| {
            Ok(FakeResponse::file(request, &data))
        });
    }

    pub fn serve_status(&self, suffix: &str, status: u16) {
        self.route(suffix, move |_| Ok(FakeResponse::status(status)));
    }

    /// La petición no obtiene respuesta (red caída).
    pub fn fail(&self, suffix: &str, error: TransportError) {
        self.route(suffix, move |_| Err(error.clone()));
    }

    /// Peticiones recibidas, en orden.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// URL de las peticiones recibidas, sin query.
    pub fn requested_paths(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|request| strip_query(&request.url).to_string())
            .collect()
    }
}

fn strip_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

impl HttpTransport for FakeTransport {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());

        let path = strip_query(&request.url);
        let handler = self
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .rev()
            .find(|(suffix, _)| path.ends_with(suffix.as_str()))
            .map(|(_, handler)| Arc::clone(handler));

        let response = match handler {
            Some(handler) => handler(request)?,
            None => FakeResponse::status(404),
        };

        let body: Box<dyn Read + Send> = match response.fail_after {
            Some(remaining) => Box::new(FailingReader {
                data: Cursor::new(response.body),
                remaining,
            }),
            None => Box::new(Cursor::new(response.body)),
        };
        Ok(HttpResponse {
            status: response.status,
            headers: response.headers,
            body,
        })
    }
}

/// Instalador de prueba: no toca nada, registra cada llamada y devuelve el
/// resultado configurado.
pub struct RecordingInstaller {
    calls: Mutex<Vec<(DownloadedUpdate, InstallOptions)>>,
    result: Result<InstallOutcome, UpdaterError>,
}

impl RecordingInstaller {
    /// Instala "bien" y no pide relanzar.
    pub fn new() -> Self {
        Self::with_result(Ok(InstallOutcome::default()))
    }

    pub fn with_outcome(outcome: InstallOutcome) -> Self {
        Self::with_result(Ok(outcome))
    }

    pub fn failing(error: UpdaterError) -> Self {
        Self::with_result(Err(error))
    }

    fn with_result(result: Result<InstallOutcome, UpdaterError>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            result,
        }
    }

    /// Llamadas recibidas, en orden.
    pub fn calls(&self) -> Vec<(DownloadedUpdate, InstallOptions)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Default for RecordingInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl Installer for RecordingInstaller {
    fn install(
        &self,
        update: &DownloadedUpdate,
        options: &InstallOptions,
    ) -> Result<InstallOutcome, UpdaterError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((update.clone(), *options));
        self.result.clone()
    }
}
