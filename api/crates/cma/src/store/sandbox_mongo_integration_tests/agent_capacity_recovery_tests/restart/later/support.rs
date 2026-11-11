use super::*;
use crate::store::agent_log_integrity::AppendCapacity;
use crate::store::agent_log_segments::{
    capture, publication, transaction_scope::TransactionScope, writer_commit,
};
use crate::store::agents::agent_replacement_fields;
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

pub(super) async fn complete(store: &MongoSandboxStore) -> (RecoveryRequest, RecoveryWork) {
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
    let service = RecoveryService::new(store.clone(), transactions::access(store))
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
        panic!("durable claim")
    };
    let confirmation = RecoveryConfirmation {
        operation_key: request.key().unwrap(),
        target: claim.work.target.clone(),
        authority_fence: claim.work.lease.as_ref().unwrap().fence,
        native_state: ConfirmedNativeState::Idle,
    };
    let CommitOutcome::Recorded(work) = service
        .complete(&caller, &claim, &confirmation, &now)
        .await
        .unwrap()
    else {
        panic!("durable completion")
    };
    (request, work)
}

pub(super) fn worker(
    store: &MongoSandboxStore,
    router: Arc<dyn CapacityRecoveryReconnect>,
) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    let store = Arc::new(store.clone());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry.clone()),
            registry,
        },
        AgentWorkerConfig::new("current-restart", "capacity-test"),
    );
    worker.install_capacity_restart_for_test(router);
    worker
}

pub(super) async fn progress(
    store: &MongoSandboxStore,
    request: &RecoveryRequest,
    turn: bool,
) -> Agent {
    let now = layer2_core::now_rfc3339();
    if !turn {
        store
            .append_agent_log_keyed(
                &request.agent_id,
                &AgentLogPublicationIdentity {
                    key: format!("later-output:{}", uuid::Uuid::new_v4()),
                    fingerprint: [9; 32],
                    appended_at: now,
                },
                AgentLogKind::ConversationGap { frames: 1 },
            )
            .await
            .unwrap();
        return store.get_agent(&request.agent_id).await.unwrap().unwrap();
    }
    let current = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let mut next = current.clone();
    next.revision += 1;
    next.updated_at = now.clone();
    next.work.next_fence += 1;
    next.conversation.managed_guidance_revision += 1;
    next.status = AgentStatus::Working;
    next.conversation.active_turn = Some(current.conversation.last_seq + 1);
    next.conversation.last_seq += 3;
    let logs = vec![
        AgentLogEntry {
            agent_id: current.id.clone(),
            seq: current.conversation.last_seq + 1,
            appended_at: now.clone(),
            kind: AgentLogKind::TurnStarted {
                turn_sequence: current.conversation.last_seq + 1,
                model: None,
                origin_key: None,
            },
        },
        AgentLogEntry {
            agent_id: current.id.clone(),
            seq: current.conversation.last_seq + 2,
            appended_at: now.clone(),
            kind: AgentLogKind::OwnerMessage {
                content: "later fixture input".into(),
            },
        },
        AgentLogEntry {
            agent_id: current.id.clone(),
            seq: next.conversation.last_seq,
            appended_at: now.clone(),
            kind: AgentLogKind::StatusChanged {
                from: current.status,
                to: next.status,
            },
        },
    ];
    let operation = publication::PublicationIdentity {
        key: format!("current-turn:{}", uuid::Uuid::new_v4()),
        fingerprint: publication::request_digest(&next).unwrap(),
        appended_at: now,
    };
    let mut session = store.client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let source = capture::capture_transaction(store, &current, &mut scope)
        .await
        .unwrap();
    let plan = writer_commit::prepare(
        store,
        &mut scope,
        &source,
        writer_commit::PublicationInput {
            operation: &operation,
            current: &current,
            next: &next,
            logs: &logs,
            capacity: AppendCapacity::Normal,
            result_digest: publication::result_digest(&next).unwrap(),
        },
        None,
        None,
    )
    .await
    .unwrap();
    writer_commit::persist(
        store,
        &mut scope,
        plan,
        writer_commit::AggregateUpdate {
            filter: doc! {"_id": current.id.as_str()},
            fields: agent_replacement_fields(&next).unwrap(),
        },
    )
    .await
    .unwrap();
    scope.release().commit_transaction().await.unwrap();
    next
}

#[derive(Debug, PartialEq)]
pub(super) struct Snapshot {
    pub agent: Agent,
    pub carrier: Document,
    pub original_publication: Document,
    pub publications: u64,
}

pub(super) async fn snapshot(store: &MongoSandboxStore, original: &RecoveryWork) -> Snapshot {
    let key = original.request.key().unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":original.request.agent_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let snapshot = Snapshot {
        agent: crate::store::agents::read_agent(&row).unwrap(),
        carrier: records::collection(store)
            .find_one(doc! {"_id":&key})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap(),
        original_publication: store
            .segment_logs
            .publications
            .find_one(doc! {
                "publication_key": format!("{key}:step:{}", original.revision)
            })
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
