use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

use super::*;
use crate::sandbox::SandboxClient;

fn frame(uptime_ms: u64) -> supervisor::ResourceFrame {
    supervisor::ResourceFrame {
        uptime_ms,
        cpu_usage_usec: Some(2_000_000),
        cpu_millis_limit: Some(1_000),
        memory_bytes_used: Some(400),
        memory_bytes_limit: Some(1_000),
        memory_bytes_peak: Some(700),
        stall_permille: Some(15),
        ceiling_hits: Some(3),
        allocation_failures: Some(1),
        kills: Some(1),
        received_bytes: Some(8_000),
        sent_bytes: Some(4_000),
    }
}

#[test]
fn a_first_frame_carries_levels_and_no_rates() {
    let now = frame(2_000);
    let sample = derive_sample(None, &now);
    assert_eq!(sample.cpu_millis_used, None);
    assert_eq!(sample.cpu_millis_limit, now.cpu_millis_limit);
    assert_eq!(sample.memory_bytes_used, now.memory_bytes_used);
    assert_eq!(sample.memory_bytes_limit, now.memory_bytes_limit);
    assert_eq!(sample.memory_bytes_peak, now.memory_bytes_peak);
    assert_eq!(sample.stall_permille, now.stall_permille);
    assert_eq!(sample.ceiling_hits, now.ceiling_hits);
    assert_eq!(sample.allocation_failures, now.allocation_failures);
    assert_eq!(sample.kills, now.kills);
    assert_eq!(sample.received_bytes, now.received_bytes);
    assert_eq!(sample.sent_bytes, now.sent_bytes);
    assert_eq!(sample.received_bytes_per_second, None);
    assert_eq!(sample.sent_bytes_per_second, None);
    assert_eq!(sample.interval_ms, None);
}

#[test]
fn rates_are_derived_over_the_frames_own_interval() {
    let previous = frame(1_000);
    let mut now = frame(3_000);
    now.cpu_usage_usec = Some(previous.cpu_usage_usec.expect("cpu") + 1_000_000);
    now.received_bytes = Some(previous.received_bytes.expect("received") + 4_000);
    now.sent_bytes = Some(previous.sent_bytes.expect("sent") + 6_000);

    let sample = derive_sample(Some(&previous), &now);
    assert_eq!(sample.cpu_millis_used, Some(500));
    assert_eq!(sample.received_bytes_per_second, Some(2_000));
    assert_eq!(sample.sent_bytes_per_second, Some(3_000));
    assert_eq!(sample.interval_ms, Some(2_000));
}

#[test]
fn one_missing_counter_suppresses_only_its_rate() {
    let previous = frame(1_000);
    let mut now = frame(3_000);
    now.cpu_usage_usec = None;
    now.received_bytes = Some(previous.received_bytes.expect("received") + 2_000);
    now.sent_bytes = None;

    let sample = derive_sample(Some(&previous), &now);
    assert_eq!(sample.cpu_millis_used, None);
    assert_eq!(sample.received_bytes_per_second, Some(1_000));
    assert_eq!(sample.sent_bytes_per_second, None);
    assert_eq!(sample.interval_ms, Some(2_000));

    let mut next = frame(5_000);
    next.cpu_usage_usec = Some(previous.cpu_usage_usec.expect("cpu") + 2_000_000);
    next.received_bytes = None;
    next.sent_bytes = None;
    let sample = derive_sample(Some(&previous), &next);
    assert_eq!(sample.cpu_millis_used, Some(500));
    assert_eq!(sample.received_bytes_per_second, None);
    assert_eq!(sample.sent_bytes_per_second, None);
    assert_eq!(sample.interval_ms, Some(4_000));
}

#[test]
fn a_counter_that_went_backwards_does_not_poison_sound_rates() {
    let previous = frame(1_000);
    let mut now = frame(2_000);
    now.cpu_usage_usec = Some(1);
    now.received_bytes = Some(previous.received_bytes.expect("received") + 2_000);
    now.sent_bytes = Some(1);

    let sample = derive_sample(Some(&previous), &now);
    assert_eq!(sample.cpu_millis_used, None);
    assert_eq!(sample.received_bytes_per_second, Some(2_000));
    assert_eq!(sample.sent_bytes_per_second, None);
    assert_eq!(sample.interval_ms, Some(1_000));
}

#[test]
fn a_non_forward_clock_yields_no_interval_or_rates() {
    let previous = frame(2_000);
    for uptime_ms in [2_000, 1_999] {
        let sample = derive_sample(Some(&previous), &frame(uptime_ms));
        assert_eq!(sample.cpu_millis_used, None);
        assert_eq!(sample.received_bytes_per_second, None);
        assert_eq!(sample.sent_bytes_per_second, None);
        assert_eq!(sample.interval_ms, None);
    }
}

#[test]
fn unreadable_levels_and_absent_limits_stay_absent() {
    let previous = frame(1_000);
    let mut now = frame(2_000);
    now.cpu_millis_limit = None;
    now.memory_bytes_used = None;
    now.memory_bytes_limit = None;
    now.memory_bytes_peak = None;

    let sample = derive_sample(Some(&previous), &now);
    assert_eq!(sample.cpu_millis_limit, None);
    assert_eq!(sample.memory_bytes_used, None);
    assert_eq!(sample.memory_bytes_limit, None);
    assert_eq!(sample.memory_bytes_peak, None);
}

#[test]
fn rate_overflow_saturates_instead_of_wrapping() {
    let mut previous = frame(1);
    previous.cpu_usage_usec = Some(0);
    previous.received_bytes = Some(0);
    previous.sent_bytes = Some(0);
    let mut now = frame(2);
    now.cpu_usage_usec = Some(u64::MAX);
    now.received_bytes = Some(u64::MAX);
    now.sent_bytes = Some(u64::MAX);

    let sample = derive_sample(Some(&previous), &now);
    assert_eq!(sample.cpu_millis_used, Some(u32::MAX));
    assert_eq!(sample.received_bytes_per_second, Some(u64::MAX));
    assert_eq!(sample.sent_bytes_per_second, Some(u64::MAX));
}

enum LinkScript {
    NoUpgrade,
    Messages(Vec<Message>),
}

struct Observation {
    readiness_request: String,
    link_uri: Option<String>,
    disconnected: bool,
}

struct Fixture {
    provider_base: String,
    provider: tokio::task::JoinHandle<String>,
    runtime: tokio::task::JoinHandle<Observation>,
}

impl Fixture {
    async fn finish(mut self) -> (String, Observation) {
        let provider = tokio::time::timeout(Duration::from_secs(2), &mut self.provider)
            .await
            .expect("the endpoint was resolved once")
            .expect("the provider fixture finished");
        let runtime = tokio::time::timeout(Duration::from_secs(2), &mut self.runtime)
            .await
            .expect("the runtime fixture finished")
            .expect("the runtime fixture did not panic");
        (provider, runtime)
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut chunk = [0_u8; 1_024];
        let read = stream.read(&mut chunk).await.expect("the request reads");
        assert!(read > 0, "the request contains complete headers");
        request.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(request).expect("HTTP request text")
}

async fn respond(stream: &mut tokio::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("the response writes");
    stream.shutdown().await.expect("the response closes");
}

async fn fixture(readiness: serde_json::Value, script: LinkScript) -> Fixture {
    let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the runtime fixture binds");
    let runtime_address = runtime_listener.local_addr().expect("a runtime address");
    let runtime = tokio::spawn(async move {
        let (mut stream, _) = runtime_listener.accept().await.expect("readiness connects");
        let readiness_request = read_request(&mut stream).await;
        respond(&mut stream, &readiness.to_string()).await;

        match script {
            LinkScript::NoUpgrade => {
                let upgraded =
                    tokio::time::timeout(Duration::from_millis(150), runtime_listener.accept())
                        .await
                        .is_ok();
                Observation {
                    readiness_request,
                    link_uri: None,
                    disconnected: !upgraded,
                }
            }
            LinkScript::Messages(messages) => {
                let (stream, _) = runtime_listener.accept().await.expect("the link connects");
                let uri = Arc::new(Mutex::new(None));
                let observed = Arc::clone(&uri);
                // Tungstenite fixes this callback's error to a full HTTP
                // response. Its size is not a choice this fixture can narrow.
                #[allow(clippy::result_large_err)]
                let callback = move |request: &Request, response: Response| {
                    *observed.lock().expect("the uri lock") = Some(request.uri().to_string());
                    Ok(response)
                };
                let mut socket = tokio_tungstenite::accept_hdr_async(stream, callback)
                    .await
                    .expect("the websocket upgrades");
                for message in messages {
                    if socket.send(message).await.is_err() {
                        break;
                    }
                }
                let disconnected = tokio::time::timeout(Duration::from_secs(1), async {
                    while let Some(message) = socket.next().await {
                        if message.is_err() || matches!(message, Ok(Message::Close(_))) {
                            break;
                        }
                    }
                })
                .await
                .is_ok();
                let link_uri = uri.lock().expect("the uri lock").clone();
                Observation {
                    readiness_request,
                    link_uri,
                    disconnected,
                }
            }
        }
    });

    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the provider fixture binds");
    let provider_address = provider_listener.local_addr().expect("a provider address");
    let provider = tokio::spawn(async move {
        let (mut stream, _) = provider_listener.accept().await.expect("endpoint resolves");
        let request = read_request(&mut stream).await;
        let body = serde_json::json!({ "endpoint": runtime_address.to_string() }).to_string();
        respond(&mut stream, &body).await;
        request
    });

    Fixture {
        provider_base: format!("http://{provider_address}"),
        provider,
        runtime,
    }
}

fn hosted(base: &str, telemetry_interval_ms: Option<u64>) -> HostedKernel {
    let shape = super::super::SandboxShape {
        telemetry_interval_ms,
        ..super::super::SandboxShape::default()
    };
    HostedKernel::new(
        SandboxClient::new(base, "test-key"),
        shape,
        "test-kernel",
        ".kernel/skills",
        "",
    )
}

fn declaration() -> Message {
    Message::Text(
        serde_json::json!({
            "t": "ready",
            "wireVersion": supervisor::WIRE_VERSION,
            "lanes": [supervisor::LANE_NAME_TELEMETRY]
        })
        .to_string(),
    )
}

fn reading(frame: &supervisor::ResourceFrame) -> Message {
    let payload = serde_json::to_vec(&serde_json::json!({
        "uptimeMs": frame.uptime_ms,
        "cpuUsageUsec": frame.cpu_usage_usec,
        "cpuMillisLimit": frame.cpu_millis_limit,
        "memoryBytesUsed": frame.memory_bytes_used,
        "memoryBytesLimit": frame.memory_bytes_limit,
        "memoryBytesPeak": frame.memory_bytes_peak,
        "stallPermille": frame.stall_permille,
        "ceilingHits": frame.ceiling_hits,
        "allocationFailures": frame.allocation_failures,
        "kills": frame.kills,
        "receivedBytes": frame.received_bytes,
        "sentBytes": frame.sent_bytes,
    }))
    .expect("the reading serializes");
    Message::Binary(supervisor::frame(supervisor::LANE_TELEMETRY, &payload))
}

#[tokio::test]
async fn a_deployment_with_no_cadence_opens_nothing() {
    let kernel = hosted("http://127.0.0.1:1", None);
    let error = kernel
        .watch_resources(&RuntimeHandle("sbx-1".into()), mpsc::channel(1).0)
        .await
        .expect_err("an absent cadence is an absent watch");
    assert!(matches!(error, AdapterError::Unsupported(_)));
}

#[tokio::test]
async fn readiness_without_resources_is_refused_before_an_upgrade() {
    let fixture = fixture(
        serde_json::json!({ "status": "ready" }),
        LinkScript::NoUpgrade,
    )
    .await;
    let kernel = hosted(&fixture.provider_base, Some(2_000));
    let error = kernel
        .watch_resources(&RuntimeHandle("sbx-1".into()), mpsc::channel(1).0)
        .await
        .expect_err("an older pod cannot serve this link");
    assert!(matches!(error, AdapterError::Unsupported(_)));

    let (provider, runtime) = fixture.finish().await;
    assert!(provider.starts_with("GET /v1/sandboxes/sbx-1/endpoints/9090?"));
    assert!(runtime.readiness_request.starts_with("GET /readyz "));
    assert!(runtime.disconnected, "no websocket upgrade was attempted");
}

#[tokio::test]
async fn a_watch_uses_only_its_query_and_skips_frames_it_cannot_read() {
    let first = frame(1_000);
    let mut second = frame(3_000);
    second.cpu_usage_usec = Some(first.cpu_usage_usec.expect("cpu") + 1_000_000);
    second.received_bytes = Some(first.received_bytes.expect("received") + 4_000);
    second.sent_bytes = Some(first.sent_bytes.expect("sent") + 6_000);
    let mut third = second.clone();
    third.uptime_ms = 5_000;
    let mut fourth = third.clone();
    fourth.uptime_ms = 7_000;
    let messages = vec![
        declaration(),
        Message::Binary(supervisor::frame(supervisor::LANE_TERMINAL, b"screen")),
        Message::Binary(supervisor::frame(supervisor::LANE_TELEMETRY, b"not-json")),
        reading(&first),
        reading(&second),
        reading(&third),
        reading(&fourth),
    ];
    let fixture = fixture(
        serde_json::json!({ "resources": true }),
        LinkScript::Messages(messages),
    )
    .await;
    let kernel = hosted(&fixture.provider_base, Some(2_000));
    let (tx, mut rx) = mpsc::channel(1);
    kernel
        .watch_resources(&RuntimeHandle("sbx-1".into()), tx)
        .await
        .expect("the declared watch opens");

    let no_control_link = kernel
        .refresh_credential(&RuntimeHandle("sbx-1".into()), "opaque-test-value")
        .await
        .expect_err("telemetry is never remembered as a control link");
    assert!(matches!(no_control_link, AdapterError::Unavailable(_)));

    let first_sample = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("the first reading arrives")
        .expect("the relay is open");
    let second_sample = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("the second reading arrives")
        .expect("the relay is open");
    assert_eq!(first_sample.interval_ms, None);
    assert_eq!(second_sample.cpu_millis_used, Some(500));
    assert_eq!(second_sample.received_bytes_per_second, Some(2_000));
    assert_eq!(second_sample.sent_bytes_per_second, Some(3_000));
    drop(rx);

    let (provider, runtime) = fixture.finish().await;
    assert!(provider.starts_with("GET /v1/sandboxes/sbx-1/endpoints/9090?"));
    let uri = runtime.link_uri.expect("the link path was recorded");
    assert_eq!(
        uri,
        format!(
            "/runtime/sbx-1/terminal?wire={}&telemetry=2000",
            supervisor::WIRE_VERSION
        )
    );
    for forbidden in ["lanes=", "cols=", "rows="] {
        assert!(!uri.contains(forbidden), "unexpected {forbidden} in {uri}");
    }
    assert!(
        runtime.disconnected,
        "dropping the receiver ended the relay"
    );
}

#[tokio::test]
async fn an_unframed_link_is_refused_instead_of_read_as_telemetry() {
    let fixture = fixture(
        serde_json::json!({ "resources": true }),
        LinkScript::Messages(vec![Message::Binary(b"old terminal output".to_vec())]),
    )
    .await;
    let kernel = hosted(&fixture.provider_base, Some(2_000));
    let error = kernel
        .watch_resources(&RuntimeHandle("sbx-1".into()), mpsc::channel(1).0)
        .await
        .expect_err("an unframed link has no telemetry discriminator");
    assert!(matches!(error, AdapterError::Unsupported(_)));

    let (_, runtime) = fixture.finish().await;
    assert!(runtime.link_uri.is_some());
    assert!(runtime.disconnected, "the refused socket was closed");
}
