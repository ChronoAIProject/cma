use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use cma_sdk::{Client, IdempotencyKey};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, task::JoinHandle};

pub mod fixtures;

pub struct Expected {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
    pub status: StatusCode,
    pub response: Option<Value>,
}

pub struct Server {
    pub client: Client,
    requests: Arc<Mutex<VecDeque<Expected>>>,
    task: JoinHandle<()>,
}

impl Server {
    pub async fn new() -> Self {
        let requests = Arc::new(Mutex::new(VecDeque::new()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .fallback(any(handle))
            .with_state(Arc::clone(&requests));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = Client::builder(format!("http://{address}/gateway"))
            .unwrap()
            .bearer_token("sdk-fixture")
            .unwrap()
            .build()
            .unwrap();
        Self {
            client,
            requests,
            task,
        }
    }

    pub fn expect(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
        status: u16,
        response: Option<Value>,
    ) {
        self.requests.lock().unwrap().push_back(Expected {
            method,
            path: format!("/gateway/api/v1{path}"),
            body,
            status: StatusCode::from_u16(status).unwrap(),
            response,
        });
    }

    pub fn assert_drained(&self) {
        assert!(self.requests.lock().unwrap().is_empty());
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(
    State(requests): State<Arc<Mutex<VecDeque<Expected>>>>,
    request: Request<Body>,
) -> Response {
    let expected = requests
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected HTTP request");
    assert_eq!(request.method().as_str(), expected.method);
    assert_eq!(request.uri().to_string(), expected.path);
    assert_eq!(
        request.headers().get(header::AUTHORIZATION).unwrap(),
        "Bearer sdk-fixture"
    );
    assert_eq!(
        request.headers().get(header::ACCEPT).unwrap(),
        "application/json"
    );
    if expected.method != "GET" {
        assert_eq!(
            request.headers().get("idempotency-key").unwrap(),
            "command-819"
        );
    } else {
        assert!(request.headers().get("idempotency-key").is_none());
    }
    if expected.body.is_some() {
        assert_eq!(
            request.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }
    let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let body = (!bytes.is_empty()).then(|| serde_json::from_slice::<Value>(&bytes).unwrap());
    assert_eq!(body, expected.body);
    match expected.response {
        Some(body) => (expected.status, axum::Json(body)).into_response(),
        None => expected.status.into_response(),
    }
}

pub fn key() -> IdempotencyKey {
    IdempotencyKey::new("command-819").unwrap()
}
