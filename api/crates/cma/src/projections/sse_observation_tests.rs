//! Exercise client abort attribution over HTTP against the public AG-UI replay route.
use super::{fixture_support::attach_identity, fixture_with_terminal, AgentId, AgentLogKind};
use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{header, HeaderValue},
    middleware::{from_fn, Next},
    response::Response,
    Router,
};
use futures_util::{stream, StreamExt};
use layer2_core::sandbox::store::AgentStore;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Notify, time::timeout};

const PATH: &str = "/api/v1/agui/events?thread_id=agt_composed";
const OBSERVATION: &str = "x-fixture-observation";
const DEADLINE: Duration = Duration::from_secs(5);
const STARTED: &str = r#""type":"RUN_STARTED""#;
const FINISHED: &str = r#""type":"RUN_FINISHED""#;

#[derive(Clone, Copy)]
enum Case {
    CompletedReplay,
    CancelledReplay,
    ClientAbort,
    BodyFailure,
}

impl Case {
    fn id(self) -> &'static str {
        match self {
            Self::CompletedReplay => "fixture-completed-replay",
            Self::CancelledReplay => "fixture-cancelled-replay",
            Self::ClientAbort => "fixture-deliberate-abort",
            Self::BodyFailure => "fixture-injected-failure",
        }
    }

    fn terminal(self) -> Option<AgentLogKind> {
        match self {
            Self::CompletedReplay => Some(AgentLogKind::TurnCompleted { turn_sequence: 3 }),
            Self::CancelledReplay => Some(AgentLogKind::TurnInterrupted { turn_sequence: 3 }),
            Self::ClientAbort | Self::BodyFailure => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AbortReceipt {
    observation_id: HeaderValue,
}

struct Consumer {
    response: reqwest::Response,
}

impl Consumer {
    async fn read_started(&mut self) -> String {
        let mut bytes = Vec::new();
        loop {
            let chunk = self
                .response
                .chunk()
                .await
                .unwrap()
                .expect("run start frame");
            bytes.extend_from_slice(&chunk);
            assert!(bytes.len() < 64 * 1024, "fixture replay exceeded its bound");
            if std::str::from_utf8(&bytes).is_ok_and(|text| text.contains(STARTED)) {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }

    fn abort(self) -> AbortReceipt {
        let observation_id = self.response.headers()[OBSERVATION].clone();
        drop(self.response);
        AbortReceipt { observation_id }
    }
}

struct BodyDropped(Arc<Notify>);

impl Drop for BodyDropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

async fn fixture_transport(
    mut request: Request,
    next: Next,
    body_dropped: Arc<Notify>,
    fail_body: Option<Arc<Notify>>,
) -> Response {
    let observation_id = request.headers()[OBSERVATION].clone();
    attach_identity(&mut request, "owner");
    let (mut parts, body) = next.run(request).await.into_parts();
    parts.headers.insert(OBSERVATION, observation_id);
    let dropped = BodyDropped(body_dropped);
    let data = body.into_data_stream().map(move |frame| {
        // Keep the signal alive until the observed body is released.
        let _ = &dropped;
        frame
    });
    let body = match fail_body {
        Some(fail) => Body::from_stream(stream::unfold(
            (data, Some(fail), false),
            |(mut data, fail, started)| async move {
                let fail = fail?;
                if started {
                    fail.notified().await;
                    let error = axum::Error::new(std::io::Error::other("fixture-body-failure"));
                    return Some((Err::<Bytes, _>(error), (data, None, true)));
                }
                let frame = data.next().await?;
                let started = frame.as_ref().is_ok_and(|bytes| {
                    std::str::from_utf8(bytes).is_ok_and(|s| s.contains(STARTED))
                });
                Some((frame, (data, Some(fail), started)))
            },
        )),
        None => Body::from_stream(data),
    };
    Response::from_parts(parts, body)
}

async fn open_observation(
    case: Case,
    app: Router,
    body_dropped: Arc<Notify>,
    fail_body: Option<Arc<Notify>>,
) -> (Consumer, tokio::task::JoinHandle<()>) {
    let app = app
        .layer(from_fn(move |request, next| {
            fixture_transport(request, next, body_dropped.clone(), fail_body.clone())
        }))
        .layer(from_fn(crate::telemetry::http::observe));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let follow = case.terminal().is_none();
    let response = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .timeout(DEADLINE)
        .build()
        .unwrap()
        .get(format!("http://{address}{PATH}&follow={follow}"))
        .header(OBSERVATION, case.id())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert_eq!(response.headers()[OBSERVATION], case.id());
    (Consumer { response }, server)
}

fn samples<'a>(text: &'a str, metric: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|line| line.starts_with(metric))
        .collect()
}

fn assert_counts(metrics: &PrometheusHandle, case: Case, reason: &str, disposition: &str) {
    let text = metrics.render();
    assert_eq!(
        samples(&text, "cma_sse_contract_opened_total{"),
        [
            r#"cma_sse_contract_opened_total{stream_kind="agui",consumer="unknown",protocol="agui"} 1"#
        ],
    );
    assert_eq!(
        samples(&text, "cma_sse_contract_closed_total{"),
        [format!(
            r#"cma_sse_contract_closed_total{{stream_kind="agui",consumer="unknown",protocol="agui",reason="{reason}"}} 1"#,
        )],
    );
    assert_eq!(
        samples(&text, "cma_sse_observation_closed_total{"),
        [format!(
            r#"cma_sse_observation_closed_total{{route="/api/v1/agui/events",reason="{disposition}"}} 1"#,
        )],
    );
    for fixture_value in [case.id(), "agt_composed", "fixture-body-failure"] {
        assert!(!text.contains(fixture_value));
    }
}

async fn qualify(case: Case) {
    // The server and consumer share this test's current-thread metric recorder.
    let recorder = PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let fixture = fixture_with_terminal(case.terminal()).await;
    let body_dropped = Arc::new(Notify::new());
    let fail_body = matches!(case, Case::BodyFailure).then(|| Arc::new(Notify::new()));
    let (mut consumer, server) = open_observation(
        case,
        fixture.router,
        body_dropped.clone(),
        fail_body.clone(),
    )
    .await;
    let started = consumer.read_started().await;
    let (abort_receipt, reason, disposition) = match case {
        Case::CompletedReplay | Case::CancelledReplay => {
            let rest = consumer.response.text().await.unwrap();
            let text = started + &rest;
            let terminal = text
                .lines()
                .find(|line| line.starts_with("data: ") && line.contains(FINISHED))
                .expect("retained terminal frame");
            let event: serde_json::Value = serde_json::from_str(&terminal[6..]).unwrap();
            let expected = match case {
                Case::CompletedReplay => "complete",
                _ => "interrupt",
            };
            assert_eq!(event["outcome"]["type"], expected);
            (None, "finite_replay", "eof")
        }
        Case::ClientAbort => {
            assert!(!started.contains(FINISHED));
            assert!(samples(&metrics.render(), "cma_sse_contract_closed_total{").is_empty());
            let receipt = consumer.abort();
            (Some(receipt), "drop_unknown", "dropped")
        }
        Case::BodyFailure => {
            assert!(!started.contains(FINISHED));
            assert!(samples(&metrics.render(), "cma_sse_contract_closed_total{").is_empty());
            fail_body.unwrap().notify_one();
            let error = consumer.response.chunk().await.unwrap_err();
            assert!(error.is_decode() && !error.is_timeout(), "{error:?}");
            (None, "server_error", "error")
        }
    };
    timeout(DEADLINE, body_dropped.notified())
        .await
        .expect("server released the SSE body");
    if matches!(case, Case::ClientAbort) {
        // Closing observation does not close the Agent's durable turn.
        let agent = fixture
            .store
            .get_agent(&AgentId("agt_composed".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(agent.conversation.active_turn, Some(3));
        assert_eq!(
            abort_receipt,
            Some(AbortReceipt {
                observation_id: HeaderValue::from_static(case.id())
            }),
        );
    } else {
        assert_eq!(abort_receipt, None);
    }
    assert_counts(&metrics, case, reason, disposition);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn sse_completed_turn_reaches_eof_without_a_client_abort() {
    qualify(Case::CompletedReplay).await;
}

#[tokio::test]
async fn sse_cancelled_turn_replays_to_eof_without_a_client_abort() {
    qualify(Case::CancelledReplay).await;
}

#[tokio::test]
async fn sse_client_abort_has_a_receipt_and_preserves_unknown_server_attribution() {
    qualify(Case::ClientAbort).await;
}

#[tokio::test]
async fn sse_body_failure_closes_without_a_client_abort_receipt() {
    qualify(Case::BodyFailure).await;
}
