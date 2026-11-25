//! Servidor HTTP de prueba con autenticación básica delante de
//! `git http-backend` (CGI), para probar de punta a punta fetch, pull, push y
//! clone contra un remoto HTTP que pide usuario y contraseña. Solo para tests
//! (de este crate y de los que lo usan); escucha en `127.0.0.1` y un puerto
//! libre.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::auth::base64;

#[derive(Default)]
struct Counters {
    authorized: AtomicUsize,
    rejected: AtomicUsize,
}

/// Lo que el servidor vio llegar (para comprobar que nada raro viaja).
#[derive(Debug, Clone, Default)]
pub struct SeenRequest {
    pub method: String,
    pub path: String,
    pub authorized: bool,
}

pub struct AuthHttpServer {
    base_url: String,
    counters: Arc<Counters>,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
    stop: Arc<AtomicBool>,
    address: std::net::SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl AuthHttpServer {
    /// Sirve los repositorios de `project_root` (`<url>/<nombre>.git`) a quien
    /// se autentique como `username` / `password`.
    pub fn start(project_root: &Path, username: &str, password: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let expected = format!(
            "Basic {}",
            base64(format!("{username}:{password}").as_bytes())
        );
        let counters = Arc::new(Counters::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let root = project_root.to_path_buf();
        let user = username.to_string();

        let thread = {
            let counters = counters.clone();
            let seen = seen.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("mergeon-test-http".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(stream) = stream else { continue };
                        let counters = counters.clone();
                        let seen = seen.clone();
                        let root = root.clone();
                        let expected = expected.clone();
                        let user = user.clone();
                        std::thread::spawn(move || {
                            let _ = serve(stream, &root, &expected, &user, &counters, &seen);
                        });
                    }
                })?
        };

        Ok(Self {
            base_url: format!("http://{address}"),
            counters,
            seen,
            stop,
            address,
            thread: Some(thread),
        })
    }

    /// `http://127.0.0.1:<puerto>`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// URL de un repositorio de la raíz: `http://127.0.0.1:<puerto>/<name>`.
    pub fn repo_url(&self, name: &str) -> String {
        format!("{}/{name}", self.base_url)
    }

    /// Peticiones que traían las credenciales buenas.
    pub fn authorized_requests(&self) -> usize {
        self.counters.authorized.load(Ordering::SeqCst)
    }

    /// Peticiones rechazadas con 401 (sin credenciales o con otras).
    pub fn rejected_requests(&self) -> usize {
        self.counters.rejected.load(Ordering::SeqCst)
    }

    pub fn seen(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for AuthHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Despierta al `accept` para que vea la orden de parar.
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_body(
    reader: &mut BufReader<TcpStream>,
    headers: &HashMap<String, String>,
) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let mut size_line = String::new();
            reader.read_line(&mut size_line)?;
            let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
                .unwrap_or(0);
            if size == 0 {
                // Trailers hasta la línea vacía.
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                break;
            }
            let mut chunk = vec![0; size];
            reader.read_exact(&mut chunk)?;
            body.extend_from_slice(&chunk);
            let mut crlf = String::new();
            reader.read_line(&mut crlf)?;
        }
    } else if let Some(length) = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        body.resize(length, 0);
        reader.read_exact(&mut body)?;
    }
    Ok(body)
}

fn serve(
    stream: TcpStream,
    root: &Path,
    expected_authorization: &str,
    user: &str,
    counters: &Counters,
    seen: &Mutex<Vec<SeenRequest>>,
) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let authorized =
        headers.get("authorization").map(String::as_str) == Some(expected_authorization);
    seen.lock().unwrap().push(SeenRequest {
        method: method.clone(),
        path: path.to_string(),
        authorized,
    });

    if !authorized {
        counters.rejected.fetch_add(1, Ordering::SeqCst);
        // El cuerpo de un POST rechazado se descarta.
        let _ = read_body(&mut reader, &headers);
        writer.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"mergeon-test\"\r\n\
              Content-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return writer.flush();
    }
    counters.authorized.fetch_add(1, Ordering::SeqCst);

    if headers
        .get("expect")
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = read_body(&mut reader, &headers)?;

    let mut command = Command::new("git");
    command
        .arg("http-backend")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("HOME", root)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", &method)
        .env("REMOTE_USER", user)
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(content_type) = headers.get("content-type") {
        command.env("CONTENT_TYPE", content_type);
    }
    if let Some(encoding) = headers.get("content-encoding") {
        command.env("HTTP_CONTENT_ENCODING", encoding);
    }
    if let Some(protocol) = headers.get("git-protocol") {
        command.env("HTTP_GIT_PROTOCOL", protocol);
    }
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("stdin");
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(&body);
    });
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .read_to_end(&mut output)?;
    let _ = feeder.join();
    let _ = child.wait();

    // Respuesta CGI: cabeceras, línea vacía y cuerpo.
    let split = output
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| (at, 4))
        .or_else(|| {
            output
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|at| (at, 2))
        });
    let (head, body) = match split {
        Some((at, gap)) => (&output[..at], &output[at + gap..]),
        None => (&output[..0], &output[..]),
    };
    let mut status = "200 OK".to_string();
    let mut response = Vec::new();
    for line in String::from_utf8_lossy(head).lines() {
        let line = line.trim_end();
        if let Some(value) = line.strip_prefix("Status:") {
            status = value.trim().to_string();
        } else if !line.is_empty() {
            response.extend_from_slice(line.as_bytes());
            response.extend_from_slice(b"\r\n");
        }
    }
    writer.write_all(format!("HTTP/1.1 {status}\r\n").as_bytes())?;
    writer.write_all(&response)?;
    writer.write_all(
        format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )?;
    writer.write_all(body)?;
    writer.flush()
}

fn null_device() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("NUL")
    } else {
        PathBuf::from("/dev/null")
    }
}
