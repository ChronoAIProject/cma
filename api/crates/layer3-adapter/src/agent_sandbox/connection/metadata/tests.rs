use super::*;
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, Api};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay: Duration,
}
fn token(value: &str) -> Reply {
    Reply {
        status: 200,
        headers: vec![("Metadata-Flavor", "Google".into())],
        body: json!({"access_token":value,"expires_in":3600,"token_type":"Bearer"}).to_string(),
        delay: Duration::ZERO,
    }
}
fn pods() -> Reply {
    Reply {
        status: 200,
        headers: vec![],
        body: json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[]}).to_string(),
        delay: Duration::ZERO,
    }
}
struct Endpoint {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Endpoint {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buffer[..read]);
                    if bytes.windows(4).any(|slice| slice == b"\r\n\r\n") {
                        break;
                    }
                }
                captured
                    .lock()
                    .await
                    .push(String::from_utf8(bytes).unwrap().to_ascii_lowercase());
                if !reply.delay.is_zero() {
                    tokio::time::sleep(reply.delay).await;
                }
                let headers = reply
                    .headers
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}\r\n"))
                    .collect::<String>();
                let response = format!("HTTP/1.1 {} Fixture\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", reply.status, reply.body.len(), reply.body);
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self {
            origin,
            requests,
            task,
        }
    }
    fn tokens(&self) -> Arc<MetadataTokens> {
        Arc::new(
            MetadataTokens::new(&format!("{}/token", self.origin), Duration::from_secs(1)).unwrap(),
        )
    }
    async fn finish(&mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .unwrap()
            .unwrap();
        self.requests.lock().await.clone()
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn client(endpoint: &Endpoint, tokens: Arc<MetadataTokens>) -> kube::Client {
    let config = kube::Config::new(endpoint.origin.parse().unwrap());
    let layer = MetadataLayer {
        target: config.cluster_url.clone(),
        tokens,
    };
    transport::client(config, &layer).unwrap()
}
async fn expire(tokens: &MetadataTokens) {
    tokens.cache.lock().await.as_mut().unwrap().refresh_at =
        Instant::now() - Duration::from_secs(1);
}

#[tokio::test]
async fn one_long_lived_kubernetes_client_refreshes_metadata_token_before_expiry() {
    let mut metadata = Endpoint::new(vec![token("first-secret"), token("second-secret")]).await;
    let tokens = metadata.tokens();
    let mut cluster = Endpoint::new(vec![pods(), pods(), pods()]).await;
    let api: Api<Pod> = Api::namespaced(client(&cluster, tokens.clone()), "example-registry-01");
    for _ in 0..2 {
        api.list(&ListParams::default()).await.unwrap();
    }
    assert_eq!(metadata.requests.lock().await.len(), 1);
    expire(&tokens).await;
    api.list(&ListParams::default()).await.unwrap();
    let requests = cluster.finish().await;
    assert!(requests[..2]
        .iter()
        .all(|request| request.contains("authorization: bearer first-secret")));
    assert!(requests[2].contains("authorization: bearer second-secret"));
    assert!(requests
        .iter()
        .all(|request| request.contains("/api/v1/namespaces/example-registry-01/pods")));
    let fetches = metadata.finish().await;
    assert_eq!(fetches.len(), 2);
    assert!(fetches
        .iter()
        .all(|request| request.contains("metadata-flavor: google")
            && !request.contains("authorization:")));
    assert!(tokens.authorization().await.unwrap().is_sensitive());
}

#[tokio::test]
async fn concurrent_requests_share_one_token_fetch_and_failed_refresh_never_sends_stale_token() {
    let mut failed = token("do-not-expose");
    failed.status = 503;
    let mut metadata = Endpoint::new(vec![token("cached-secret"), failed]).await;
    let tokens = metadata.tokens();
    let results = futures_util::future::join_all((0..16).map(|_| tokens.authorization())).await;
    assert!(results.into_iter().all(|result| result.is_ok()));
    assert_eq!(metadata.requests.lock().await.len(), 1);
    let cluster = Endpoint::new(vec![pods()]).await;
    let client = client(&cluster, tokens.clone());
    expire(&tokens).await;
    let api: Api<Pod> = Api::namespaced(client, "example-registry-01");
    let error = api
        .list(&ListParams::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("cached-secret") && !error.contains("do-not-expose"));
    assert!(cluster.requests.lock().await.is_empty());
    assert!(tokens.cache.lock().await.is_none());
    assert_eq!(metadata.finish().await.len(), 2);
}

#[tokio::test]
async fn malformed_oversized_short_lived_and_untrusted_metadata_responses_are_redacted() {
    let mut missing_flavor = token("secret");
    missing_flavor.headers.clear();
    let mut huge = token("secret");
    huge.body = "s".repeat(RESPONSE_LIMIT + 1);
    let mut invalid_json = token("secret");
    invalid_json.body = "secret-not-json".into();
    let mut short = token("secret");
    short.body = json!({"access_token":"secret","expires_in":60,"token_type":"Bearer"}).to_string();
    let mut newline = token("secret");
    newline.body =
        json!({"access_token":"secret\r\nInjected:value","expires_in":3600,"token_type":"Bearer"})
            .to_string();
    let mut wrong_type = token("secret");
    wrong_type.body =
        json!({"access_token":"secret","expires_in":3600,"token_type":"Basic"}).to_string();
    for reply in [
        missing_flavor,
        huge,
        invalid_json,
        short,
        newline,
        wrong_type,
    ] {
        let mut metadata = Endpoint::new(vec![reply]).await;
        let tokens = metadata.tokens();
        let error = tokens.authorization().await.unwrap_err().to_string();
        assert_eq!(error, token_unavailable().to_string());
        assert!(!error.contains(&metadata.origin) && !error.contains("secret"));
        assert_eq!(metadata.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn metadata_request_timeout_is_bounded_and_redirects_are_never_followed() {
    let mut slow = token("secret");
    slow.delay = Duration::from_millis(350);
    let mut metadata = Endpoint::new(vec![slow]).await;
    let tokens = MetadataTokens::new(&metadata.origin, Duration::from_millis(100)).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), tokens.authorization())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(metadata.finish().await.len(), 1);
    let redirected = Endpoint::new(vec![token("foreign-secret")]).await;
    let mut redirect = token("secret");
    redirect.status = 302;
    redirect
        .headers
        .push(("Location", redirected.origin.clone()));
    let mut metadata = Endpoint::new(vec![redirect]).await;
    assert!(metadata.tokens().authorization().await.is_err());
    assert_eq!(metadata.finish().await.len(), 1);
    assert!(redirected.requests.lock().await.is_empty());
}

#[tokio::test]
async fn foreign_request_uri_or_host_is_refused_before_token_fetch_or_cluster_io() {
    let metadata = Endpoint::new(vec![token("secret")]).await;
    let tokens = metadata.tokens();
    let cluster = Endpoint::new(vec![pods()]).await;
    let client = client(&cluster, tokens);
    for request in [
        Request::get("http://foreign.example/api/v1/pods")
            .body(Vec::new())
            .unwrap(),
        Request::get("https://foreign.example/api/v1/pods")
            .body(Vec::new())
            .unwrap(),
        Request::get("/api/v1/pods")
            .header("Host", "foreign.example")
            .body(Vec::new())
            .unwrap(),
    ] {
        let error = client.request_text(request).await.unwrap_err().to_string();
        assert!(error.contains("target changed"));
        assert!(!error.contains("secret"));
    }
    assert!(metadata.requests.lock().await.is_empty());
    assert!(cluster.requests.lock().await.is_empty());
}

#[tokio::test]
async fn kubernetes_redirect_cannot_forward_metadata_authorization_to_another_host() {
    let mut metadata = Endpoint::new(vec![token("secret")]).await;
    let redirected = Endpoint::new(vec![pods()]).await;
    let mut reply = pods();
    reply.status = 302;
    reply.headers.push(("Location", redirected.origin.clone()));
    let mut cluster = Endpoint::new(vec![reply]).await;
    let client = client(&cluster, metadata.tokens());
    let api: Api<Pod> = Api::namespaced(client, "example-registry-01");
    let _ = api.list(&ListParams::default()).await;
    assert_eq!(metadata.finish().await.len(), 1);
    assert!(cluster.finish().await[0].contains("authorization: bearer secret"));
    assert!(redirected.requests.lock().await.is_empty());
}

#[tokio::test]
async fn metadata_transport_preserves_authenticated_interactive_exec_and_status() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            handshake::server::{Request as UpgradeRequest, Response as UpgradeResponse},
            Message,
        },
    };
    let mut metadata = Endpoint::new(vec![token("stream-fixture")]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = kube::Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    let layer = MetadataLayer {
        target: config.cluster_url.clone(),
        tokens: metadata.tokens(),
    };
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        // The upstream handshake callback fixes its unboxed error type.
        #[allow(clippy::result_large_err)]
        let mut socket = accept_hdr_async(
            socket,
            |request: &UpgradeRequest, mut response: UpgradeResponse| {
                assert_eq!(request.headers()["authorization"], "Bearer stream-fixture");
                assert_eq!(
                    request.uri().path(),
                    "/api/v1/namespaces/example-registry-01/pods/fixture/exec"
                );
                response.headers_mut().insert(
                    "sec-websocket-protocol",
                    "v5.channel.k8s.io".parse().unwrap(),
                );
                Ok(response)
            },
        )
        .await
        .unwrap();
        for expected in 0..32_u8 {
            let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
                panic!("binary input required")
            };
            assert_eq!(bytes, [0, expected]);
            socket
                .send(Message::Binary(vec![1, expected]))
                .await
                .unwrap();
        }
        socket
            .send(Message::Binary(
                [vec![3], br#"{"status":"Success"}"#.to_vec()].concat(),
            ))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
    });
    let api: Api<Pod> = Api::namespaced(transport::client(config, &layer).unwrap(), "example-registry-01");
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut process = api
            .exec(
                "fixture",
                ["cat"],
                &kube::api::AttachParams::default()
                    .stdin(true)
                    .stdout(true)
                    .stderr(false),
            )
            .await
            .unwrap();
        let mut input = process.stdin().unwrap();
        let mut output = process.stdout().unwrap();
        let status = process.take_status().unwrap();
        for expected in 0..32_u8 {
            input.write_all(&[expected]).await.unwrap();
            let mut actual = [0];
            output.read_exact(&mut actual).await.unwrap();
            assert_eq!(actual, [expected]);
        }
        assert_eq!(status.await.unwrap().status.as_deref(), Some("Success"));
        server.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(metadata.finish().await.len(), 1);
}

#[tokio::test]
async fn metadata_transport_keeps_cluster_read_timeout() {
    let mut metadata = Endpoint::new(vec![token("timeout-fixture")]).await;
    let mut slow = pods();
    slow.delay = Duration::from_millis(350);
    let mut cluster = Endpoint::new(vec![slow]).await;
    let mut config = kube::Config::new(cluster.origin.parse().unwrap());
    config.read_timeout = Some(Duration::from_millis(100));
    let layer = MetadataLayer {
        target: config.cluster_url.clone(),
        tokens: metadata.tokens(),
    };
    let api: Api<Pod> = Api::namespaced(transport::client(config, &layer).unwrap(), "example-registry-01");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), api.list(&ListParams::default()))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(metadata.finish().await.len(), 1);
    assert_eq!(cluster.finish().await.len(), 1);
}
