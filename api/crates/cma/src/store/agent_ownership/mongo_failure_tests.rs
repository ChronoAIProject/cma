use super::*;
use layer3_adapter::owner_protocol::{OwnershipFailureReason as Reason, OwnershipPhase as Phase};
use std::sync::Arc;

#[path = "predecessor_reader.rs"]
mod predecessor;
use predecessor::PredecessorLease;

#[path = "mongo_failure_transport_tests.rs"]
mod transport;

async fn row(store: &MongoAgentOwnership, token: &OwnershipToken) -> Document {
    store
        .owners
        .find_one(doc! {"_id": token.agent_id.as_str()})
        .await
        .unwrap()
        .unwrap()
}

async fn owned() -> (MongoAgentOwnership, OwnershipLease) {
    let mut store = fixture().await;
    store.receiver_diagnostics_enabled = true;
    store.register(member("diagnostic-member")).await.unwrap();
    let owner = store
        .acquire(&AgentId("diagnostic-agent".into()), "diagnostic-member")
        .await
        .unwrap()
        .unwrap();
    (store, owner)
}

fn first() -> OwnershipFailure {
    OwnershipFailure {
        phase: Phase::Challenge,
        reason: Reason::LeaseMismatch,
        http_status: Some(200),
        protocol: Some(1),
        lease_ms: Some(u64::MAX),
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn new_writer_diagnostic_is_durable_and_decodes_with_the_predecessor_reader() {
    let (store, owner) = owned().await;
    let before = row(&store, &owner.token).await;
    store
        .record_receiver_failure(&owner.token, first())
        .await
        .unwrap();
    // A separate client represents a restarted manager and uses majority reads.
    let restarted = MongoAgentOwnership::open(
        &Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap(),
        store.database.name(),
    )
    .await
    .unwrap();
    let mut after = row(&restarted, &owner.token).await;
    let previous = PredecessorLease::decode(&after).unwrap();
    assert_eq!(
        serde_json::to_value(previous).unwrap(),
        serde_json::to_value(&owner).unwrap()
    );
    assert_eq!(owner_from(&after).unwrap(), owner);
    let diagnostic = after.remove("receiver_failure").unwrap();
    assert_eq!(
        after, before,
        "diagnostics cannot change any predecessor authority field"
    );
    let diagnostic = diagnostic.as_document().unwrap();
    assert_eq!(diagnostic.get_str("reason").unwrap(), "lease_mismatch");
    assert_eq!(diagnostic.get_i32("http_status").unwrap(), 200);
    assert_eq!(
        diagnostic.get_decimal128("lease_ms").unwrap().to_string(),
        u64::MAX.to_string()
    );
    assert!(diagnostic.get_datetime("observed_at").is_ok());
    assert!(!diagnostic.to_string().contains("diagnostic-member"));
    assert!(!diagnostic.to_string().contains("diagnostic-agent"));
    // The frozen reader still rejects an invalid predecessor record.
    let mut malformed = after.clone();
    malformed.remove("epoch");
    assert!(PredecessorLease::decode(&malformed).is_err());
    store
        .record_receiver_failure(&owner.token, OwnershipFailure::http(Phase::Revoke, 403))
        .await
        .unwrap();
    assert_eq!(
        row(&store, &owner.token)
            .await
            .get_document("receiver_failure")
            .unwrap(),
        diagnostic
    );
    let renewed = store.renew(&owner.token).await.unwrap();
    assert_eq!(
        serde_json::to_value(PredecessorLease::decode(&row(&store, &owner.token).await).unwrap())
            .unwrap(),
        serde_json::to_value(renewed).unwrap()
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn concurrent_failures_retain_one_first_cause_and_reject_every_wrong_token() {
    let (store, owner) = owned().await;
    let original = row(&store, &owner.token).await;
    for field in ["agent", "instance", "epoch", "protocol"] {
        let mut wrong = owner.token.clone();
        match field {
            "agent" => wrong.agent_id = AgentId("other-agent".into()),
            "instance" => wrong.instance_id = "other-member".into(),
            "epoch" => wrong.epoch += 1,
            _ => wrong.protocol += 1,
        }
        assert!(store
            .record_receiver_failure(&wrong, first())
            .await
            .is_err());
        assert_eq!(row(&store, &owner.token).await, original);
    }
    let failures = [first(), OwnershipFailure::http(Phase::Confirm, 401)];
    let (left, right) = tokio::join!(
        store.record_receiver_failure(&owner.token, failures[0]),
        store.record_receiver_failure(&owner.token, failures[1]),
    );
    left.unwrap();
    right.unwrap();
    let recorded = row(&store, &owner.token).await;
    let cause = recorded.get_document("receiver_failure").unwrap();
    assert!(matches!(
        cause.get_str("reason").unwrap(),
        "lease_mismatch" | "http_auth_refused"
    ));
    for failure in failures {
        store
            .record_receiver_failure(&owner.token, failure)
            .await
            .unwrap();
        assert_eq!(row(&store, &owner.token).await, recorded);
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn late_failure_cannot_write_after_release_expiry_or_successor_acquisition() {
    let (store, owner) = owned().await;
    store
        .record_receiver_failure(&owner.token, first())
        .await
        .unwrap();
    store.release(&owner.token).await.unwrap();
    let released = row(&store, &owner.token).await;
    assert_eq!(
        store.record_receiver_failure(&owner.token, first()).await,
        Err(OwnershipError::Stale)
    );
    assert_eq!(row(&store, &owner.token).await, released);
    let successor = store
        .acquire(&owner.token.agent_id, &owner.token.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > owner.token.epoch);
    let failure = OwnershipFailure::http(Phase::Confirm, 404);
    store
        .record_receiver_failure(&successor.token, failure)
        .await
        .unwrap();
    let current = row(&store, &successor.token).await;
    assert_eq!(
        current
            .get_document("receiver_failure")
            .unwrap()
            .get_i64("owner_epoch")
            .unwrap(),
        successor.token.epoch as i64
    );
    assert_eq!(
        store.record_receiver_failure(&owner.token, first()).await,
        Err(OwnershipError::Stale)
    );
    assert_eq!(row(&store, &successor.token).await, current);
    expire(&store, &successor.token).await;
    let expired = row(&store, &successor.token).await;
    assert_eq!(
        store
            .record_receiver_failure(&successor.token, first())
            .await,
        Err(OwnershipError::Stale)
    );
    assert_eq!(row(&store, &successor.token).await, expired);
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn default_off_diagnostics_preserve_the_owner_record_and_refusal() {
    let store = Arc::new(fixture().await);
    let manager = layer2_core::agent_ownership::OwnershipCoordinator::register(
        store.clone(),
        member("disabled-member"),
    )
    .await
    .unwrap();
    let authority = manager
        .acquire(&AgentId("disabled-agent".into()))
        .await
        .unwrap()
        .unwrap();
    let before = row(&store, &authority.token).await;
    assert!(
        store
            .record_receiver_failure(&authority.token, first())
            .await
            .is_err(),
        "a disabled writer must not acknowledge a durable record"
    );
    // With replicas active but the diagnostic switch unset, no new write or
    // retained observation occurs. The handshake must still fail and cancel.
    let (transport, server) = transport::runtime().await;
    let result = authority
        .run(async {
            use layer3_adapter::hosted::HostedControlTransport;
            let _ = transport
                .exchange(
                    &layer3_adapter::RuntimeHandle::new("fixture-runtime"),
                    layer3_adapter::wire::RequestFrame::Resources,
                )
                .await;
            std::future::pending::<()>().await
        })
        .await;
    assert_eq!(result, Err(OwnershipError::Stale));
    assert!(authority.receiver_cancelled());
    assert_eq!(row(&store, &authority.token).await, before);
    assert!(authority.receiver_failure().is_none());
    assert!(authority.revoke_transports().await.is_err());
    server.await.unwrap();
    store.database.drop().await.unwrap();
}
