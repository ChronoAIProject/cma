//! Real accepted-work service/worker composition. Native IO is a counted protocol
//! stub; these tests do not qualify the production native recovery boundary.
use super::*;
use crate::store::agent_capacity_recovery::{records, RecoveryService};
use layer2_core::sandbox::agent_capacity_recovery::{
    activation::RecoveryActivation,
    ports::{
        CapacityRecoveryRuntime, CommitOutcome, CompletedAttachment, RecoveryError, RecoveryLookup,
    },
    service::CapacityRecoveryFeatures,
    state::{
        ConfirmedNativeState, RecoveryClaim, RecoveryConfirmation, RecoveryPhase, RecoveryRequest,
        RecoveryWork,
    },
};
use std::sync::atomic::{AtomicBool, AtomicUsize};

mod faults;

#[derive(Default)]
struct Native {
    attaches: AtomicUsize,
    restarts: AtomicUsize,
    acknowledges: AtomicUsize,
    fail_ack: AtomicBool,
}

#[async_trait::async_trait]
impl CapacityRecoveryRuntime for Native {
    type Attachment = (String, u64);

    async fn reattach_existing(
        &self,
        claim: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        self.attaches.fetch_add(1, Ordering::SeqCst);
        let key = claim.work.request.key()?;
        let fence = claim.work.lease.as_ref().unwrap().fence;
        Ok((
            (key.clone(), fence),
            RecoveryConfirmation {
                operation_key: key,
                target: claim.work.target.clone(),
                authority_fence: fence,
                native_state: ConfirmedNativeState::Idle,
            },
        ))
    }

    async fn reattach_completed(
        &self,
        activation: &RecoveryActivation,
    ) -> Result<CompletedAttachment<Self::Attachment>, RecoveryError> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        let key = activation.completed().request.key()?;
        let fence = activation.writer_fence();
        Ok(CompletedAttachment::Reattached {
            attachment: (key.clone(), fence),
            confirmation: Box::new(RecoveryConfirmation {
                operation_key: key,
                target: activation.completed().target.clone(),
                authority_fence: fence,
                native_state: ConfirmedNativeState::Idle,
            }),
        })
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

fn service(store: &MongoSandboxStore, enabled: bool) -> RecoveryService {
    RecoveryService::new(store.clone(), transactions::access(store))
        .with_features(CapacityRecoveryFeatures { enabled })
}

async fn admitted(store: &MongoSandboxStore) -> (Caller, RecoveryRequest, RecoveryWork) {
    let agent = fixture::capacity_fixture(store).await;
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
    let CommitOutcome::Recorded(work) =
        service(store, true).admit(&caller, &request).await.unwrap()
    else {
        panic!("fixture admission must publish its exact work receipt");
    };
    assert_eq!(work.phase, RecoveryPhase::Admitted);
    assert_eq!(
        records::collection(store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    (caller, request, work)
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    agent: Agent,
    carrier: Document,
    publications: u64,
}

async fn snapshot(store: &MongoSandboxStore, request: &RecoveryRequest) -> Snapshot {
    // Independent majority reads can observe different stable timestamps after
    // a committed-but-unknown result. Assert atomic state from one snapshot and
    // finish the read with majority concern before trusting the observation.
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":request.agent_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let snapshot = Snapshot {
        agent: crate::store::agents::read_agent(&row).unwrap(),
        carrier: records::collection(store)
            .find_one(doc! {"_id":request.key().unwrap()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap(),
        publications: store
            .segment_logs
            .publications
            .count_documents(doc! {})
            .session(&mut session)
            .await
            .unwrap(),
    };
    session.commit_transaction().await.unwrap();
    snapshot
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_composition_presence_is_read_only_before_optional_migration() {
    let (client, name) = database("capacity_composition_presence").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let db = client.database(&name);
    let mut before = db.list_collection_names().await.unwrap();
    before.sort();
    assert!(!store.capacity_recovery_work_exists().await.unwrap());
    assert!(!store.capacity_recovery_work_exists().await.unwrap());
    let mut after = db.list_collection_names().await.unwrap();
    after.sort();
    assert_eq!(
        before, after,
        "scheduler presence must not install optional schema"
    );
    crate::store::agent_log_segments::schema::migrate(&db)
        .await
        .unwrap();
    assert!(store.capacity_recovery_work_exists().await.unwrap());
    assert_eq!(
        records::collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_composition_accepts_and_settles_with_new_admission_disabled() {
    let (client, name) = database("capacity_composition_success").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, original) = admitted(&store).await;
    let before = snapshot(&store, &request).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let service = service(&reopened, false);
    assert_eq!(
        service.admit(&caller, &request).await.unwrap(),
        CommitOutcome::Recorded(original)
    );
    assert_eq!(snapshot(&store, &request).await, before);
    let native = Native::default();
    let CommitOutcome::Recorded(completed) = service
        .run_accepted(&native, &request, "accepted")
        .await
        .unwrap()
    else {
        panic!("accepted worker must settle the original request");
    };
    assert_eq!(completed.phase, RecoveryPhase::Completed);
    assert_eq!(completed.request, request);
    assert_eq!(completed.attempts, 1);
    assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
    assert_eq!(native.restarts.load(Ordering::SeqCst), 0);
    assert_eq!(
        service.lookup(&caller, &request).await.unwrap(),
        RecoveryLookup::Recorded(Box::new(completed))
    );
    let status = service
        .status(&caller, &request.agent_id, Some(&request))
        .await
        .unwrap();
    assert_eq!(status.phase, Some(RecoveryPhase::Completed));
    assert_eq!(status.attempts, 1);
    assert_eq!(status.operation_key, Some(request.key().unwrap()));
    let after = snapshot(&store, &request).await;
    assert_eq!(after.publications, before.publications + 2);
    assert_eq!(after.agent.revision, before.agent.revision + 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_composition_refuses_foreign_receipts_and_changed_authority_before_io() {
    let (client, name) = database("capacity_composition_refusal").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, _) = admitted(&store).await;
    let service = service(&store, false);
    let before = snapshot(&store, &request).await;
    let native = Native::default();
    let mut foreign = request.clone();
    foreign.requested_at = NOW.into();
    assert_eq!(foreign.key().unwrap(), request.key().unwrap());
    assert_eq!(
        service.run_accepted(&native, &foreign, "foreign").await,
        Err(RecoveryError::Conflict)
    );
    let intruder = Caller::new(
        SubjectId::new("foreign"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert!(matches!(
        service
            .status(&intruder, &request.agent_id, Some(&request))
            .await,
        Err(RecoveryError::Unauthorized)
    ));
    assert_eq!(snapshot(&store, &request).await, before);
    let mut changed = before.agent.clone();
    changed.owner = SubjectId::new("replacement-owner");
    changed.revision += 1;
    replace_agent_fixture(&store, &changed).await;
    let after = snapshot(&store, &request).await;
    assert_eq!(
        service.run_accepted(&native, &request, "changed").await,
        // Inconsistent current ownership prevents obtaining a lineage snapshot.
        Err(RecoveryError::Unavailable)
    );
    assert_eq!(
        service.lookup(&caller, &request).await,
        Err(RecoveryError::Unauthorized)
    );
    assert_eq!(snapshot(&store, &request).await, after);
    assert_eq!(native.attaches.load(Ordering::SeqCst), 0);
    assert_eq!(native.restarts.load(Ordering::SeqCst), 0);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_composition_ack_crash_reopens_exact_completion_without_republication() {
    let (client, name) = database("capacity_composition_crash").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, _) = admitted(&store).await;
    let native = Native::default();
    native.fail_ack.store(true, Ordering::SeqCst);
    assert_eq!(
        service(&store, false)
            .run_accepted(&native, &request, "first")
            .await,
        Err(RecoveryError::Unavailable)
    );
    let completed = snapshot(&store, &request).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let service = service(&reopened, false);
    let CommitOutcome::Recorded(work) = service
        .run_accepted(&native, &request, "restarted")
        .await
        .unwrap()
    else {
        panic!("restart must reconcile the exact durable completion");
    };
    assert_eq!(work.phase, RecoveryPhase::Completed);
    assert_eq!(work.attempts, 1);
    assert_eq!(
        service.lookup(&caller, &request).await.unwrap(),
        RecoveryLookup::Recorded(Box::new(work))
    );
    assert_eq!(snapshot(&store, &request).await, completed);
    assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(native.restarts.load(Ordering::SeqCst), 1);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}
