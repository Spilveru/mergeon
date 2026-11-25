//! El transporte real contra un servidor HTTP local de un solo uso: comprueba
//! la integración con `ureq` sin salir de la máquina.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use mergeon_github::{
    GitHubClient, GitHubConfig, GitHubErrorCode, HttpMethod, HttpRequest, HttpTransport,
    TransportError, UreqTransport,
};

/// Lee una petición HTTP/1.1 completa (cabeceras y cuerpo) como texto.
fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    let mut content_length = 0;

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap();
        }
        let end_of_head = line == "\r\n";
        head.push_str(&line);
        if end_of_head {
            break;
        }
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    head + &String::from_utf8(body).unwrap()
}

/// Servidor que atiende una sola conexión con la respuesta dada y entrega la
/// petición recibida por el canal.
fn serve_once(response: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        stream.write_all(response.as_bytes()).unwrap();
        sender.send(request).unwrap();
    });

    (address, receiver)
}

fn request(method: HttpMethod, url: String, body: Option<&str>) -> HttpRequest {
    HttpRequest {
        method,
        url,
        headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), "Bearer ghu_token".to_string()),
        ],
        body: body.map(str::to_string),
        timeout: Duration::from_secs(5),
    }
}

#[test]
fn sends_the_request_and_returns_error_statuses_as_responses() {
    let (address, received) = serve_once(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\
         Content-Length: 29\r\nConnection: close\r\n\r\n{\"message\":\"Bad credentials\"}",
    );

    let response = UreqTransport::new()
        .send(&request(
            HttpMethod::Post,
            format!("{address}/login/oauth/access_token?page=1"),
            Some("{\"client_id\":\"abc\"}"),
        ))
        .unwrap();

    assert_eq!(response.status, 401);
    assert!(!response.is_ok());
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.body, b"{\"message\":\"Bad credentials\"}");

    let sent = received.recv().unwrap().to_lowercase();
    assert!(sent.starts_with("post /login/oauth/access_token?page=1 http/1.1\r\n"));
    assert!(sent.contains("accept: application/json\r\n"));
    assert!(sent.contains("authorization: bearer ghu_token\r\n"));
    assert!(sent.contains("user-agent: mergeon/"));
    assert!(sent.ends_with("\r\n\r\n{\"client_id\":\"abc\"}"));
}

#[test]
fn sends_get_requests_without_a_body() {
    let (address, received) = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nConnection: close\r\n\r\n[]",
    );

    let response = UreqTransport::new()
        .send(&request(HttpMethod::Get, format!("{address}/user"), None))
        .unwrap();

    assert_eq!(response.status, 200);
    assert!(response.is_ok());
    assert_eq!(response.body, b"[]");

    let sent = received.recv().unwrap().to_lowercase();
    assert!(sent.starts_with("get /user http/1.1\r\n"));
    assert!(!sent.contains("content-length"));
    assert!(sent.ends_with("\r\n\r\n"));
}

#[test]
fn reports_a_timeout_when_the_server_does_not_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (release, hold) = mpsc::channel::<()>();

    let server = thread::spawn(move || {
        // Acepta la conexión y la deja abierta sin responder.
        let (_stream, _) = listener.accept().unwrap();
        let _ = hold.recv();
    });

    // De punta a punta: el cliente lo traduce a `github_timeout`.
    let config = GitHubConfig::default()
        .with_api_url(address)
        .with_timeout(Duration::from_millis(200));
    let error = GitHubClient::new(config)
        .authenticate_with_token("ghp_token")
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::GithubTimeout);
    assert_eq!(error.status, 504);

    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn reports_a_network_error_when_nothing_is_listening() {
    // Se reserva un puerto y se libera: nadie escucha en él.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let error = UreqTransport::new()
        .send(&request(HttpMethod::Get, format!("{address}/user"), None))
        .unwrap_err();
    assert!(matches!(error, TransportError::Network(_)), "{error:?}");

    let client = GitHubClient::new(GitHubConfig::default().with_api_url(address.clone()));
    let error = client.authenticate_with_token("ghp_token").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubUnavailable);
    assert_eq!(error.status, 503);
    assert_eq!(error.message, format!("GitHub is unreachable at {address}"));
}

/// Prueba de humo contra GitHub real (TLS incluido). Necesita red, así que no
/// corre por defecto: `cargo test -p mergeon-github -- --ignored`.
#[test]
#[ignore = "necesita red"]
fn live_github_rejects_an_invalid_token() {
    let error = GitHubClient::new(GitHubConfig::default())
        .authenticate_with_token("ghp_mergeon_invalid_token")
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
    assert_eq!(error.status, 401);
    assert_eq!(error.message, "Bad credentials");
}
