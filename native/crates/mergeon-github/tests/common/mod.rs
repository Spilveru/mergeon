//! Transporte falso: reproduce los mocks de `fetch` de los tests de vitest.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mergeon_github::{
    AppMode, GitHubClient, GitHubConfig, HttpRequest, HttpResponse, HttpTransport, TransportError,
};
use serde_json::Value;

type Reply = Result<HttpResponse, TransportError>;

#[derive(Default)]
pub struct FakeTransport {
    queued: Mutex<VecDeque<Reply>>,
    fallback: Mutex<Option<Reply>>,
    calls: Mutex<Vec<HttpRequest>>,
}

impl FakeTransport {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// `fetch.mockResolvedValue(...)` / `fetch.mockRejectedValue(...)`.
    pub fn respond(&self, reply: Reply) {
        *self.fallback.lock().unwrap() = Some(reply);
    }

    /// `fetch.mockResolvedValueOnce(...)`.
    pub fn respond_once(&self, reply: Reply) {
        self.queued.lock().unwrap().push_back(reply);
    }

    pub fn calls(&self) -> Vec<HttpRequest> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// Cuerpo JSON de la petición número `index`.
    pub fn body(&self, index: usize) -> Value {
        let calls = self.calls();
        let body = calls[index].body.as_deref().expect("request without body");
        serde_json::from_str(body).expect("request body is not JSON")
    }
}

impl HttpTransport for FakeTransport {
    fn send(&self, request: &HttpRequest) -> Reply {
        self.calls.lock().unwrap().push(request.clone());

        if let Some(reply) = self.queued.lock().unwrap().pop_front() {
            return reply;
        }

        self.fallback
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| panic!("unexpected request: {request:?}"))
    }
}

pub fn json_response(payload: Value) -> Reply {
    json_response_with_status(payload, 200)
}

pub fn json_response_with_status(payload: Value, status: u16) -> Reply {
    Ok(HttpResponse {
        status,
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: payload.to_string().into_bytes(),
    })
}

pub fn raw_response(body: &str, status: u16) -> Reply {
    Ok(HttpResponse {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    })
}

/// Cliente en modo OAuth App (el modo por defecto del original).
pub fn oauth_client(transport: &Arc<FakeTransport>) -> GitHubClient {
    GitHubClient::with_transport(GitHubConfig::default(), transport.clone())
}

/// Equivale a `MERGEON_GITHUB_APP_MODE=github-app` en los tests de vitest.
pub fn github_app_client(transport: &Arc<FakeTransport>) -> GitHubClient {
    GitHubClient::with_transport(
        GitHubConfig::default().with_app_mode(AppMode::GitHubApp),
        transport.clone(),
    )
}
