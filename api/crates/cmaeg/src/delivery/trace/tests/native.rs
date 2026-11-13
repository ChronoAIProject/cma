use super::*;
use crate::{
    delivery::{gate::recording::RecordingGate, send::SendContext, telegram_media_tests::setup},
    telegram::media_fixture::{Answer, Fixture},
    telemetry::turn_trace::scope,
};
use cmaeg_core::message::OutboundMessage;
use serde_json::json;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

#[tokio::test]
async fn enabled_native_multipart_uses_original_turn_and_reports_confirmed_prefix_unknown_suffix() {
    let fixture = Fixture::new().await;
    fixture.answer(Answer::ok());
    fixture.answer(Answer {
        broken: true,
        ..Answer::ok()
    });
    let (sender, channel, event) = setup(&fixture);
    let message: OutboundMessage = serde_json::from_value(json!({
        "text":"private-message-canary",
        "native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},
            "filename":"private-filename-canary.bin","caption":"private-caption-canary"}]
    }))
    .unwrap();
    let gate = RecordingGate::new(true);
    let (writer, records, _guard) = testing::capture();
    let _installed = testing::install(writer);
    let subscriber = tracing_subscriber::registry().with(scope::layer());
    let turn_id = uuid::Uuid::new_v4();
    let parent = Context {
        turn_id: Some(turn_id),
        span_id: SpanId::fresh(),
        parent_span_id: None,
        request_id: uuid::Uuid::new_v4(),
        ingress_request_id: Some(uuid::Uuid::new_v4()),
    };
    let (receipt, _) = async {
        let span = scope::span();
        scope::attach(&span, parent);
        sender
            .reply(
                &SendContext {
                    trace: &span,
                    channel: &channel,
                    first_send_done: false,
                    gate: &gate,
                },
                &event,
                &message,
            )
            .await
    }
    .with_subscriber(subscriber)
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
    let calls = fixture.calls();
    assert_eq!(
        calls.len(),
        2,
        "an ambiguous platform response must not resend"
    );
    assert!(calls[0].path.ends_with("/sendMessage"));
    assert!(calls[1].path.ends_with("/sendDocument"));
    let lines: Vec<Value> = (0..6)
        .map(|_| {
            serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap()
        })
        .collect();
    for line in &lines {
        assert_eq!(line["turn_id"], turn_id.to_string());
        assert_eq!(line["stage"], "platform");
        assert!(!line.to_string().contains("private"));
        if line["phase"] == "end" {
            let start = lines
                .iter()
                .find(|start| start["phase"] == "start" && start["span_id"] == line["span_id"])
                .unwrap();
            assert_eq!(start["request_id"], line["request_id"]);
            assert_eq!(start["parent_span_id"], line["parent_span_id"]);
        }
    }
    let ends: Vec<_> = lines.iter().filter(|line| line["phase"] == "end").collect();
    assert_eq!(ends[0]["part_index"], 0);
    assert_eq!(ends[0]["outcome"], "confirmed");
    assert_eq!(ends[1]["part_index"], 1);
    assert_eq!(ends[1]["outcome"], "unknown");
    assert_eq!(ends[2]["outcome"], "partial");
    assert!(records.try_recv().is_err());
}

#[tokio::test]
async fn native_trace_off_keeps_request_bytes_and_gate_calls_identical() {
    let message: OutboundMessage =
        serde_json::from_value(json!({"text":"private-text-canary"})).unwrap();
    let (writer, records, _guard) = testing::capture();
    let mut calls = Vec::new();
    let mut gates = Vec::new();
    for enabled in [false, true] {
        let _installed = enabled.then(|| testing::install(writer.clone()));
        let fixture = Fixture::new().await;
        let (sender, channel, event) = setup(&fixture);
        let gate = RecordingGate::new(true);
        let subscriber = tracing_subscriber::registry().with(scope::layer());
        let (receipt, _) = async {
            let span = scope::span();
            scope::attach(
                &span,
                Context {
                    turn_id: Some(uuid::Uuid::new_v4()),
                    span_id: SpanId::fresh(),
                    parent_span_id: None,
                    request_id: uuid::Uuid::new_v4(),
                    ingress_request_id: None,
                },
            );
            sender
                .reply(
                    &SendContext {
                        trace: &span,
                        channel: &channel,
                        first_send_done: false,
                        gate: &gate,
                    },
                    &event,
                    &message,
                )
                .await
        }
        .with_subscriber(subscriber)
        .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        calls.push(fixture.calls()[0].body.clone());
        gates.push(gate.calls());
        if !enabled {
            assert!(records.try_recv().is_err());
        }
    }
    assert_eq!(calls[0], calls[1]);
    assert_eq!(gates[0], gates[1]);
    let lines: Vec<Value> = (0..4)
        .map(|_| {
            serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap()
        })
        .collect();
    assert!(lines
        .iter()
        .all(|line| !line.to_string().contains("private")));
    assert_eq!(
        lines
            .iter()
            .filter(|line| line["outcome"] == "confirmed")
            .count(),
        2
    );
    assert!(records.try_recv().is_err());
}
