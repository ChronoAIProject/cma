use super::*;
use crate::sandbox::agent_io::{
    AgentCommand, AgentContent, AgentInput, CommandLedgerError, AGENT_IO_SCHEMA_VERSION,
};

#[test]
fn targeted_fingerprint_preserves_v1_and_binds_every_target_coordinate() {
    let agent = crate::AgentId("agt_fixture".into());
    let target = TargetedResponse {
        response_id: crate::ids::AgentResponseId("resp_fixture".into()),
        turn_sequence: 42,
    };
    assert_eq!(AGENT_IO_SCHEMA_VERSION, 1);
    assert_eq!(
        AgentCommand::message("hello").fingerprint().unwrap(),
        "59caf2610b09b536f3056230ea497b355e3c287f421549172bdb34fa6dae6a67"
    );
    let original = target
        .fingerprint(&agent, 17, &AgentCommand::Interrupt)
        .unwrap();
    assert_ne!(original, AgentCommand::Interrupt.fingerprint().unwrap());
    assert_ne!(
        original,
        target
            .fingerprint(
                &crate::AgentId("agt_other".into()),
                17,
                &AgentCommand::Interrupt
            )
            .unwrap()
    );
    assert_ne!(
        original,
        target
            .fingerprint(&agent, 18, &AgentCommand::Interrupt)
            .unwrap()
    );
    let mut changed = target.clone();
    changed.turn_sequence += 1;
    assert_ne!(
        original,
        changed
            .fingerprint(&agent, 17, &AgentCommand::Interrupt)
            .unwrap()
    );
    changed = target.clone();
    changed.response_id = crate::ids::AgentResponseId::generate();
    assert_ne!(
        original,
        changed
            .fingerprint(&agent, 17, &AgentCommand::Interrupt)
            .unwrap()
    );
    assert_eq!(
        target.fingerprint(&agent, 0, &AgentCommand::Interrupt),
        Err(CommandLedgerError::Invalid)
    );
    assert_eq!(
        target.fingerprint(&agent, 17, &AgentCommand::message("invalid action")),
        Err(CommandLedgerError::Invalid)
    );
}

#[test]
fn targeted_steer_uses_the_existing_canonical_content_rules() {
    let target = TargetedResponse {
        response_id: crate::ids::AgentResponseId::generate(),
        turn_sequence: 2,
    };
    let agent = crate::AgentId::generate();
    let plain = AgentCommand::Steer {
        input: AgentInput::text("hello world"),
    };
    let split = AgentCommand::Steer {
        input: AgentInput {
            content: vec![
                AgentContent::Text {
                    text: "hello ".into(),
                },
                AgentContent::Text {
                    text: "world".into(),
                },
            ],
        },
    };
    assert_eq!(
        target.fingerprint(&agent, 1, &plain).unwrap(),
        target.fingerprint(&agent, 1, &split).unwrap()
    );
}

#[test]
fn target_and_sidecar_decoders_refuse_unknown_fields() {
    let target = serde_json::json!({"response_id":"resp_fixture","turn_sequence":2,"runtime_locator":"forged"});
    assert!(serde_json::from_value::<TargetedResponse>(target).is_err());
    let binding = serde_json::json!({
        "schema_version":1, "command_id":"cmd_control", "request_fingerprint":"a".repeat(64),
        "response_id":"resp_fixture", "response_command_id":"cmd_response", "turn_sequence":2,
        "expected_agent_revision":1, "agent_created_at":"2026-09-05T00:00:00Z", "actor":"forged"
    });
    assert!(serde_json::from_value::<TargetedControlBinding>(binding).is_err());
    for target in [
        serde_json::json!({"response_id":"resp_fixture"}),
        serde_json::json!({"response_id":"resp_fixture","turn_sequence":null}),
    ] {
        assert!(serde_json::from_value::<TargetedResponse>(target).is_err());
    }
}
