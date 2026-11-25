//! El transporte real contra un servidor HTTP local: comprueba la integración
//! con `ureq` (estados, `Range`, redirecciones, cortes) sin salir de la máquina.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mergeon_updater::testing::RecordingInstaller;
use mergeon_updater::{
    AppUpdater, Arch, CheckOptions, Env, HttpRequest, HttpTransport, InstallResult, Platform,
    TransportError, UpdateStatus, UpdaterConfig, UreqTransport, sign_manifest_bytes,
};

use common::{
    EventLog, fast_download_settings, manifest, payload, settle, test_private_key, test_public_key,
};

/// Petición recibida por el servidor de prueba.
#[derive(Debug, Clone)]
struct Received {
    path: String,
    headers: Vec<(String, String)>,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Byte inicial pedido con `Range: bytes=N-`.
    fn range_start(&self) -> Option<usize> {
        self.header("range")?
            .strip_prefix("bytes=")?
            .split('-')
            .next()?
            .parse()
            .ok()
    }
}

/// Respuesta en crudo: cabecera de estado, cabeceras y cuerpo. Con
/// `cut_body_at` el servidor anuncia el cuerpo entero y cierra antes.
struct Reply {
    status: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    cut_body_at: Option<usize>,
}

impl Reply {
    fn new(status: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            cut_body_at: None,
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Archivo estático con soporte de `Range`.
    fn file(request: &Received, data: &[u8]) -> Self {
        match request.range_start() {
            Some(start) if start < data.len() => Self::new("206 Partial Content", &data[start..])
                .header(
                    "Content-Range",
                    &format!("bytes {start}-{}/{}", data.len() - 1, data.len()),
                ),
            Some(_) => Self::new("416 Range Not Satisfiable", ""),
            None => Self::new("200 OK", data),
        }
    }
}

struct TestServer {
    url: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl TestServer {
    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn read_request(stream: &TcpStream) -> Option<Received> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let path = request_line.split_whitespace().nth(1)?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Some(Received { path, headers })
}

/// Servidor HTTP/1.1 mínimo: una petición por conexión.
fn serve(handler: impl Fn(&Received) -> Reply + Send + 'static) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some(request) = read_request(&stream) else {
                continue;
            };
            log.lock().unwrap().push(request.clone());
            let reply = handler(&request);

            let mut head = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (name, value) in &reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");

            let body_end = reply.cut_body_at.unwrap_or(reply.body.len());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&reply.body[..body_end]);
            let _ = stream.flush();
        }
    });

    TestServer { url, received }
}

fn get(url: String, headers: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        url,
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        response_timeout: Duration::from_secs(5),
        body_timeout: Duration::from_secs(5),
    }
}

fn read_body(mut body: Box<dyn Read + Send>) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    body.read_to_end(&mut data)?;
    Ok(data)
}

#[test]
fn default_transport_rejects_plain_http_urls() {
    let server = serve(|_| Reply::new("200 OK", "should not be reached"));
    let result = UreqTransport::new().get(&get(format!("{}/latest.yml", server.url), &[]));
    assert!(
        matches!(result, Err(TransportError::Network(_))),
        "UreqTransport::new() debe rechazar http://: {result:?}"
    );
    assert!(server.received().is_empty());
}

#[test]
fn returns_error_statuses_as_responses() {
    let server = serve(|request| match request.path.as_str() {
        "/app/latest.yml" => Reply::new("200 OK", "version: 1.0.0\n"),
        _ => Reply::new("404 Not Found", "nope"),
    });
    let transport = UreqTransport::new_allowing_insecure_http_for_tests();

    let found = transport
        .get(&get(
            format!("{}/app/latest.yml", server.url),
            &[("Accept", "*/*")],
        ))
        .unwrap();
    assert_eq!(found.status, 200);
    assert!(found.is_ok());
    assert_eq!(found.content_length(), Some(15));
    assert_eq!(read_body(found.body).unwrap(), b"version: 1.0.0\n");

    // Un 404 no es un `Err`: el estado decide el mensaje que ve el usuario.
    let missing = transport
        .get(&get(format!("{}/app/other.yml", server.url), &[]))
        .unwrap();
    assert_eq!(missing.status, 404);
    assert!(!missing.is_ok());
    assert_eq!(read_body(missing.body).unwrap(), b"nope");

    let received = server.received();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].path, "/app/latest.yml");
    assert_eq!(received[0].header("accept"), Some("*/*"));
    assert!(
        received[0]
            .header("user-agent")
            .unwrap()
            .starts_with("Mergeon/")
    );
}

#[test]
fn forwards_the_range_header_and_follows_redirects() {
    let data = payload(5_000);
    let served = data.clone();
    let server = serve(move |request| match request.path.as_str() {
        // Como `releases/latest/download/<archivo>` de GitHub.
        "/latest/download/app.bin" => {
            Reply::new("302 Found", "").header("Location", "/assets/app.bin")
        }
        "/assets/app.bin" => Reply::file(request, &served),
        _ => Reply::new("404 Not Found", ""),
    });

    let response = UreqTransport::new_allowing_insecure_http_for_tests()
        .get(&get(
            format!("{}/latest/download/app.bin", server.url),
            &[("Range", "bytes=1000-")],
        ))
        .unwrap();

    assert_eq!(response.status, 206);
    assert_eq!(
        response.header("content-range"),
        Some("bytes 1000-4999/5000")
    );
    assert_eq!(read_body(response.body).unwrap(), &data[1000..]);
    let received = server.received();
    assert_eq!(received.len(), 2);
    assert_eq!(received[1].path, "/assets/app.bin");
    assert_eq!(received[1].range_start(), Some(1000));
}

#[test]
fn reports_a_truncated_body_as_a_read_error() {
    let server = serve(|_| {
        let mut reply = Reply::new("200 OK", payload(10_000));
        reply.cut_body_at = Some(4_000);
        reply
    });

    let response = UreqTransport::new_allowing_insecure_http_for_tests()
        .get(&get(format!("{}/app.bin", server.url), &[]))
        .unwrap();

    // Sin este error un corte de conexión pasaría por descarga completa.
    assert_eq!(response.content_length(), Some(10_000));
    assert!(read_body(response.body).is_err());
}

#[test]
fn reports_connection_failures_and_timeouts() {
    // Puerto que ya no escucha nadie.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let result = UreqTransport::new_allowing_insecure_http_for_tests()
        .get(&get(format!("{closed}/latest.yml"), &[]));
    assert!(
        matches!(result, Err(TransportError::Network(_))),
        "{result:?}"
    );

    // Servidor que acepta la conexión y nunca contesta.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let silent = format!("http://{}", listener.local_addr().unwrap());
    let holder = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(600));
        drop(stream);
    });
    let mut request = get(format!("{silent}/latest.yml"), &[]);
    request.response_timeout = Duration::from_millis(200);
    let result = UreqTransport::new_allowing_insecure_http_for_tests().get(&request);
    assert_eq!(result.err(), Some(TransportError::Timeout));
    holder.join().unwrap();
}

#[test]
fn updates_end_to_end_over_real_http_resuming_a_dropped_connection() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(600_000);
    let name = "Mergeon-Setup-0.2.0.exe";
    let manifest_text = manifest("0.2.0", &[(name, &data)]);
    let manifest_sig = sign_manifest_bytes(manifest_text.as_bytes(), test_private_key()).unwrap();
    let artifact_requests = Arc::new(AtomicUsize::new(0));

    let server = {
        let data = data.clone();
        let artifact_requests = artifact_requests.clone();
        serve(move |request| {
            let path = request.path.split('?').next().unwrap_or("");
            match path {
                "/win/latest.yml" => Reply::new("200 OK", manifest_text.clone()),
                "/win/latest.yml.sig" => Reply::new("200 OK", manifest_sig.clone()),
                "/win/Mergeon-Setup-0.2.0.exe" => {
                    let mut reply = Reply::file(request, &data);
                    // La primera conexión se cae a mitad del instalador.
                    if artifact_requests.fetch_add(1, Ordering::SeqCst) == 0 {
                        reply.cut_body_at = Some(250_000);
                    }
                    reply
                }
                _ => Reply::new("404 Not Found", ""),
            }
        })
    };

    let feed_url = format!("{}/win/", server.url);
    let mut config = UpdaterConfig::from_env(
        Some("0.1.6".to_string()),
        true,
        Platform::Win32,
        Arch::X64,
        &Env::from_pairs([("APP_UPDATE_FEED_URL", feed_url.as_str())]),
        cache.path(),
    );
    config.check_interval = None;
    config.download = fast_download_settings();
    config.public_key = Some(test_public_key().to_string());
    config.allow_insecure_http = true;

    let installer = Arc::new(RecordingInstaller::new());
    let updater = AppUpdater::builder(config)
        .transport(Arc::new(
            UreqTransport::new_allowing_insecure_http_for_tests(),
        ))
        .installer(installer.clone())
        .build();
    let events = EventLog::attach(&updater);

    updater.check(CheckOptions::manual());
    settle(&updater);

    let state = updater.state();
    assert_eq!(state.status, UpdateStatus::Downloaded, "{state:?}");
    assert_eq!(state.latest_version.as_deref(), Some("0.2.0"));
    assert_eq!(
        fs::read(cache.path().join("pending").join(name)).unwrap(),
        data
    );
    assert_eq!(
        events.status_sequence(),
        [
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
        ]
    );

    let received = server.received();
    assert_eq!(received.len(), 4);
    assert!(
        received[0].path.starts_with("/win/latest.yml?noCache="),
        "{}",
        received[0].path
    );
    assert!(
        received[1].path.starts_with("/win/latest.yml.sig?noCache="),
        "{}",
        received[1].path
    );
    assert_eq!(received[2].range_start(), None);
    // El reintento pide exactamente lo que faltaba.
    let resumed_from = received[3].range_start().unwrap();
    assert!(
        resumed_from > 0 && resumed_from <= 250_000,
        "{resumed_from}"
    );

    assert_eq!(updater.install(), InstallResult::ok());
    assert_eq!(installer.calls().len(), 1);
}
