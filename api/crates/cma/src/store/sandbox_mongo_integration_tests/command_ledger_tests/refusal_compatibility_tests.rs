use super::*;
use work_tests::{claim_request, live_request};

// Exact strict enum from B1 a66b63f08; intentionally no unknown fallback.
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum B1CommandFailureCode {
    QueueTimeout,
    AuthorityRevoked,
    AuthorityUnavailable,
    AgentUnavailable,
    TargetTurnChanged,
    RuntimeUnavailable,
    RuntimeLost,
    ExecutionFailed,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn prerequisite_refusals_persist_b1_codes_before_and_after_claim() {
    let (client, name) = database("refusal_compatibility").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    for code in [
        CommandFailureCode::CredentialSetupRequired,
        CommandFailureCode::ConnectionsRequired,
    ] {
        // Prove the predecessor decoder would detect the original regression.
        assert!(serde_json::from_value::<B1CommandFailureCode>(
            serde_json::to_value(code).unwrap()
        )
        .is_err());
        for claimed in [false, true] {
            let reservation = live_request(&agent, &parent, &format!("{code:?}-{claimed}"));
            let admitted = record(store.reserve_command(reservation.clone()).await.unwrap());
            let id = &admitted.receipt.envelope.command_id;
            let now = layer2_core::now_rfc3339();
            if claimed {
                let leased = store
                    .claim_next_command(claim_request(&agent, "compatibility", &now))
                    .await
                    .unwrap()
                    .unwrap();
                store
                    .reject_command(&leased.work.lease.unwrap().claim, code, now)
                    .await
                    .unwrap();
            } else {
                store
                    .reject_unclaimed_command(id, admitted.revision, code, now)
                    .await
                    .unwrap();
            }
            let raw = store
                .agent_commands
                .find_one(doc! { "_id": id.as_str() })
                .await
                .unwrap()
                .unwrap();
            let failure = raw
                .get_document("record")
                .unwrap()
                .get_document("work")
                .unwrap()
                .get_document("failure")
                .unwrap();
            assert_eq!(failure.get_str("code").unwrap(), "execution_failed");
            let predecessor: B1CommandFailureCode =
                mongodb::bson::deserialize_from_bson(failure.get("code").unwrap().clone()).unwrap();
            assert_eq!(predecessor, B1CommandFailureCode::ExecutionFailed);
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let retained = reopened.get_command(id).await.unwrap().unwrap();
            assert_eq!(
                retained.work.failure.as_ref().unwrap().code,
                CommandFailureCode::ExecutionFailed
            );
            assert_eq!(retained.status, AgentResponseStatus::Failed);
            assert!(retained.work.dispatch.is_none() && retained.work.lease.is_none());
            assert_eq!(retained.queue_deadline, admitted.queue_deadline);
            assert_eq!(retained.receipt, admitted.receipt);
            assert_eq!(
                record(reopened.reserve_command(reservation).await.unwrap()),
                retained
            );
            assert_eq!(
                reopened
                    .agent_commands
                    .find_one(doc! { "_id": id.as_str() })
                    .await
                    .unwrap()
                    .unwrap(),
                raw
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_precise_codes_remain_readable_without_rewriting_records() {
    let (client, name) = database("retained_refusal").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    for code in [
        CommandFailureCode::CredentialSetupRequired,
        CommandFailureCode::ConnectionsRequired,
    ] {
        let reservation = live_request(&agent, &parent, &format!("{code:?}"));
        let mut retained = record(store.reserve_command(reservation.clone()).await.unwrap());
        retained
            .reject_unclaimed(retained.revision, code, layer2_core::now_rfc3339())
            .unwrap();
        // Fixture for a record produced by the earlier writer, never a migration.
        retained.work.failure.as_mut().unwrap().code = code;
        let id = &retained.receipt.envelope.command_id;
        let mut raw = store
            .agent_commands
            .find_one(doc! { "_id": id.as_str() })
            .await
            .unwrap()
            .unwrap();
        raw.insert(
            "record",
            mongodb::bson::serialize_to_bson(&retained).unwrap(),
        );
        raw.insert("revision", i64::try_from(retained.revision).unwrap());
        raw.insert("status", "failed");
        store
            .agent_commands
            .replace_one(doc! { "_id": id.as_str() }, raw.clone())
            .await
            .unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(reopened.get_command(id).await.unwrap().unwrap(), retained);
        assert_eq!(
            record(reopened.reserve_command(reservation).await.unwrap()),
            retained
        );
        assert_eq!(
            reopened
                .agent_commands
                .find_one(doc! { "_id": id.as_str() })
                .await
                .unwrap()
                .unwrap(),
            raw
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[test]
fn session_capacity_failure_sidecar_preserves_the_predecessor_record_and_validates_precision() {
    use crate::store::agent_commands::records::{document, read};

    let parent = active_sandbox("sbx_capacity_compatibility");
    let agent = ready_agent(&parent, "agt_capacity_compatibility");
    let mut command = request(&agent, &parent, "capacity-compatibility")
        .record(1, 0, None)
        .unwrap();
    command
        .reject_unclaimed(
            command.revision,
            CommandFailureCode::SessionCapacityUnavailable,
            NOW.into(),
        )
        .unwrap();
    let row = document(&command).unwrap();
    assert_eq!(
        row.get_str("admission_capacity_failure").unwrap(),
        "session_capacity_unavailable"
    );
    let predecessor = row
        .get_document("record")
        .unwrap()
        .get_document("work")
        .unwrap()
        .get_document("failure")
        .unwrap();
    let code: B1CommandFailureCode =
        mongodb::bson::deserialize_from_bson(predecessor.get("code").unwrap().clone()).unwrap();
    assert_eq!(code, B1CommandFailureCode::ExecutionFailed);
    assert_eq!(read(&row).unwrap(), command);

    for marker in [
        Bson::Null,
        Bson::String("unknown_capacity".into()),
        Bson::Int32(1),
    ] {
        let mut corrupt = row.clone();
        corrupt.insert("admission_capacity_failure", marker);
        assert_eq!(read(&corrupt), Err(CommandLedgerError::Corrupt));
    }
    let mut conflicting = row.clone();
    conflicting.insert("model_budget_failure", "owner_exhausted");
    assert_eq!(read(&conflicting), Err(CommandLedgerError::Corrupt));
    let mut bound = row;
    bound.insert("status", "queued");
    assert_eq!(read(&bound), Err(CommandLedgerError::Corrupt));
}
