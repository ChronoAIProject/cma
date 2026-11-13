//! Runtime isolation must cover every segmentation sidecar without widening
//! collection authority to a database, a foreign service or obsolete carriers.
use super::*;
use crate::store::{agent_capacity_recovery::records, agent_log_segments as segments};
use mongodb::bson::Bson;

fn status(grants: Vec<Bson>) -> Document {
    doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],
    "authenticatedUserPrivileges":grants}}
}

fn collection(grant: &Bson) -> &str {
    grant
        .as_document()
        .unwrap()
        .get_document("resource")
        .unwrap()
        .get_str("collection")
        .unwrap()
}

#[test]
fn complete_segment_inventory_is_accepted_by_exact_runtime_authority() {
    let grants = role("fixture").get_array("privileges").unwrap().clone();
    validate(&status(grants.clone()), "fixture").unwrap();
    let mut required = segments::COLLECTIONS.to_vec();
    if !required.contains(&records::COLLECTION) {
        required.push(records::COLLECTION);
    }
    for name in required {
        assert_eq!(
            grants
                .iter()
                .filter(|grant| collection(grant) == name)
                .count(),
            1
        );
        let mut missing = grants.clone();
        missing.retain(|grant| collection(grant) != name);
        assert!(validate(&status(missing), "fixture").is_err(), "{name}");
    }
}

#[test]
fn writer_control_and_recovery_require_their_actual_runtime_operations() {
    for name in [
        segments::writer_gate::COLLECTION,
        segments::writer_admission::COLLECTION,
        records::COLLECTION,
    ] {
        for action in ["find", "insert", "update", "listIndexes"] {
            let mut grants = role("fixture").get_array("privileges").unwrap().clone();
            let grant = grants
                .iter_mut()
                .find(|grant| collection(grant) == name)
                .unwrap();
            grant
                .as_document_mut()
                .unwrap()
                .get_array_mut("actions")
                .unwrap()
                .retain(|value| value.as_str() != Some(action));
            assert!(
                validate(&status(grants), "fixture").is_err(),
                "{name}: {action}"
            );
        }
    }
}

#[test]
fn segment_authority_refuses_foreign_database_wildcard_and_extra_actions() {
    let grants = role("fixture").get_array("privileges").unwrap().clone();
    for (database, name, action) in [
        ("foreign", records::COLLECTION, "find"),
        ("fixture", "", "find"),
        ("fixture", "eg_channel_records", "find"),
        ("fixture", "agent_capacity_recovery_work_v1", "find"),
        ("fixture", records::COLLECTION, "dropCollection"),
        ("fixture", segments::writer_gate::COLLECTION, "dropIndex"),
    ] {
        let mut changed = grants.clone();
        changed.push(
            doc! {"resource":{"db":database,"collection":name},
            "actions":[action]}
            .into(),
        );
        assert!(
            validate(&status(changed), "fixture").is_err(),
            "{name}: {action}"
        );
    }
}
