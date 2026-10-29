use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::sandbox::AgentOperationError;

#[test]
fn schema_one_fingerprint_keeps_its_persisted_canonical_encoding() {
    // Independently encoded with sorted JSON object keys and UTF-8 text.
    assert_eq!(
        AgentCommand::message("hello").fingerprint().unwrap(),
        "59caf2610b09b536f3056230ea497b355e3c287f421549172bdb34fa6dae6a67"
    );
}

#[test]
fn canonical_text_partitions_and_metadata_order_share_a_fingerprint() {
    let simple = AgentCommand::message("hello world");
    let partitioned = AgentCommand::Message {
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
        order: AgentInputOrder::Append,
        metadata: BTreeMap::new(),
    };
    assert_eq!(
        simple.fingerprint().unwrap(),
        partitioned.fingerprint().unwrap()
    );
    let left: AgentCommand = serde_json::from_value(json!({"type":"message","input":{"content":[{"type":"text","text":"hello"}]},"metadata":{"a":"1","b":"2"}})).unwrap();
    let right: AgentCommand = serde_json::from_value(json!({"metadata":{"b":"2","a":"1"},"input":{"content":[{"text":"hello","type":"text"}]},"type":"message"})).unwrap();
    assert_eq!(left.fingerprint().unwrap(), right.fingerprint().unwrap());
}

#[test]
fn execution_relevant_changes_never_reuse_an_input_fingerprint() {
    let base = AgentCommand::message("hello");
    let expected = base.fingerprint().unwrap();
    for command in [
        AgentCommand::message("hello "),
        AgentCommand::Steer {
            input: AgentInput::text("hello"),
        },
        AgentCommand::Message {
            input: AgentInput::text("hello"),
            order: AgentInputOrder::AfterTurn {
                turn_sequence: Some(4),
            },
            metadata: BTreeMap::new(),
        },
        AgentCommand::Message {
            input: AgentInput::text("hello"),
            order: AgentInputOrder::Append,
            metadata: BTreeMap::from([("purpose".into(), "review".into())]),
        },
    ] {
        assert_ne!(expected, command.fingerprint().unwrap());
    }
}

#[test]
fn command_intent_cannot_deserialize_server_authority_fields() {
    let original = serde_json::to_value(AgentCommand::message("hello")).unwrap();
    for field in [
        "owner",
        "actor",
        "authority",
        "grant_revision",
        "command_id",
        "response_id",
        "admitted_at",
        "schema_version",
        "source",
    ] {
        let mut forged = original.clone();
        forged[field] = json!("forged");
        assert!(
            serde_json::from_value::<AgentCommand>(forged).is_err(),
            "accepted {field}"
        );
    }
}

#[test]
fn attachments_are_hash_bound_and_never_silently_dropped() {
    let mut reference = AgentAttachmentReference {
        id: "attachment-one".into(),
        sha256: "a".repeat(64),
        media_type: "image/png".into(),
        size_bytes: 4096,
    };
    let input = AgentInput {
        content: vec![
            AgentContent::Text {
                text: "describe".into(),
            },
            AgentContent::Attachment {
                reference: reference.clone(),
            },
        ],
    };
    let original = AgentCommand::Steer {
        input: input.clone(),
    }
    .fingerprint()
    .unwrap();
    assert_eq!(
        input.into_text(),
        Err(AgentOperationError::ContentUnsupported)
    );
    reference.sha256 = "b".repeat(64);
    let changed = AgentCommand::Steer {
        input: AgentInput {
            content: vec![
                AgentContent::Text {
                    text: "describe".into(),
                },
                AgentContent::Attachment { reference },
            ],
        },
    };
    assert_ne!(original, changed.fingerprint().unwrap());
}

#[test]
fn invalid_or_unbounded_content_is_refused() {
    for input in [
        AgentInput { content: vec![] },
        AgentInput::text(" \n"),
        AgentInput::text("x".repeat(crate::sandbox::MAX_AGENT_MESSAGE_BYTES + 1)),
        AgentInput {
            content: vec![AgentContent::Text { text: "x".into() }; 33],
        },
    ] {
        assert!(input.canonical().is_err());
    }
    let mut command = serde_json::to_value(AgentCommand::message("hello")).unwrap();
    command["input"]["content"][0]["owner"] = json!("forged");
    assert!(serde_json::from_value::<AgentCommand>(command).is_err());
}

#[test]
fn admission_receipt_binds_version_input_and_order_without_conflating_owner_and_actor() {
    use crate::ids::{AgentCommandId, AgentResponseId};
    use crate::{AgentId, SubjectId};
    let command = AgentCommand::message("hello");
    let receipt = AdmissionReceipt {
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: AgentCommandId::generate(),
            agent_id: AgentId("agt_existing".into()),
            owner: AgentOwner::Organization {
                subject: SubjectId::new("organization-one"),
            },
            actor: AgentActor::Human {
                subject: SubjectId::new("admin-human"),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "interaction-one".into(),
            },
            correlation_id: "correlation-one".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                authority_id: "membership-one".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        response_id: Some(AgentResponseId::generate()),
        admitted_at: "2026-09-14T00:00:00Z".into(),
        agent_order: 1,
        event_cursor: 0,
    };
    receipt.validate_integrity().unwrap();
    let encoded = serde_json::to_value(&receipt).unwrap();
    assert_eq!(encoded["envelope"]["owner"]["subject"], "organization-one");
    assert_eq!(encoded["envelope"]["actor"]["subject"], "admin-human");
    for field in ["version", "input", "order", "time", "response"] {
        let mut invalid = receipt.clone();
        match field {
            "version" => invalid.envelope.schema_version += 1,
            "input" => invalid.envelope.command = AgentCommand::message("changed"),
            "order" => invalid.agent_order = 0,
            "time" => invalid.admitted_at = "not-a-time".into(),
            "response" => invalid.response_id = None,
            _ => unreachable!(),
        }
        assert!(invalid.validate_integrity().is_err(), "accepted {field}");
    }
}
