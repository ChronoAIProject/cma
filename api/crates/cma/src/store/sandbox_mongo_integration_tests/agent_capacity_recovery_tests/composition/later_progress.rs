//! Actual service restart after later publication. The native boundary is a
//! counted protocol stub with no input API, not production-native qualification.
use super::*;
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

#[derive(Default)]
struct CurrentNative {
    attaches: AtomicUsize,
    acknowledges: AtomicUsize,
    fail_ack: AtomicBool,
    stale_confirmation: AtomicBool,
    activations: Mutex<Vec<RecoveryActivation>>,
}

#[async_trait::async_trait]
impl CapacityRecoveryRuntime for CurrentNative {
    type Attachment = RecoveryActivation;

    async fn reattach_existing(
        &self,
        _: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        panic!("completed work must not repeat claim attachment or replay input")
    }

    async fn reattach_completed(
        &self,
        activation: &RecoveryActivation,
    ) -> Result<CompletedAttachment<Self::Attachment>, RecoveryError> {
        self.attaches.fetch_add(1, Ordering::SeqCst);
        self.activations.lock().unwrap().push(activation.clone());
        let mut confirmation = RecoveryConfirmation {
            operation_key: activation.completed().request.key()?,
            target: activation.target().clone(),
            authority_fence: activation.writer_fence(),
            native_state: ConfirmedNativeState::Idle,
        };
        if self.stale_confirmation.load(Ordering::SeqCst) {
            confirmation.authority_fence -= 1;
        }
        Ok(CompletedAttachment::Reattached {
            attachment: activation.clone(),
            confirmation: Box::new(confirmation),
        })
    }

    async fn acknowledge(
        &self,
        activation: Self::Attachment,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        assert_eq!(activation.completed(), completed);
        if self.fail_ack.swap(false, Ordering::SeqCst) {
            return Err(RecoveryError::Unavailable);
        }
        self.acknowledges.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn completed_before_ack(
    store: &MongoSandboxStore,
) -> (Caller, RecoveryRequest, RecoveryWork) {
    let (caller, request, _) = admitted(store).await;
    let native = Native::default();
    native.fail_ack.store(true, Ordering::SeqCst);
    assert_eq!(
        service(store, false)
            .run_accepted(&native, &request, "completion-before-crash")
            .await,
        Err(RecoveryError::Unavailable)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
    let RecoveryLookup::Recorded(work) = service(store, false)
        .lookup(&caller, &request)
        .await
        .unwrap()
    else {
        panic!("the crash must follow a durable completion")
    };
    assert_eq!(work.phase, RecoveryPhase::Completed);
    (caller, request, *work)
}

async fn progress(store: &MongoSandboxStore, request: &RecoveryRequest) {
    store
        .append_agent_log_keyed(
            &request.agent_id,
            &AgentLogPublicationIdentity {
                key: format!("post-completion:{}", uuid::Uuid::new_v4()),
                fingerprint: [7; 32],
                appended_at: layer2_core::now_rfc3339(),
            },
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
}

async fn completion_receipt(store: &MongoSandboxStore, work: &RecoveryWork) -> Document {
    store
        .segment_logs
        .publications
        .find_one(doc! {
            "publication_key": format!("{}:step:{}", work.request.key().unwrap(), work.revision)
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_later_service_retries_crash_and_refuses_foreign_or_stale_activation() {
    let (client, name) = database("capacity_service_later_crash").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, completed) = completed_before_ack(&store).await;
    let original = snapshot(&store, &request).await;
    let receipt = completion_receipt(&store, &completed).await;
    progress(&store, &request).await;
    let advanced = snapshot(&store, &request).await;
    assert_eq!(advanced.agent.revision, original.agent.revision);
    assert!(advanced.agent.conversation.last_seq > original.agent.conversation.last_seq);
    assert_eq!(advanced.carrier, original.carrier);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let service = service(&reopened, false);
    let native = CurrentNative::default();
    let mut foreign = request.clone();
    foreign.requested_at = NOW.into();
    assert_eq!(foreign.key().unwrap(), request.key().unwrap());
    assert_eq!(
        service.run_accepted(&native, &foreign, "foreign").await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 0);
    native.stale_confirmation.store(true, Ordering::SeqCst);
    assert_eq!(
        service.run_accepted(&native, &request, "stale").await,
        Err(RecoveryError::InvalidEvidence)
    );
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
    native.stale_confirmation.store(false, Ordering::SeqCst);
    native.fail_ack.store(true, Ordering::SeqCst);
    assert_eq!(
        service.run_accepted(&native, &request, "ack-crash").await,
        Err(RecoveryError::Unavailable)
    );
    assert_eq!(snapshot(&store, &request).await, advanced);
    assert_eq!(
        service
            .run_accepted(&native, &request, "retry")
            .await
            .unwrap(),
        CommitOutcome::Recorded(completed.clone())
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 3);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
    for activation in native.activations.lock().unwrap().iter() {
        assert_eq!(activation.completed(), &completed);
        assert_eq!(activation.agent(), &advanced.agent);
        assert_eq!(
            activation.writer_fence(),
            advanced.agent.work.next_fence - 1
        );
    }
    assert_eq!(
        service.lookup(&caller, &request).await.unwrap(),
        RecoveryLookup::Recorded(Box::new(completed.clone()))
    );
    assert_eq!(snapshot(&store, &request).await, advanced);
    assert_eq!(completion_receipt(&store, &completed).await, receipt);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn capacity_later_service_unknown_claim_never_activates_and_retry_uses_original_completion() {
    for mode in ["committed", "uncommitted"] {
        let (admin, client, name, app) = super::faults::clients().await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, request, completed) = completed_before_ack(&store).await;
        progress(&store, &request).await;
        let before = snapshot(&store, &request).await;
        let receipt = completion_receipt(&store, &completed).await;
        let native = CurrentNative::default();
        super::faults::failpoint(&admin, &app, mode).await;
        let result = service(&store, false)
            .run_accepted(&native, &request, "uncertain-terminal")
            .await;
        super::faults::disable(&admin).await;
        assert_eq!(result, Ok(CommitOutcome::Unknown), "{mode}");
        assert_eq!(native.attaches.load(Ordering::SeqCst), 0);
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
        assert_eq!(snapshot(&store, &request).await, before);
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            service(&reopened, false)
                .run_accepted(&native, &request, "exact-retry")
                .await
                .unwrap(),
            CommitOutcome::Recorded(completed.clone())
        );
        assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
        assert_eq!(snapshot(&store, &request).await, before);
        assert_eq!(completion_receipt(&store, &completed).await, receipt);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_later_service_requires_the_exact_sequence_only_publication() {
    let (client, name) = database("capacity_service_sequence_proof").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, request, completed) = completed_before_ack(&store).await;
    let original = snapshot(&store, &request).await;
    let receipt = completion_receipt(&store, &completed).await;
    progress(&store, &request).await;
    let current = snapshot(&store, &request).await;
    assert_eq!(current.agent.revision, original.agent.revision);
    assert!(current.agent.conversation.last_seq > original.agent.conversation.last_seq);
    let publications = &store.segment_logs.publications;
    let witness = publications
        .find_one(doc! {
            "agent_id": request.agent_id.as_str(),
            "result_revision": current.agent.revision as i64,
            "last_seq": current.agent.conversation.last_seq as i64,
        })
        .await
        .unwrap()
        .unwrap();
    assert_ne!(witness.get_str("_id").unwrap(), receipt.get_str("_id").unwrap());
    let id = witness.get_str("_id").unwrap();
    let service = service(&store, false);
    let native = CurrentNative::default();
    // Establish a usable witness before fault injection so refusal cannot pass
    // merely because another prerequisite already makes activation impossible.
    service.run_accepted(&native, &request, "before").await.unwrap();
    for variant in 0..3 {
        publications.delete_one(doc! {"_id": id}).await.unwrap();
        if variant != 0 {
            let mut changed = witness.clone();
            if variant == 1 {
                changed.insert("format_version", 1_i64);
                changed.remove("agent_digest");
            } else {
                changed.insert("agent_digest", mongodb::bson::Binary {
                    subtype: mongodb::bson::spec::BinarySubtype::Generic,
                    bytes: vec![0; 32],
                });
            }
            publications.insert_one(changed).await.unwrap();
        }
        assert!(service.run_accepted(&native, &request, "unproved").await.is_err());
        assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
        publications.delete_one(doc! {"_id": id}).await.unwrap();
        publications.insert_one(witness.clone()).await.unwrap();
        assert_eq!(snapshot(&store, &request).await, current);
    }
    assert_eq!(
        service.run_accepted(&native, &request, "restored").await.unwrap(),
        CommitOutcome::Recorded(completed.clone())
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 2);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 2);
    assert_eq!(snapshot(&store, &request).await, current);
    assert_eq!(completion_receipt(&store, &completed).await, receipt);
    client.database(&name).drop().await.unwrap();
}
