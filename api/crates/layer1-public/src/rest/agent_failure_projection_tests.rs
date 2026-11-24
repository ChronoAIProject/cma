use super::*;
use layer2_core::sandbox::TypedFailure;
use serde_json::json;

const AT: &str = "2026-11-19T00:00:00Z";

#[test]
fn provider_failure_direct_rest_serializers_protect_raw_views() {
    for class in ["agent_turn_failed", "PRIVATE_CLASS_SENTINEL"] {
        let failure = TypedFailure { class: class.into(), detail: "unexpected status 524: <html>PRIVATE_BODY_SENTINEL https://failure.invalid X-Private-Header</html>".into() };
        let raw = AgentVisibleLogEntry {
            seq: 6,
            appended_at: AT.into(),
            retained_output: None,
            deferred_failure_detail: Some("PRIVATE_PROOF_SENTINEL".into()),
            kind: Some(AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: failure.clone(),
            }),
        };
        let event =
            serde_json::to_value(public_event(raw).unwrap_or_else(|_| panic!("valid event")))
                .unwrap();
        assert_eq!(event["seq"], 6);
        assert_eq!(event["appended_at"], AT);
        assert_eq!(event["kind"]["turn_sequence"], 3);
        assert_safe(&event);
        let transcript = serde_json::from_value(json!({
            "agent_id":"agt_safe_failure", "through_seq":6, "turns":[{
                "turn_sequence":3,"started_at":AT,"origin_key":"private-origin",
                "metadata":{"private":"metadata"},"owner_message":"Begin",
                "agent_messages":["<p>503 assistant content</p>"], "messages":[],
                "outcome":{"type":"failed","at":AT,"failure":failure}
            }]
        }))
        .unwrap();
        let transcript = serde_json::to_value(
            public_transcript(transcript).unwrap_or_else(|_| panic!("valid transcript")),
        )
        .unwrap();
        assert_safe(&transcript);
        assert_eq!(transcript["through_seq"], 6);
        assert_eq!(transcript["turns"][0]["outcome"]["at"], AT);
        assert_eq!(
            transcript["turns"][0]["agent_messages"][0],
            "<p>503 assistant content</p>"
        );
        assert!(transcript["turns"][0].get("metadata").is_none());
    }
}

fn assert_safe(value: &serde_json::Value) {
    let wire = value.to_string();
    for private in [
        "PRIVATE_BODY_SENTINEL",
        "PRIVATE_CLASS_SENTINEL",
        "PRIVATE_PROOF_SENTINEL",
        "failure.invalid",
        "X-Private-Header",
    ] {
        assert!(
            !wire.contains(private),
            "public payload exposed {private}: {wire}"
        );
    }
    assert!(wire.contains("agent_turn_failed"));
    assert!(wire.contains("The Agent run failed."));
}
