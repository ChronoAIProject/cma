use super::*;
use tokio::task::{JoinHandle, JoinSet};

pub(super) struct Exchange {
    pub request: axum::http::Request<Bytes>,
    pub reply: h2::server::SendResponse<Bytes>,
}

pub(super) struct H2Peer {
    origin: String,
    inbox: mpsc::Receiver<Exchange>,
    streams: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    shutdown: Option<oneshot::Sender<()>>,
    server: JoinHandle<()>,
}

impl H2Peer {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let streams = Arc::new(AtomicUsize::new(0));
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = connections.clone();
        let seen = streams.clone();
        let (sender, inbox) = mpsc::channel(8);
        let (shutdown, mut stop) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut stop => break,
                    completed = tasks.join_next(), if !tasks.is_empty() => {
                        completed.unwrap().unwrap();
                    }
                    connection = listener.accept() => {
                        let (socket, _) = connection.unwrap();
                        accepted.fetch_add(1, Ordering::SeqCst);
                        tasks.spawn(serve(socket, sender.clone(), seen.clone()));
                    }
                }
            }
            tasks.shutdown().await;
        });
        Self {
            origin,
            inbox,
            streams,
            connections,
            shutdown: Some(shutdown),
            server,
        }
    }

    pub fn router(
        &self,
        credential: CredentialHolder,
        builder: reqwest::ClientBuilder,
        model_credit_required: bool,
    ) -> Router {
        // Cleartext prior knowledge selects real H2 framing; all proxy policy is shared.
        let state = ProxyState {
            target: TargetBase::parse(&format!("{}/api/v1/proxy/s/chrono-llm-public", self.origin))
                .unwrap(),
            credential,
            client: proxy_client(builder.http2_prior_knowledge()),
            github_client: None,
            allowed_host: None,
            model_proxy: true,
            model_credit_required,
        };
        Router::new().fallback(any(forward)).with_state(state)
    }

    pub async fn next(&mut self) -> Exchange {
        tokio::time::timeout(Duration::from_secs(2), self.inbox.recv())
            .await
            .expect("H2 stream arrives")
            .unwrap()
    }

    pub fn stream_count(&self) -> usize {
        self.streams.load(Ordering::SeqCst)
    }

    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), &mut self.server)
            .await
            .expect("H2 peer shuts down")
            .unwrap();
    }
}

impl Drop for H2Peer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(
    socket: tokio::net::TcpStream,
    sender: mpsc::Sender<Exchange>,
    count: Arc<AtomicUsize>,
) {
    let mut connection = h2::server::handshake(socket).await.unwrap();
    let mut streams = JoinSet::new();
    loop {
        // Continue polling the connection while handlers drain bodies or tests send resets.
        tokio::select! {
            completed = streams.join_next(), if !streams.is_empty() => {
                completed.unwrap().unwrap();
            }
            stream = connection.accept() => {
                let Some(stream) = stream else { break };
                let Ok((request, reply)) = stream else { break };
                count.fetch_add(1, Ordering::SeqCst);
                let sender = sender.clone();
                streams.spawn(async move {
                    let (parts, mut body) = request.into_parts();
                    let mut bytes = Vec::new();
                    while let Some(chunk) = body.data().await {
                        let chunk = chunk.unwrap();
                        assert!(bytes.len() + chunk.len() <= 4096);
                        bytes.extend_from_slice(&chunk);
                        body.flow_control().release_capacity(chunk.len()).unwrap();
                    }
                    sender.send(Exchange {
                        request: axum::http::Request::from_parts(parts, Bytes::from(bytes)),
                        reply,
                    }).await.unwrap();
                });
            }
        }
    }
    streams.shutdown().await;
}

pub(super) async fn credential() -> CredentialHolder {
    let credential = CredentialHolder::new();
    credential
        .install(&CredentialFrame {
            operation_id: "h2-contract-install".into(),
            value: RuntimeCredential::new("h2-scoped-fixture"),
            custody_generation: Some(1),
            github_route: None,
        })
        .await
        .unwrap();
    credential
}

pub(super) fn send(app: Router) -> JoinHandle<Response> {
    tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses?continuation=1")
                .header(header::AUTHORIZATION, "Bearer caller-fixture")
                .header("open-sandbox-api-key", "caller-provider-fixture")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(PAYLOAD))
                .unwrap(),
        )
        .await
        .unwrap()
    })
}

pub(super) const PAYLOAD: &str =
    r#"{ "previous_response_id":"resp_fixture", "input":"continued" }"#;

pub(super) fn assert_request(request: &axum::http::Request<Bytes>) {
    assert_eq!(request.version(), axum::http::Version::HTTP_2);
    assert_eq!(request.method(), "POST");
    assert_eq!(
        request.uri().path_and_query().unwrap().as_str(),
        "/api/v1/proxy/s/chrono-llm-public/v1/responses?continuation=1"
    );
    assert_eq!(
        request.headers()[header::AUTHORIZATION],
        "Bearer h2-scoped-fixture"
    );
    assert!(!request.headers().contains_key("open-sandbox-api-key"));
    assert_eq!(request.body(), PAYLOAD);
}
