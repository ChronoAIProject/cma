use serde_json::json;

use super::*;
use crate::sandbox::{AgentLogEntry, AgentLogKind, AgentOperationDelivery, AgentOperationKind};
use crate::{AgentId, RuntimeCapabilities, RuntimeCapability};

#[test]
fn event_projection_keeps_exact_cursor_without_internal_execution_authority() {
    let cases = [
        AgentLogKind::ConversationOpened {
            conversation_id: "native-private-id".into(),
            runtime_generation: 8,
        },
        AgentLogKind::ApprovalRequested {
            approval_id: "approval-one".into(),
            action: Some("write".into()),
            detail: Some("src/main.rs".into()),
            runtime_locator: Some("private-locator".into()),
        },
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 42,
            origin_key: Some("private-origin".into()),
        },
        AgentLogKind::SignInRequired {
            url: "https://example.test/secret-sign-in".into(),
            code: Some("private-code".into()),
            message: None,
        },
    ];
    for kind in cases {
        let event = AgentEvent::try_from(AgentLogEntry {
            agent_id: AgentId("agt_existing".into()),
            seq: 42,
            appended_at: "2026-09-14T00:00:00Z".into(),
            kind,
        })
        .unwrap();
        assert_eq!(event.cursor, 42);
        assert_eq!(event.agent_id.as_str(), "agt_existing");
        let encoded = serde_json::to_string(&event).unwrap();
        for private in [
            "native-private-id",
            "private-locator",
            "private-origin",
            "secret-sign-in",
            "private-code",
            "conversation_id",
            "runtime_locator",
            "origin_key",
        ] {
            assert!(!encoded.contains(private), "exposed {private}");
        }
        assert_eq!(serde_json::from_str::<AgentEvent>(&encoded).unwrap(), event);
    }
}

#[test]
fn ambiguous_delivery_is_neither_failed_execution_nor_completion() {
    let event = AgentEvent::try_from(AgentLogEntry {
        agent_id: AgentId("agt_existing".into()),
        seq: 6,
        appended_at: "2026-09-14T00:00:00Z".into(),
        kind: AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 2,
            operation: AgentOperationKind::SendMessage,
            state: AgentOperationDelivery::RecoveryRequired,
        },
    })
    .unwrap();
    assert!(matches!(
        event.kind,
        AgentEventKind::DeliveryChanged {
            state: AgentOperationDelivery::RecoveryRequired,
            ..
        }
    ));
    for status in [
        AgentResponseStatus::Queued,
        AgentResponseStatus::InProgress,
        AgentResponseStatus::WaitingAction,
    ] {
        assert!(!status.is_terminal());
    }
    for status in [
        AgentResponseStatus::Completed,
        AgentResponseStatus::Failed,
        AgentResponseStatus::Cancelled,
    ] {
        assert!(status.is_terminal());
    }
}

#[test]
fn live_reasoning_and_text_cannot_be_deserialized_as_durable_events() {
    let live = AgentEphemeralEvent::ReasoningSummaryDelta {
        agent_id: AgentId("agt_existing".into()),
        turn_sequence: 2,
        message_id: "message-one".into(),
        delta: "Checking the files".into(),
    };
    let encoded = serde_json::to_value(live).unwrap();
    assert!(encoded.get("cursor").is_none());
    assert!(serde_json::from_value::<AgentEvent>(encoded.clone()).is_err());
    assert!(serde_json::from_value::<AgentEvent>(json!({"schema_version":1,"agent_id":"agt_existing","cursor":3,"appended_at":"2026-09-14T00:00:00Z","kind":encoded})).is_err());
}

#[test]
fn interface_capabilities_do_not_advertise_unimplemented_delivery_features() {
    let capabilities = AgentInterfaceCapabilities::from_runtime(&RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Steer,
    ]));
    assert!(capabilities.text_input && capabilities.steer && capabilities.durable_event_replay);
    assert!(capabilities.busy_message_queue);
    assert!(
        !AgentInterfaceCapabilities::from_runtime(&RuntimeCapabilities::default())
            .busy_message_queue
    );
    assert!(
        !capabilities.attachment_input
            && !capabilities.interrupt
            && !capabilities.durable_text_deltas
    );
    let absent = AgentInterfaceCapabilities::from_runtime(&RuntimeCapabilities::default());
    assert!(!absent.text_input && !absent.steer && !absent.approval_control);
}

#[test]
fn credential_and_grant_revision_rotation_preserve_the_same_idempotency_scope() {
    let mut authority = AgentAuthorizationRevision {
        replay_principal: None,
        authority_id: "bridge-grant-one".into(),
        policy_revision: 1,
        grant_revision: Some(3),
    };
    let agent = AgentId("agt_existing".into());
    let scope = authority.idempotency_scope(&agent, AgentOperationKind::SendMessage);
    authority.policy_revision += 1;
    authority.grant_revision = Some(4);
    assert_eq!(
        scope,
        authority.idempotency_scope(&agent, AgentOperationKind::SendMessage)
    );
    assert_ne!(
        scope,
        authority.idempotency_scope(&agent, AgentOperationKind::Steer)
    );
    assert_ne!(
        scope,
        authority.idempotency_scope(
            &AgentId("agt_another".into()),
            AgentOperationKind::SendMessage
        )
    );
    authority.authority_id = "bridge-grant-two".into();
    assert_ne!(
        scope,
        authority.idempotency_scope(&agent, AgentOperationKind::SendMessage)
    );
}
