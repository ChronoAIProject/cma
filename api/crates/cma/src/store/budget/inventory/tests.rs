use super::*;
use crate::store::agents::agent_document;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use layer2_core::budget::{BudgetPolicy, TurnReservation};
use layer2_core::sandbox::Agent;
use mongodb::bson::{doc, Document};
use std::os::unix::fs::{symlink, PermissionsExt};

pub(super) fn agent(id: &str) -> Agent {
    let mut agent = ready_agent(&active_sandbox("inventory-parent"), id);
    agent.status = layer2_core::sandbox::AgentStatus::Working;
    agent.conversation.last_seq = 4;
    agent.conversation.active_turn = Some(2);
    agent
}

pub(super) fn saved(agent: &Agent) -> super::super::TurnRecord {
    super::super::TurnRecord {
        owner: agent.owner.clone(),
        agent_id: agent.id.clone(),
        turn_sequence: Some(2),
        runtime_generation: Some(1),
        period_start: Some(123),
        period_shard: Some(2),
        reservation: TurnReservation::new(
            BudgetPolicy {
                owner_day_limit: 1000,
                turn_limit: 100,
                max_calls_per_turn: 10,
                day_timezone: "UTC".into(),
                input_token_units: 1,
                output_token_units: 2,
                max_unobserved_units: 10,
                usage_timeout_seconds: 60,
            },
            123,
        ),
    }
}

pub(super) fn document(saved: &super::super::TurnRecord) -> Document {
    doc! {"_id": format!("turn:{}:{}", saved.agent_id, saved.turn_sequence.unwrap()),
    "document_schema_version": 1_i64,
    "record": mongodb::bson::serialize_to_bson(saved).unwrap()}
}

#[test]
fn decoding_refuses_key_schema_and_policy_conflicts_even_for_settled_turns() {
    let mut saved = saved(&agent("fixture"));
    saved.reservation.settled = true;
    let valid = document(&saved);
    assert!(projection::turn(&valid).is_ok());
    for (key, value) in [
        ("_id", mongodb::bson::Bson::from("turn:another:2")),
        ("document_schema_version", 999_i64.into()),
    ] {
        let mut invalid = valid.clone();
        invalid.insert(key, value);
        assert!(projection::turn(&invalid).is_err());
    }
    let mut invalid = valid;
    invalid
        .get_document_mut("record")
        .unwrap()
        .insert("future_field", true);
    assert!(projection::turn(&invalid).is_err());
    saved.reservation.policy.turn_limit = 0;
    assert!(projection::turn(&document(&saved)).is_err());
}

#[test]
fn saved_amounts_and_identity_remain_exact_without_policy_aggregation() {
    let mut saved = saved(&agent("fixture"));
    saved.reservation.reserved_units = (1_u64 << 53) + 1;
    saved.reservation.observed = i64::MAX as u64;
    saved.reservation.final_model_calls = Some((1_u64 << 53) + 3);
    saved.runtime_generation = None;
    saved.period_start = None;
    saved.period_shard = None;
    let saved = projection::turn(&document(&saved)).unwrap();
    let value = projection::exact_saved(&saved).unwrap();
    assert_eq!(value["reservation"]["reserved_units"], "9007199254740993");
    assert_eq!(value["reservation"]["observed"], i64::MAX.to_string());
    assert_eq!(
        value["reservation"]["final_model_calls"],
        "9007199254740995"
    );
    assert_eq!(value["reservation"]["final_usage_known"], false);
    assert_eq!(value["reservation"]["policy"]["output_token_units"], "2");
    assert!(value["runtime_generation"].is_null());
    assert!(value.get("period_start").is_none());
}

#[test]
fn linkage_preserves_missing_conflicting_and_generation_evidence() {
    let mut agent = agent("fixture");
    let mut saved = saved(&agent);
    let classify = |saved: &super::super::TurnRecord, agent: &Agent| {
        projection::linkage(saved, Some(&agent_document(agent).unwrap())).0
    };
    assert_eq!(classify(&saved, &agent), "matching_active_turn");
    let mut segmented = agent_document(&agent).unwrap();
    segmented.insert("document_schema_version", 2_i64);
    segmented.insert("agent_log_format", crate::store::agent_log_segments::FORMAT);
    segmented.insert(
        "agent_log_incarnation",
        crate::store::agent_log_segments::incarnation(&agent),
    );
    segmented.insert("agent_log_head_revision", 1_i64);
    assert_eq!(
        projection::linkage(&saved, Some(&segmented)).0,
        "matching_active_turn"
    );
    let mut invalid = agent.clone();
    invalid.status = layer2_core::sandbox::AgentStatus::Ready;
    assert_eq!(classify(&saved, &invalid), "agent_unreadable");
    agent.conversation.runtime_generation = Some(3);
    assert_eq!(classify(&saved, &agent), "generation_changed");
    saved.runtime_generation = None;
    assert_eq!(classify(&saved, &agent), "generation_missing");
    agent.conversation.active_turn = Some(3);
    assert_eq!(classify(&saved, &agent), "different_active_turn");
    agent.conversation.active_turn = None;
    agent.status = layer2_core::sandbox::AgentStatus::Ready;
    assert_eq!(classify(&saved, &agent), "no_active_turn");
    agent.owner = layer2_core::SubjectId::new("other-owner");
    assert_eq!(classify(&saved, &agent), "agent_identity_conflict");
    assert_eq!(projection::linkage(&saved, None).0, "agent_missing");
    let mut row = agent_document(&agent).unwrap();
    row.insert("document_schema_version", 999_i64);
    assert_eq!(
        projection::linkage(&saved, Some(&row)).0,
        "agent_unreadable"
    );
    row.insert(crate::store::agent_log_integrity::QUARANTINE_FIELD, true);
    assert_eq!(
        projection::linkage(&saved, Some(&row)).0,
        "agent_quarantined"
    );
}

#[test]
fn authority_requires_exact_scoped_reads_and_refuses_write_or_foreign_grants() {
    let role = authority::test_role("fixture");
    let grants = role.get_array("privileges").unwrap().clone();
    let status = |grants| {
        doc! {"authInfo": {"authenticatedUsers": [{"user": "reader"}],
        "authenticatedUserPrivileges": grants}}
    };
    authority::validate(&status(grants.clone()), "fixture").unwrap();
    assert!(authority::validate(&status(grants.clone()), "other").is_err());
    let mut incomplete = grants.clone();
    incomplete.pop();
    assert!(authority::validate(&status(incomplete), "fixture").is_err());
    for action in [
        "insert",
        "update",
        "remove",
        "createCollection",
        "createIndex",
        "collMod",
    ] {
        let mut expanded = grants.clone();
        expanded[0]
            .as_document_mut()
            .unwrap()
            .get_array_mut("actions")
            .unwrap()
            .push(action.into());
        assert!(authority::validate(&status(expanded), "fixture").is_err());
    }
}

#[test]
fn publication_is_private_create_only_and_receipt_does_not_disclose_identities() {
    let directory = std::env::temp_dir().join(format!("turn-inventory-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory = std::fs::canonicalize(directory).unwrap();
    let path = directory.join("report.json");
    let report = CompleteInventory {
        bytes: b"private-owner-and-agent".to_vec(),
        selected: 1,
        settled: 0,
        outcomes: BTreeMap::from([("agent_missing", 1)]),
    };
    let receipt = cli::publish(&path, &report).unwrap();
    assert!(!receipt.contains("private-owner"));
    assert_eq!(std::fs::read(&path).unwrap(), report.bytes);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(cli::publish(&path, &report).is_err());
    let alias = directory.join("alias");
    symlink(&path, &alias).unwrap();
    assert!(cli::publish(&alias, &report).is_err());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(cli::publish(&directory.join("other"), &report).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn malformed_inventory_command_refuses_before_database_or_service_startup() {
    for args in [
        vec!["--turn-budget-inventory"],
        vec!["--turn-budget-inventory-apply"],
        vec!["--turn-budget-inventory", "relative.json"],
        vec!["other", "--turn-budget-inventory", "/unavailable/path"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(cli::is_command(&args));
        assert!(cli::command(&args).await.is_err());
    }
    assert!(!cli::is_command(&[]));
}
