use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

use super::*;
use crate::hosted::{HostedKernel, SandboxShape};
use crate::AdapterError;

struct FixtureResponse {
    status: u16,
    body: String,
    delay: Duration,
}

impl FixtureResponse {
    fn ok(body: String) -> Self {
        Self {
            status: 200,
            body,
            delay: Duration::ZERO,
        }
    }

    fn delayed_ok(body: String, delay: Duration) -> Self {
        Self {
            status: 200,
            body,
            delay,
        }
    }

    fn rejected(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }
}

struct Fixture {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn finish(mut self) -> Vec<String> {
        match tokio::time::timeout(Duration::from_secs(2), &mut self.server).await {
            Ok(joined) => joined.expect("fixture server must not panic"),
            Err(_) => {
                self.server.abort();
                panic!("fixture server did not receive every expected request");
            }
        }
        self.requests.lock().expect("requests lock").clone()
    }
}

async fn serve(responses: Vec<FixtureResponse>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture binds");
    let address = listener.local_addr().expect("fixture has an address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.expect("fixture accepts");
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).await.expect("fixture reads");
                assert!(read > 0, "client closed before sending request headers");
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            observed
                .lock()
                .expect("requests lock")
                .push(String::from_utf8(request).expect("request is HTTP text"));

            let reason = match response.status {
                200 => "OK",
                401 => "Unauthorized",
                503 => "Service Unavailable",
                status => panic!("fixture does not name HTTP status {status}"),
            };
            let reply = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                reason,
                response.body.len(),
                response.body
            );
            tokio::time::sleep(response.delay).await;
            stream
                .write_all(reply.as_bytes())
                .await
                .expect("fixture writes");
            stream.shutdown().await.expect("fixture closes");
        }
    });

    Fixture {
        base_url: format!("http://{address}"),
        requests,
        server,
    }
}

#[tokio::test]
async fn one_bounded_artifact_can_outlive_the_provider_default_timeout() {
    let fixture = serve(vec![FixtureResponse::delayed_ok(
        "complete checkpoint".into(),
        Duration::from_millis(75),
    )])
    .await;
    let client = SandboxClient::new_with_provider_timeout(
        &fixture.base_url,
        "test-key",
        Duration::from_millis(20),
    );

    let artifact = client
        .post_to_sandbox_with_bounded_response_until(
            &fixture.base_url,
            "/runtime/workspace/checkpoint",
            Vec::new(),
            1_024,
            Duration::from_secs(1),
        )
        .await
        .expect("the request-specific artifact deadline overrides the client default");

    assert_eq!(artifact, b"complete checkpoint");
    let requests = fixture.finish().await;
    assert_eq!(
        request_lines(&requests),
        ["POST /runtime/workspace/checkpoint HTTP/1.1"]
    );
}

fn page(
    ids: &[&str],
    page: u32,
    total_items: u64,
    total_pages: u32,
    has_next_page: bool,
) -> String {
    serde_json::json!({
        "items": ids.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>(),
        "pagination": {
            "page": page,
            "pageSize": LIST_PAGE_SIZE,
            "totalItems": total_items,
            "totalPages": total_pages,
            "hasNextPage": has_next_page
        }
    })
    .to_string()
}

fn pagination(has_next_page: bool) -> Pagination {
    Pagination {
        page: 1,
        page_size: LIST_PAGE_SIZE,
        total_items: 1,
        total_pages: 1,
        has_next_page,
    }
}

fn request_lines(requests: &[String]) -> Vec<&str> {
    requests
        .iter()
        .map(|request| request.lines().next().expect("request line"))
        .collect()
}

#[test]
fn a_listing_without_pagination_stops() {
    assert_eq!(next_step(None, 1), PageStep::Stop);
}

#[test]
fn a_listing_whose_envelope_says_it_ended_stops() {
    assert_eq!(next_step(Some(&pagination(false)), 1), PageStep::Stop);
}

#[test]
fn a_listing_below_the_bound_follows_the_next_page() {
    assert_eq!(
        next_step(Some(&pagination(true)), LIST_PAGE_LIMIT - 1),
        PageStep::Next(LIST_PAGE_LIMIT)
    );
}

#[test]
fn a_listing_at_the_bound_is_refused() {
    assert_eq!(
        next_step(Some(&pagination(true)), LIST_PAGE_LIMIT),
        PageStep::Refuse
    );
}

#[tokio::test]
async fn every_advertised_page_is_returned_in_order_and_requested_explicitly() {
    let fixture = serve(vec![
        FixtureResponse::ok(page(&["one", "two"], 1, 6, 3, true)),
        FixtureResponse::ok(page(&["three", "four"], 2, 6, 3, true)),
        FixtureResponse::ok(page(&["five", "six"], 3, 6, 3, false)),
    ])
    .await;
    let client = SandboxClient::new(&fixture.base_url, "test-key");

    let listed = client.list().await.expect("all pages list");
    assert_eq!(
        listed
            .iter()
            .map(|sandbox| sandbox.id.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three", "four", "five", "six"]
    );

    let requests = fixture.finish().await;
    assert_eq!(
        request_lines(&requests),
        [
            "GET /v1/sandboxes?page=1&pageSize=100 HTTP/1.1",
            "GET /v1/sandboxes?page=2&pageSize=100 HTTP/1.1",
            "GET /v1/sandboxes?page=3&pageSize=100 HTTP/1.1",
        ]
    );
    assert!(requests.iter().all(|request| {
        request
            .to_ascii_lowercase()
            .contains("open-sandbox-api-key: test-key")
    }));
}

#[tokio::test]
async fn a_bare_listing_remains_a_single_page() {
    let fixture = serve(vec![FixtureResponse::ok(
        serde_json::json!({ "items": [{ "id": "only" }] }).to_string(),
    )])
    .await;
    let client = SandboxClient::new(&fixture.base_url, "test-key");

    let listed = client.list().await.expect("bare listing works");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "only");

    let requests = fixture.finish().await;
    assert_eq!(
        request_lines(&requests),
        ["GET /v1/sandboxes?page=1&pageSize=100 HTTP/1.1"]
    );
}

#[tokio::test]
async fn an_empty_listing_is_a_complete_answer() {
    let fixture = serve(vec![FixtureResponse::ok(page(&[], 1, 0, 1, false))]).await;
    let client = SandboxClient::new(&fixture.base_url, "test-key");

    let listed = client.list().await.expect("empty fleet is not an error");
    assert!(listed.is_empty());

    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn a_listing_that_never_ends_is_refused_at_the_bound() {
    let responses = (0..LIST_PAGE_LIMIT)
        .map(|_| FixtureResponse::ok(page(&["still-here"], 1, 5_001, 51, true)))
        .collect();
    let fixture = serve(responses).await;
    let client = SandboxClient::new(&fixture.base_url, "test-key");

    let error = client.list().await.expect_err("endless listing must fail");
    assert!(matches!(
        error,
        SandboxError::Incomplete {
            pages: LIST_PAGE_LIMIT,
            page_size: LIST_PAGE_SIZE
        }
    ));

    let requests = fixture.finish().await;
    assert_eq!(requests.len(), LIST_PAGE_LIMIT as usize);
    assert_eq!(
        request_lines(&requests).last(),
        Some(&"GET /v1/sandboxes?page=50&pageSize=100 HTTP/1.1")
    );
}

#[tokio::test]
async fn a_failure_after_the_first_page_fails_the_whole_listing_with_context() {
    let body = format!(
        r#"{{"code":"PAGE_FAILED","message":"provider could not read page: {}"}}"#,
        "x".repeat(400)
    );
    let fixture = serve(vec![
        FixtureResponse::ok(page(&["one", "two"], 1, 4, 2, true)),
        FixtureResponse::rejected(503, body),
    ])
    .await;
    let client = SandboxClient::new(&fixture.base_url, "test-key");

    let error = client
        .list()
        .await
        .expect_err("a partial listing must never be returned");
    match error {
        SandboxError::Rejected { status, detail } => {
            assert_eq!(status, 503);
            assert!(detail.contains("page 2"), "got {detail}");
            assert!(detail.contains("PAGE_FAILED"), "got {detail}");
            assert!(
                detail.contains("provider could not read page"),
                "got {detail}"
            );
            assert!(detail.ends_with('…'), "body was not truncated: {detail}");
            assert!(detail.len() < 400, "truncated detail is too long");
        }
        other => panic!("expected provider rejection, got {other:?}"),
    }

    let requests = fixture.finish().await;
    assert_eq!(
        request_lines(&requests),
        [
            "GET /v1/sandboxes?page=1&pageSize=100 HTTP/1.1",
            "GET /v1/sandboxes?page=2&pageSize=100 HTTP/1.1",
        ]
    );
}

#[tokio::test]
async fn a_hosted_listing_maps_an_incomplete_provider_listing_to_unavailable() {
    let responses = (0..LIST_PAGE_LIMIT)
        .map(|_| FixtureResponse::ok(page(&["still-here"], 1, 5_001, 51, true)))
        .collect();
    let fixture = serve(responses).await;
    let kernel = HostedKernel::new(
        SandboxClient::new(&fixture.base_url, "test-key"),
        SandboxShape::default(),
        "test-kernel",
        ".test/skills",
        "AGENTS.md",
    );

    let error = kernel
        .list_runtimes()
        .await
        .expect_err("incomplete provider listing must not become a trusted inventory");
    assert!(
        matches!(&error, AdapterError::Unavailable(message) if message.contains("did not end after 50 pages of 100")),
        "got {error:?}"
    );

    let requests = fixture.finish().await;
    assert_eq!(requests.len(), LIST_PAGE_LIMIT as usize);
}

#[test]
fn base_url_loses_its_trailing_slash() {
    let c = SandboxClient::new("http://provider/", "k");
    assert_eq!(c.base_url(), "http://provider");
}

#[test]
fn a_sandbox_is_called_on_the_scheme_the_provider_was_reached_on() {
    // The provider runs in-cluster on plain HTTP. Forcing TLS here made
    // every sandbox call fail as "provider unreachable", which stopped
    // rotation from ever capturing a conversation.
    let c = SandboxClient::new("http://provider.svc.cluster.local", "k");
    assert_eq!(
        c.sandbox_url(
            "provider.svc.cluster.local/proxy/9090",
            "/runtime/conversation"
        ),
        "http://provider.svc.cluster.local/proxy/9090/runtime/conversation"
    );

    let c = SandboxClient::new("https://provider.example.com", "k");
    assert_eq!(
        c.sandbox_url("provider.example.com/proxy/9090", "/runtime/conversation"),
        "https://provider.example.com/proxy/9090/runtime/conversation"
    );
}

#[test]
fn an_endpoint_keeps_its_own_scheme_out_of_the_url() {
    // The provider may hand back the endpoint with a scheme already on it,
    // and it is not authoritative about how we reach it.
    let c = SandboxClient::new("http://provider", "k");
    for endpoint in ["https://host/proxy", "http://host/proxy", "host/proxy/"] {
        assert_eq!(c.sandbox_url(endpoint, "/p"), "http://host/proxy/p");
    }
}

#[test]
fn the_api_key_travels_as_a_header_not_a_url() {
    let c = SandboxClient::new("http://provider", "super-secret");
    let (name, value) = c.auth_header();
    assert_eq!(name, "OPEN-SANDBOX-API-KEY");
    assert_eq!(value, "super-secret");
    assert!(
        !c.base_url().contains("super-secret"),
        "the key must never be part of a URL"
    );
}

#[test]
fn a_running_sandbox_is_recognised_case_insensitively() {
    let s: Sandbox =
        serde_json::from_str(r#"{"id":"a","status":{"state":"Running","reason":"RUNNING"}}"#)
            .expect("parses");
    assert!(s.is_running());
}

#[test]
fn a_pending_sandbox_is_not_running() {
    let s: Sandbox =
        serde_json::from_str(r#"{"id":"a","status":{"state":"Pending"}}"#).expect("parses");
    assert!(!s.is_running());

    let no_status: Sandbox = serde_json::from_str(r#"{"id":"a"}"#).expect("parses");
    assert!(!no_status.is_running());
}

#[test]
fn unknown_response_fields_are_ignored() {
    // The provider adds fields over time; a new one must not break us.
    let s: Sandbox = serde_json::from_str(
        r#"{"id":"a","status":{"state":"Running","somethingNew":1},"expiresAt":"x"}"#,
    )
    .expect("forward-compatible");
    assert_eq!(s.id, "a");
}

#[test]
fn a_request_serializes_with_camel_case_keys() {
    let r = SandboxRequest {
        image: ImageSpec {
            uri: "example/image:1".into(),
        },
        entrypoint: vec!["/bin/run".into()],
        env: BTreeMap::from([("A".to_string(), "1".to_string())]),
        resource_limits: BTreeMap::from([("cpu".to_string(), "1".to_string())]),
        timeout: Some(3600),
        metadata: BTreeMap::new(),
    };
    let json = serde_json::to_value(&r).expect("serializes");
    assert!(json.get("resourceLimits").is_some(), "got {json}");
    assert_eq!(json["image"]["uri"], "example/image:1");
}

#[test]
fn an_absent_timeout_serializes_as_null_not_omitted() {
    let r = SandboxRequest {
        image: ImageSpec { uri: "x".into() },
        entrypoint: vec![],
        env: BTreeMap::new(),
        resource_limits: BTreeMap::new(),
        timeout: None,
        metadata: BTreeMap::new(),
    };
    let json = serde_json::to_value(&r).expect("serializes");
    assert!(
        json.get("timeout").is_some(),
        "timeout must be present as null, not omitted: {json}"
    );
    assert!(json["timeout"].is_null(), "got {json}");
}

#[test]
fn long_error_bodies_are_truncated() {
    let long = "x".repeat(1000);
    let t = truncate(&long);
    assert!(t.len() < 400, "got {} chars", t.len());
    assert!(t.ends_with('…'));
    assert_eq!(truncate("short"), "short");
}
