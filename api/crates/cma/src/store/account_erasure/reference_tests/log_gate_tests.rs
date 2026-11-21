use super::super::references;
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn log_gate_redaction_waits_for_live_slots_and_preserves_other_cohorts() {
    use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord};
    use mongodb::bson::{spec::BinarySubtype, Binary, DateTime};
    let db = fixture().await;
    let agents = BTreeSet::from(["agt-erased".into()]);
    let holder = "10:agt-erased:incarnation:3:key";
    let original = doc! {"_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,"admission_fence":2_i64,"policy":{"enabled":true,"owners":["erased","other"],"agents":["agt-erased","agt-other"],"cohort_seed":"","basis_points":0_i32},"slots":[{"holder":holder,"fingerprint":Binary {subtype:BinarySubtype::Generic,bytes:vec![1;32]},"fence":1_i64,"expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()+300000)}]};
    GateRecord::read(GateName::Segments, original.clone()).unwrap();
    db.collection::<Document>("agent_log_gates")
        .insert_one(original.clone())
        .await
        .unwrap();
    assert_eq!(
        references::redact_with_agents(
            &db,
            "agent_log_gates",
            &original,
            &subjects(),
            &agents,
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Waiting
    );
    assert_eq!(
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
        original
    );
    db.collection::<Document>("agent_log_gates")
        .update_one(
            doc! {"_id":"agent_log_segments_v1"},
            doc! {"$set":{"slots.0.expires_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let expired = row(&db, "agent_log_gates", "agent_log_segments_v1").await;
    assert_eq!(
        references::redact_with_agents(
            &db,
            "agent_log_gates",
            &expired,
            &subjects(),
            &agents,
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    let parsed = GateRecord::read(
        GateName::Segments,
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
    )
    .unwrap();
    assert_eq!(parsed.revision, 2);
    assert_eq!(parsed.policy.owners, ["other"]);
    assert_eq!(parsed.policy.agents, ["agt-other"]);
    assert!(parsed.slots.is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn log_gate_redaction_disables_an_empty_rollout_without_expanding_it() {
    use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord};
    let db = fixture().await;
    let original = doc! {"_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,"admission_fence":0_i64,"policy":{"enabled":true,"owners":["erased"],"agents":[],"cohort_seed":"","basis_points":0_i32},"slots":[]};
    db.collection::<Document>("agent_log_gates")
        .insert_one(original.clone())
        .await
        .unwrap();
    assert_eq!(
        references::redact_with_agents(
            &db,
            "agent_log_gates",
            &original,
            &subjects(),
            &BTreeSet::new(),
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    let parsed = GateRecord::read(
        GateName::Segments,
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
    )
    .unwrap();
    assert!(!parsed.policy.enabled);
    assert!(parsed.policy.owners.is_empty());
    assert_eq!(parsed.revision, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn log_gate_redaction_compares_slots_that_change_without_revision() {
    use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord};
    use mongodb::bson::{spec::BinarySubtype, Binary, DateTime};
    let db = fixture().await;
    let original = doc! {"_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,"admission_fence":1_i64,"policy":{"enabled":true,"owners":["erased","other"],"agents":[],"cohort_seed":"","basis_points":0_i32},"slots":[{"holder":"10:agt-erased:incarnation:3:key","fingerprint":Binary {subtype:BinarySubtype::Generic,bytes:vec![1;32]},"fence":1_i64,"expires_at":DateTime::from_millis(1)}]};
    db.collection::<Document>("agent_log_gates")
        .insert_one(original.clone())
        .await
        .unwrap();
    let mut concurrent = original.clone();
    concurrent.get_array_mut("slots").unwrap()[0]
        .as_document_mut()
        .unwrap()
        .insert(
            "expires_at",
            DateTime::from_millis(DateTime::now().timestamp_millis() + 300000),
        );
    db.collection::<Document>("agent_log_gates")
        .replace_one(doc! {"_id":"agent_log_segments_v1"}, concurrent.clone())
        .await
        .unwrap();
    let agents = BTreeSet::from(["agt-erased".into()]);
    assert_eq!(
        references::redact_with_agents(
            &db,
            "agent_log_gates",
            &original,
            &subjects(),
            &agents,
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Conflict
    );
    assert_eq!(
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
        concurrent
    );
    GateRecord::read(GateName::Segments, concurrent).unwrap();
    db.drop().await.unwrap();
}
