use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, Response, StatusCode},
    routing::any,
};
use cma_sdk::Client;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{net::TcpListener, task::JoinHandle};

#[derive(Default)]
pub struct Expected {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub raw_body: Option<Vec<u8>>,
    pub status: u16,
    pub response_headers: BTreeMap<String, String>,
    pub response: Vec<u8>,
    pub chunks: Option<Vec<Vec<u8>>>,
}

pub struct WireServer {
    pub client: Client,
    pub anonymous: Client,
    pub base_url: String,
    requests: Arc<Mutex<VecDeque<Expected>>>,
    task: JoinHandle<()>,
    observed: Arc<AtomicUsize>,
}

impl WireServer {
    pub async fn new() -> Self {
        let requests = Arc::new(Mutex::new(VecDeque::new()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/gateway", listener.local_addr().unwrap());
        let observed = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .fallback(any(handle))
            .with_state((requests.clone(), observed.clone()));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = Client::builder(&base_url)
            .unwrap()
            .bearer_token("sdk-fixture")
            .unwrap()
            .build()
            .unwrap();
        let anonymous = Client::builder(&base_url).unwrap().build().unwrap();
        Self {
            client,
            anonymous,
            base_url,
            requests,
            task,
            observed,
        }
    }

    pub fn expect(&self, expected: Expected) {
        self.requests.lock().unwrap().push_back(expected);
    }
    pub fn assert_drained(&self) {
        assert!(
            self.requests.lock().unwrap().is_empty(),
            "unconsumed wire expectations"
        );
    }

    pub fn request_count(&self) -> usize {
        self.observed.load(Ordering::SeqCst)
    }
}

impl Drop for WireServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type WireState = (Arc<Mutex<VecDeque<Expected>>>, Arc<AtomicUsize>);

async fn handle(
    State((queue, observed)): State<WireState>,
    request: Request<Body>,
) -> Response<Body> {
    observed.fetch_add(1, Ordering::SeqCst);
    let expected = queue
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected HTTP request");
    assert_eq!(request.method().as_str(), expected.method);
    let actual_url = reqwest::Url::parse(&format!("http://fixture{}", request.uri())).unwrap();
    let expected_url =
        reqwest::Url::parse(&format!("http://fixture/gateway{}", expected.path)).unwrap();
    assert_eq!(actual_url.path(), expected_url.path());
    let query = |url: &reqwest::Url| {
        url.query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(query(&actual_url), query(&expected_url));
    for name in [
        "authorization",
        "accept",
        "idempotency-key",
        "content-type",
        "if-none-match",
        "last-event-id",
        "prefer",
        "x-cmaeg-owner",
        "x-cmaeg-agent-authorization",
    ] {
        assert_eq!(
            request
                .headers()
                .get(name)
                .map(|value| value.to_str().unwrap()),
            expected.headers.get(name).map(String::as_str),
            "request header {name}"
        );
    }
    let bytes = to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    if let Some(raw) = expected.raw_body {
        assert_eq!(bytes.as_ref(), raw);
    } else {
        let actual = (!bytes.is_empty()).then(|| serde_json::from_slice::<Value>(&bytes).unwrap());
        assert_eq!(actual, expected.body);
    }
    let mut response = Response::builder().status(StatusCode::from_u16(expected.status).unwrap());
    for (name, value) in expected.response_headers {
        response = response.header(name, value);
    }
    let body = if let Some(chunks) = expected.chunks {
        Body::from_stream(futures_util::stream::iter(
            chunks.into_iter().map(Ok::<_, std::convert::Infallible>),
        ))
    } else {
        Body::from(expected.response)
    };
    response.body(body).unwrap()
}

pub fn headers(authenticated: bool, accept: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::from([("accept".into(), accept.into())]);
    if authenticated {
        result.insert("authorization".into(), "Bearer sdk-fixture".into());
    }
    result
}

#[allow(dead_code)]
pub fn json(
    method: &str,
    path: &str,
    body: Option<Value>,
    status: u16,
    response: Value,
    authenticated: bool,
) -> Expected {
    let mut request_headers = headers(authenticated, "application/json");
    if body.is_some() {
        request_headers.insert("content-type".into(), "application/json".into());
    }
    Expected {
        method: method.into(),
        path: path.into(),
        headers: request_headers,
        body,
        status,
        response_headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        response: serde_json::to_vec(&response).unwrap(),
        chunks: None,
        raw_body: None,
    }
}
