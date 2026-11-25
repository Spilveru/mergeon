use std::fmt;
use std::io;
use std::time::Duration;

/// Tiempo máximo por petición (`timeoutMs = 15000` en el original).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(15_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// Petición HTTP ya armada: la URL lleva la query incluida.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Duration,
}

impl HttpRequest {
    /// Valor de una cabecera, sin distinguir mayúsculas.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

// La cabecera Authorization y el cuerpo llevan tokens: nunca se imprimen.
impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, value)| {
                if name.eq_ignore_ascii_case("authorization") {
                    (name.as_str(), "<redacted>")
                } else {
                    (name.as_str(), value.as_str())
                }
            })
            .collect();

        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body", &self.body.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// `response.ok` de fetch: estado 2xx.
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Valor de una cabecera, sin distinguir mayúsculas.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

// El cuerpo de las respuestas de OAuth trae los tokens recién emitidos.
impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &format_args!("<{} bytes>", self.body.len()))
            .finish()
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
    /// Se agotó el tiempo de la petición (`AbortError` por el timer).
    Timeout,
    /// La petición se canceló desde fuera (`AbortError` por la señal externa).
    Cancelled,
    /// Cualquier otro fallo de red: DNS, conexión, TLS...
    Network(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("request timed out"),
            Self::Cancelled => f.write_str("request was cancelled"),
            Self::Network(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TransportError {}

/// Transporte HTTP mínimo. Una respuesta con estado de error (4xx/5xx) es un
/// `Ok`: solo se devuelve `Err` cuando no hubo respuesta.
pub trait HttpTransport: Send + Sync {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// Transporte real sobre `ureq` (TLS con rustls).
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            // Los estados 4xx/5xx traen un cuerpo JSON con el mensaje de GitHub.
            .http_status_as_error(false)
            .timeout_global(Some(DEFAULT_TIMEOUT))
            // La API de GitHub rechaza las peticiones sin User-Agent.
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

impl UreqTransport {
    fn run<B: ureq::AsSendBody>(
        &self,
        request: &HttpRequest,
        body: B,
    ) -> Result<HttpResponse, TransportError> {
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(request.url.as_str());

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let http_request = builder
            .body(body)
            .map_err(|error| TransportError::Network(error.to_string()))?;

        let http_request = self
            .agent
            .configure_request(http_request)
            .timeout_global(Some(request.timeout))
            .build();

        let mut response = self.agent.run(http_request).map_err(map_ureq_error)?;

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
        let body = response.body_mut().read_to_vec().map_err(map_ureq_error)?;

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

impl HttpTransport for UreqTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        // Sin cuerpo se manda `()`: un String vacío añadiría Content-Length a un GET.
        match &request.body {
            Some(body) => self.run(request, body.as_str()),
            None => self.run(request, ()),
        }
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
