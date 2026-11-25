use std::fmt;
use std::io::{self, Read};
use std::time::Duration;

/// Petición GET ya armada: la URL lleva la query incluida.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Tiempo máximo hasta tener las cabeceras de la respuesta.
    pub response_timeout: Duration,
    /// Tiempo máximo para leer el cuerpo completo.
    pub body_timeout: Duration,
}

impl HttpRequest {
    /// Valor de una cabecera, sin distinguir mayúsculas.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// Respuesta con el cuerpo sin leer: los instaladores pesan decenas de MB y se
/// vuelcan a disco a medida que llegan.
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl HttpResponse {
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Valor de una cabecera, sin distinguir mayúsculas.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }

    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.trim().parse().ok()
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Fallo de transporte: la petición no obtuvo respuesta HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    /// Cualquier otro fallo de red: DNS, conexión, TLS...
    Network(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("request timed out"),
            Self::Network(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TransportError {}

/// Transporte HTTP mínimo. Una respuesta con estado de error (4xx/5xx) es un
/// `Ok`: solo se devuelve `Err` cuando no hubo respuesta.
pub trait HttpTransport: Send + Sync {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// Transporte real sobre `ureq` (TLS con rustls). Sigue redirecciones, que es
/// lo que hace falta para `releases/latest/download/...` de GitHub.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        Self::with_https_only(true)
    }

    /// Solo para tests de integración contra un servidor HTTP local.
    #[doc(hidden)]
    pub fn new_allowing_insecure_http_for_tests() -> Self {
        Self::with_https_only(false)
    }

    fn with_https_only(https_only: bool) -> Self {
        let config = ureq::Agent::config_builder()
            // Los 4xx/5xx se devuelven como respuesta: el estado decide el mensaje.
            .http_status_as_error(false)
            .https_only(https_only)
            .user_agent(concat!("Mergeon/", env!("CARGO_PKG_VERSION")))
            .build();

        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for UreqTransport {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut builder = ureq::http::Request::builder()
            .method("GET")
            .uri(request.url.as_str());

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let http_request = builder
            .body(())
            .map_err(|error| TransportError::Network(error.to_string()))?;

        let http_request = self
            .agent
            .configure_request(http_request)
            .timeout_resolve(Some(request.response_timeout))
            .timeout_connect(Some(request.response_timeout))
            .timeout_recv_response(Some(request.response_timeout))
            .timeout_recv_body(Some(request.body_timeout))
            .build();

        let response = self.agent.run(http_request).map_err(map_ureq_error)?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let (_, body) = response.into_parts();

        Ok(HttpResponse {
            status,
            headers,
            body: Box::new(body.into_reader()),
        })
    }
}

fn map_ureq_error(error: ureq::Error) -> TransportError {
    match error {
        ureq::Error::Timeout(_) => TransportError::Timeout,
        ureq::Error::Io(io_error) if io_error.kind() == io::ErrorKind::TimedOut => {
            TransportError::Timeout
        }
        other => TransportError::Network(other.to_string()),
    }
}
