use super::*;
use crate::{
    ingress::routing_plan::fixture,
    providers::{parse_event, SseFrame},
};
use serde_json::json;

struct SyntheticVerifier;
impl ExecutionReceiptVerifier for SyntheticVerifier {
    fn verify(&self, scope: &ExecutionScope, terminal: &Value) -> Option<ExecutionReceipt> {
        let handoff = terminal.get("synthetic_handoff")?;
        let mut binding = scope.clone();
        binding.event_id = handoff.get("event")?.as_str()?.into();
        binding.consumer_id = handoff.get("consumer")?.as_str()?.into();
        binding.provider = handoff.get("provider")?.as_str()?.into();
        binding.channel_record_id = handoff.get("record")?.as_str()?.into();
        binding.response_id = handoff.get("response")?.as_str()?.into();
        Some(ExecutionReceipt {
            scope: binding,
            reference: handoff.get("executor")?.as_str()?.into(),
        })
    }
}

fn decode(value: &Value) -> ResponsesEvent {
    parse_event(&SseFrame {
        event: None,
        data: value.to_string(),
        id: None,
        retry: None,
    })
    .unwrap()
}

#[test]
fn only_terminal_handoff_with_matching_response_and_scope_qualifies() {
    let request = fixture::request(|_| {}, fixture::message("synthetic message"));
    let frame = json!({
        "type": "response.completed",
        "response_id": "response-one",
        "response": {
            "id": "response-one",
            "conversation": {"id": "native-provider-agent"},
            "synthetic_handoff": {
                "event": request.activity.event_id.as_str(),
                "record": request.channel.definition.record_id.as_str(),
                "consumer": "default",
                "provider": "cma_codex",
                "response": "response-one",
                "admitted_by": "provider-admission-worker",
                "executor": "terminal-provider-executor"
            }
        }
    });
    let verify = |frame: &Value| {
        observe(
            Some(&SyntheticVerifier),
            &request,
            "cma_codex",
            Some("response-one"),
            &decode(frame),
        )
    };
    assert!(verify(&frame).is_some());
    assert!(observe(
        None,
        &request,
        "cma_codex",
        Some("response-one"),
        &decode(&frame)
    )
    .is_none());
    assert!(observe(
        Some(&SyntheticVerifier),
        &request,
        "cma_codex",
        None,
        &decode(&frame)
    )
    .is_none());
    for (pointer, invalid) in [
        ("/type", json!("response.created")),
        ("/response_id", json!("different-wire-response")),
        ("/response/id", json!("different-snapshot-response")),
        (
            "/response/synthetic_handoff/event",
            json!("different-event"),
        ),
        (
            "/response/synthetic_handoff/record",
            json!("different-channel"),
        ),
        (
            "/response/synthetic_handoff/consumer",
            json!("different-consumer"),
        ),
        (
            "/response/synthetic_handoff/provider",
            json!("different-provider"),
        ),
        (
            "/response/synthetic_handoff/response",
            json!("different-receipt-response"),
        ),
        (
            "/response/synthetic_handoff/executor",
            json!("https://secret.example"),
        ),
        (
            "/response/synthetic_handoff/executor",
            json!("x".repeat(129)),
        ),
        ("/response/synthetic_handoff", Value::Null),
    ] {
        let mut conflicting = frame.clone();
        *conflicting.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            verify(&conflicting).is_none(),
            "accepted conflicting {pointer}"
        );
    }
}
