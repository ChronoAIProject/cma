use super::*;
use crate::telemetry::turn_trace::{writer, SpanId};
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::mpsc,
    time::Duration,
};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{layer::SubscriberExt, Layer};

struct Capture(mpsc::Sender<Vec<u8>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture() -> (writer::Output, mpsc::Receiver<Vec<u8>>) {
    let (sender, receiver) = mpsc::channel();
    (writer::start(Capture(sender)).unwrap(), receiver)
}

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "traceparent",
        "00-11111111111141118111111111111111-0123456789abcdef-01"
            .parse()
            .unwrap(),
    );
    headers.insert(
        "x-request-id",
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
    );
    headers.insert("authorization", "private-header-canary".parse().unwrap());
    headers
}

fn line(receiver: &mpsc::Receiver<Vec<u8>>) -> Value {
    let bytes = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private"));
    serde_json::from_slice(&bytes).unwrap()
}

fn subscriber() -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::registry()
        .with(scope::testable_layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(io::sink)
                .with_filter(tracing_subscriber::filter::LevelFilter::OFF),
        )
}

#[tokio::test]
async fn authorization_is_required_and_transport_does_not_claim_platform_confirmation() {
    let (output, records) = capture();
    async {
        let request = RequestTrace::new(output.writer.clone(), &headers());
        assert!(request.headers.get("authorization").is_none());
        request.finish(StatusCode::UNAUTHORIZED);
        drop(request);
        assert!(records.try_recv().is_err());

        let request = RequestTrace::new(output.writer.clone(), &headers());
        let span = request.span.clone();
        REQUEST
            .scope(
                request,
                async {
                    assert!(scope::context(&tracing::Span::current()).is_none());
                    authorized();
                    authorized(); // Repeated checks must not start another operation.
                    let context = scope::context(&tracing::Span::current()).unwrap();
                    assert_eq!(context.parent_span_id, SpanId::parse("0123456789abcdef"));
                    REQUEST.with(|trace| trace.finish(StatusCode::ACCEPTED));
                }
                .instrument(span),
            )
            .await;
    }
    .with_subscriber(subscriber())
    .await;
    let start = line(&records);
    let end = line(&records);
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(start["request_id"], end["request_id"]);
    assert_ne!(start["request_id"], start["ingress_request_id"]);
    assert_eq!(end["outcome"], "completed");
    assert_eq!(end["stage"], "gateway");
    assert_eq!(end["http_status"], 202);
    assert!(records.try_recv().is_err());
}

#[tokio::test]
async fn queued_span_retains_original_turn_after_newer_request_and_filtered_logs() {
    let (output, records) = capture();
    async {
        let first = RequestTrace::new(output.writer.clone(), &headers());
        first.authorize();
        let queued = first.span.clone();
        let original = scope::context(&queued).unwrap();
        first.finish(StatusCode::ACCEPTED);
        drop(first);
        let mut newer = headers();
        newer.insert(
            "traceparent",
            "00-33333333333343338333333333333333-fedcba9876543210-01"
                .parse()
                .unwrap(),
        );
        let next = RequestTrace::new(output.writer.clone(), &newer);
        next.authorize();
        assert_ne!(
            scope::context(&next.span).unwrap().turn_id,
            original.turn_id
        );
        next.finish(StatusCode::ACCEPTED);
        let writer = output.writer.clone();
        let future = async move {
            let parent = scope::context(&tracing::Span::current()).unwrap();
            assert_eq!(parent.turn_id, original.turn_id);
            let operation = Operation::start(writer, Stage::Platform, Some(parent), Some(1));
            operation.finish(Outcome::Unknown, Some(502));
        }
        .instrument(queued)
        .with_current_subscriber();
        tokio::spawn(future).await.unwrap();
    }
    .with_subscriber(subscriber())
    .await;
    let lines: Vec<_> = (0..6).map(|_| line(&records)).collect();
    assert_eq!(lines[4]["turn_id"], lines[0]["turn_id"]);
    assert_eq!(lines[4]["parent_span_id"], lines[0]["span_id"]);
    assert_eq!(lines[4]["span_id"], lines[5]["span_id"]);
    assert_eq!(lines[5]["outcome"], "unknown");
}

#[tokio::test]
async fn invalid_or_duplicate_headers_do_not_create_trace_records() {
    let (output, records) = capture();
    async {
        let mut duplicate = headers();
        duplicate.append("traceparent", duplicate["traceparent"].clone());
        let mut invalid = headers();
        invalid.insert("traceparent", "private-malformed-canary".parse().unwrap());
        let mut oversized = headers();
        oversized.insert("traceparent", "x".repeat(257).parse().unwrap());
        let mut noncanonical = headers();
        noncanonical.insert(
            "traceparent",
            "00-11111111111111118111111111111111-0123456789abcdef-01"
                .parse()
                .unwrap(),
        );
        for headers in [
            HeaderMap::new(),
            duplicate,
            invalid,
            oversized,
            noncanonical,
        ] {
            let request = RequestTrace::new(output.writer.clone(), &headers);
            request.authorize();
            assert!(scope::context(&request.span).is_none());
            request.finish(StatusCode::OK);
        }
    }
    .with_subscriber(subscriber())
    .await;
    assert!(records.try_recv().is_err());
}
