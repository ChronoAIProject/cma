#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "relay/signer.rs"]
pub mod signer;
#[path = "tools/support.rs"]
pub mod support;

use cmaeg::telemetry::{self, timing::IngressTiming};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn tool_request_ids_timings_and_info_spans_exclude_secrets() {
    let metrics = telemetry::install().unwrap();
    let capture = Capture(Default::default());
    let writer = capture.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .without_time()
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let app = support::App::new().await;
    let reply_token = app
        .event_ref
        .open(&app.state.keys, relay_support::now())
        .unwrap()
        .payload
        .reply;
    let cmaeg_core::seal::ReplyAddress::NyxidRelay { reply_token, .. } = reply_token else {
        panic!()
    };
    let private_text = format!("private-message-{}", uuid::Uuid::new_v4());
    let response = app
        .fixture
        .client
        .post(app.event_url("replies"))
        .bearer_auth(&app.key)
        .header("x-request-id", "trace-request-1")
        .json(&serde_json::json!({"text":private_text}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-request-id"], "trace-request-1");
    let server_timing = response.headers()["server-timing"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        response.headers()["server-timing"]
            .to_str()
            .unwrap()
            .strip_prefix("deliver;dur=")
            .unwrap()
            .parse::<f64>()
            .unwrap()
            >= 0.0
    );
    let response = app
        .fixture
        .client
        .get(app.event_url("context"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let id = uuid::Uuid::parse_str(response.headers()["x-request-id"].to_str().unwrap()).unwrap();
    assert_eq!(id.get_version_num(), 4);
    assert!(response.headers().contains_key("server-timing"));
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    for secret in [
        &app.key,
        app.event_ref.as_str(),
        reply_token.as_str(),
        &private_text,
    ] {
        assert!(
            !logs.contains(secret),
            "sensitive field appeared in info logs"
        );
    }
    assert!(logs.contains("cmaeg.request"));
    assert!(logs.contains("cmaeg.deliver"));
    assert!(logs.contains("trace-request-1"));
    assert!(logs.contains("/v1/events/{event_ref}/replies"));
    assert!(logs.lines().any(|line| line.contains("trace-request-1")
        && line.contains("request finished")
        && line.contains(&server_timing)));
    let text = metrics.render();
    assert!(
        text.lines()
            .any(|line| line.starts_with("cmaeg_tool_call_seconds_count{")
                && line.contains("replyToEvent")
                && line.ends_with(" 1")),
        "{text}"
    );
}

#[tokio::test]
async fn only_the_restore_initiator_is_cold_and_refusals_have_timings() {
    let fixture = relay_support::Fixture::new(|_| {}, Default::default()).await;
    let observations: Vec<_> = (0..32).map(|_| IngressTiming::new("nyxid_relay")).collect();
    let results = futures_util::future::join_all(observations.iter().map(|timing| {
        fixture
            .state
            .resolver
            .resolve_observed(fixture.id.as_str(), Some(timing))
    }))
    .await;
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(
        observations
            .iter()
            .filter(|timing| timing.is_cold())
            .count(),
        1
    );
    let response = fixture
        .client
        .post(&fixture.url)
        .header("x-request-id", "invalid-request-id")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 415);
    assert_eq!(response.headers()["x-request-id"], "invalid-request-id");
    let header = response.headers()["server-timing"].to_str().unwrap();
    assert!(header.contains("launch;dur=0.000;desc=\"not_written\""));
    for field in ["restore;dur=", "verify;dur=", "total;dur="] {
        assert!(header.contains(field));
    }
}
