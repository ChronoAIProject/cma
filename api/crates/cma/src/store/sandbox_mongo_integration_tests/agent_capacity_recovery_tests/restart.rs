//! Durable Mongo completion/restart evidence; the native port is a counted stub.
//! Production native proof remains Unsupported and requires separate qualification.
use super::*;
use crate::sandbox::binding_lifecycle_test_support::{
    AgentWorker, AgentWorkerConfig, AgentWorkerDependencies, CapacityRecoveryReconnect,
    DurableAgentConnectionLossHandler, EmptyAgentSkillResolver, LiveAgentAttachmentIdentity,
    LiveAgentConnectionLossHandler, ReconnectAction, RecoveryReconnect,
};
use crate::sandbox::LiveAgentRegistry;
use crate::store::agent_capacity_recovery::{records, RecoveryService, RestartDecision};
use layer2_core::sandbox::agent_capacity_recovery::{
    activation::RecoveryActivation,
    ports::{
        CapacityRecoveryRuntime, ClaimOutcome, CommitOutcome, CompletedAttachment, RecoveryError,
    },
    service::CapacityRecoveryFeatures,
    state::{
        ConfirmedNativeState, RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork,
    },
    CapacityRecoveryDisposition,
};
use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

mod later;

#[derive(Clone, Default)]
struct Native {
    attaches: Arc<AtomicUsize>,
    acknowledges: Arc<AtomicUsize>,
    fail_ack: Arc<AtomicBool>,
    stale_confirmation: Arc<AtomicBool>,
    activations: Arc<std::sync::Mutex<Vec<RecoveryActivation>>>,
}
#[async_trait::async_trait]
impl CapacityRecoveryRuntime for Native {
    type Attachment = RecoveryActivation;
    async fn reattach_existing(
        &self,
        _: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        panic!("completed restart must never repeat the original claim or runtime step")
    }
    async fn reattach_completed(
        &self,
        activation: &RecoveryActivation,
    ) -> Result<CompletedAttachment<Self::Attachment>, RecoveryError> {
        self.attaches.fetch_add(1, Ordering::SeqCst);
        self.activations.lock().unwrap().push(activation.clone());
        let work = activation.completed();
        let key = work.request.key()?;
        let stale = self.stale_confirmation.load(Ordering::SeqCst);
        let target = if stale {
            &work.target
        } else {
            activation.target()
        };
        let native_state = if stale {
            ConfirmedNativeState::Idle
        } else {
            match activation.agent().status {
                AgentStatus::Ready => ConfirmedNativeState::Idle,
                AgentStatus::Working => ConfirmedNativeState::Working,
                AgentStatus::WaitingApproval => ConfirmedNativeState::WaitingApproval,
                _ => panic!("activation must require a current live native state"),
            }
        };
        Ok(CompletedAttachment::Reattached {
            attachment: activation.clone(),
            confirmation: Box::new(RecoveryConfirmation {
                operation_key: key,
                target: target.clone(),
                authority_fence: if stale {
                    work.next_fence - 1
                } else {
                    activation.writer_fence()
                },
                native_state,
            }),
        })
    }
    async fn acknowledge(
        &self,
        attachment: Self::Attachment,
        work: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        assert_eq!(attachment.completed(), work);
        if self.fail_ack.swap(false, Ordering::SeqCst) {
            return Err(RecoveryError::Unavailable);
        }
        self.acknowledges.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn assert_cohort_count(rendered: &str, stage: &str, outcome: &str, count: u64) {
    assert!(
        rendered.lines().any(|line| {
            line.starts_with("cma_capacity_recovery_cohort_total{")
                && line.contains(&format!("stage=\"{stage}\""))
                && line.contains(&format!("outcome=\"{outcome}\""))
                && line.ends_with(&format!(" {count}"))
        }),
        "expected {stage}/{outcome} to be {count}\n{rendered}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_completed_mongo_restart_retries_activation_without_republication() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recorder = metrics::set_default_local_recorder(&recorder);
    let (client, name) = database("capacity_completed_restart").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(!store.segment_adoption_enabled);
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
    let service = Arc::new(
        RecoveryService::new(store.clone(), transactions::access(&store))
            .with_features(CapacityRecoveryFeatures { enabled: true }),
    );
    let dry_run = service.status(&caller, &agent.id, None).await.unwrap();
    assert_eq!(
        dry_run.disposition,
        CapacityRecoveryDisposition::EvidenceRequired
    );
    assert_eq!(dry_run.phase, None);
    let observed = metrics.render();
    assert_cohort_count(&observed, "scanned", "observed", 1);
    assert_cohort_count(&observed, "eligible", "observed", 1);
    let legacy_native = Native::default();
    let legacy_router =
        RecoveryReconnect::new(service.clone(), legacy_native.clone(), "legacy".into());
    assert_eq!(
        legacy_router.route(&agent.id).await.unwrap(),
        ReconnectAction::Ordinary
    );
    assert_eq!(legacy_native.attaches.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .run_accepted(&Native::default(), &request, "no-receipt")
            .await,
        Err(RecoveryError::Conflict)
    );
    assert!(matches!(
        service.admit(&caller, &request).await.unwrap(),
        CommitOutcome::Recorded(_)
    ));
    assert!(matches!(
        service.restart_decision(&agent.id).await.unwrap(),
        RestartDecision::Deferred
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
    let restarted = Arc::new(RecoveryService::new(
        reopened.clone(),
        transactions::access(&reopened),
    ));
    assert!(matches!(
        restarted.admit(&caller, &request).await.unwrap(),
        CommitOutcome::Recorded(_)
    ));
    assert_eq!(
        restarted.reconcile(&caller, &completed).await.unwrap(),
        CommitOutcome::Recorded(completed.clone())
    );
    let rendered = metrics.render();
    for (stage, outcome) in [
        ("completed", "committed"),
        ("completed", "replay"),
        ("admitted", "committed"),
        ("admitted", "replay"),
    ] {
        assert_cohort_count(&rendered, stage, outcome, 1);
    }
    let native = Native::default();
    let router = Arc::new(RecoveryReconnect::new(
        restarted.clone(),
        native.clone(),
        "restart".into(),
    ));
    let registry = Arc::new(LiveAgentRegistry::default());
    let worker_store = Arc::new(reopened.clone());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: worker_store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            updates: DurableAgentUpdateRecorder::new(worker_store.clone()),
            operations: AgentOperationService::new(worker_store, registry.clone()),
            registry,
        },
        AgentWorkerConfig::new("restart", "capacity-test"),
    );
    worker.install_capacity_restart_for_test(router.clone());
    // The actual restart discovery must include this completed schema-v2 Agent
    // even though completion leaves no pending ordinary work.
    let live = reopened
        .live_agent_connections(&PageRequest::new(32, None).unwrap())
        .await
        .unwrap();
    assert!(live
        .items
        .iter()
        .any(|candidate| candidate.agent_id == agent.id));
    native.fail_ack.store(true, Ordering::SeqCst);
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0,
        "failed activation must defer without scheduling ordinary replay"
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(saved.clone())
    );
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        1,
        "the next real worker sweep must finish completed activation"
    );
    // The real loss handler must route the exact completion through recovery,
    // never schedule ordinary PublishConnection or change the durable Agent.
    let route: Arc<dyn CapacityRecoveryReconnect> = router.clone();
    let installed = Arc::new(OnceLock::from(route));
    let handler = DurableAgentConnectionLossHandler::with_capacity_recovery(
        Arc::new(reopened.clone()),
        installed,
    );
    handler
        .connection_lost(LiveAgentAttachmentIdentity {
            agent_id: agent.id.clone(),
            conversation_id: completed.target.conversation_id.clone(),
            runtime_generation: completed.target.conversation_generation,
            authority_fence: completed.next_fence - 1,
            nonce: 1,
        })
        .await;
    assert_eq!(
        restarted.lookup(&caller, &request).await.unwrap(),
        layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryLookup::Recorded(Box::new(
            completed
        ))
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 3);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 2);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(saved.clone())
    );
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
    // A different original timestamp shares an operation key but must not gain
    // receipt-bound background permission.
    let mut different = request.clone();
    different.requested_at = "2026-11-11T00:00:00Z".into();
    assert_eq!(
        restarted.run_accepted(&native, &different, "restart").await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 3);
    // A later Agent revision no longer has exact original-result authority,
    // even if its incarnation and native conversation remain unchanged.
    let mut advanced = saved.clone();
    advanced.revision += 1;
    replace_agent_fixture(&store, &advanced).await;
    assert!(router.route(&agent.id).await.is_err());
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(advanced));
    assert_eq!(
        restarted.run_accepted(&native, &request, "restart").await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 3);
    replace_agent_fixture(&store, &saved).await;
    // A current organization sidecar must never become personal accepted-work
    // authority just because it uses the same subject text as the old owner.
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let policy = layer2_core::resource_access::ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: layer2_core::resource_access::ResourceOwner::Organization {
            subject: parent.owner.clone(),
        },
        organization_evidence: Some(
            layer2_core::resource_access::OrganizationOwnershipEvidence {
                issuer: "fixture".into(),
                organization: parent.owner.clone(),
                admitted_by: layer2_core::resource_access::ResourceActor::Human {
                    subject: SubjectId::new("fixture"),
                },
                verified_at: NOW.into(),
            },
        ),
        revision: 1,
    };
    store
        .resource_policies
        .insert_one(crate::store::resource_access::records::policy_document(&policy).unwrap())
        .await
        .unwrap();
    assert_eq!(
        restarted.run_accepted(&native, &request, "restart").await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 3);
    store
        .resource_policies
        .delete_one(doc! {"_id": parent.id.as_str()})
        .await
        .unwrap();
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
        restarted.run_accepted(&native, &request, "restart").await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(saved));
    assert_eq!(native.attaches.load(Ordering::SeqCst), 3);
    client.database(&name).drop().await.unwrap();
}
