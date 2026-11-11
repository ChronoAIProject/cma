//! Durable Mongo completion/restart evidence; the native port is a counted stub.
//! Production native proof remains Unsupported and requires separate qualification.
use super::*;
use crate::store::agent_capacity_recovery::{records, RecoveryService};
use layer2_core::sandbox::agent_capacity_recovery::{
    activation::RecoveryActivation,
    ports::{CapacityRecoveryRuntime, ClaimOutcome, CommitOutcome, RecoveryError},
    service::CapacityRecoveryFeatures,
    state::{
        ConfirmedNativeState, RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork,
    },
};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, SystemTime};

#[derive(Default)]
struct Native {
    attaches: AtomicUsize,
    acknowledges: AtomicUsize,
    fail_ack: AtomicBool,
}
#[async_trait::async_trait]
impl CapacityRecoveryRuntime for Native {
    type Attachment = (String, u64);
    async fn reattach_existing(
        &self,
        _: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        panic!("completed restart must never repeat the original claim or runtime step")
    }
    async fn reattach_completed(
        &self,
        activation: &RecoveryActivation,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        self.attaches.fetch_add(1, Ordering::SeqCst);
        let work = activation.completed();
        let key = work.request.key()?;
        Ok((
            (key.clone(), activation.writer_fence()),
            RecoveryConfirmation {
                operation_key: key,
                target: work.target.clone(),
                authority_fence: activation.writer_fence(),
                native_state: ConfirmedNativeState::Idle,
            },
        ))
    }
    async fn acknowledge(
        &self,
        (key, fence): Self::Attachment,
        work: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        assert_eq!(key, work.request.key()?);
        assert_eq!(fence + 1, work.next_fence);
        if self.fail_ack.swap(false, Ordering::SeqCst) {
            return Err(RecoveryError::Unavailable);
        }
        self.acknowledges.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_completed_mongo_restart_retries_activation_without_republication() {
    let (client, name) = database("capacity_completed_restart").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = fixture::capacity_fixture(&store).await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let request = RecoveryRequest {
        agent_id: agent.id.clone(),
        creation_key: agent.creation_key.clone(),
        creation_fingerprint: agent.creation_fingerprint.clone(),
        failure_seq: agent.conversation.last_seq,
        requested_at: layer2_core::now_rfc3339(),
    };
    let service = RecoveryService::new(store.clone(), transactions::access(&store))
        .with_features(CapacityRecoveryFeatures { enabled: true });
    assert!(matches!(
        service.admit(&caller, &request).await.unwrap(),
        CommitOutcome::Recorded(_)
    ));
    let at = SystemTime::now();
    let now = layer2_core::rfc3339_at(at);
    let expiry = layer2_core::rfc3339_at(at + Duration::from_secs(60));
    let ClaimOutcome::Claimed(claim) = service
        .claim(&caller, &request, "original", &now, &expiry)
        .await
        .unwrap()
    else {
        panic!("claim must be durable");
    };
    let confirmation = RecoveryConfirmation {
        operation_key: request.key().unwrap(),
        target: claim.work.target.clone(),
        authority_fence: claim.work.lease.as_ref().unwrap().fence,
        native_state: ConfirmedNativeState::Idle,
    };
    let CommitOutcome::Recorded(completed) = service
        .complete(&caller, &claim, &confirmation, &now)
        .await
        .unwrap()
    else {
        panic!("completion must be durable");
    };
    // Crash before activation; no in-memory attachment survives this point.
    let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
    let carrier = records::collection(&store)
        .find_one(doc! {"_id": request.key().unwrap()})
        .await
        .unwrap();
    let publications = store
        .segment_logs
        .publications
        .count_documents(doc! {})
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let restarted = RecoveryService::new(reopened.clone(), transactions::access(&reopened));
    let native = Native::default();
    native.fail_ack.store(true, Ordering::SeqCst);
    assert_eq!(
        restarted
            .run_one(&native, &caller, &request, "restart")
            .await,
        Err(RecoveryError::Unavailable)
    );
    assert_eq!(
        restarted
            .run_one(&native, &caller, &request, "restart")
            .await
            .unwrap(),
        CommitOutcome::Recorded(completed)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 2);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(saved));
    assert_eq!(
        records::collection(&store)
            .find_one(doc! {"_id": request.key().unwrap()})
            .await
            .unwrap(),
        carrier
    );
    assert_eq!(
        store
            .segment_logs
            .publications
            .count_documents(doc! {})
            .await
            .unwrap(),
        publications
    );
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    parent.status = SandboxStatus::Retiring;
    parent.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! {"_id": parent.id.as_str()},
            doc! {"$set":
            crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    assert_eq!(
        restarted
            .run_one(&native, &caller, &request, "restart")
            .await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 2);
    client.database(&name).drop().await.unwrap();
}
