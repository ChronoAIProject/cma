use super::*;
use crate::store::agent_log_segments::{Head, Seal};
use futures_util::TryStreamExt;

pub(super) fn service(store: &MongoSandboxStore, enabled: bool) -> RecoveryService {
    RecoveryService::new(store.clone(), transactions::access(store))
        .with_features(CapacityRecoveryFeatures { enabled })
}

pub(super) async fn exhausted(store: &MongoSandboxStore) -> (Agent, Caller, RecoveryRequest) {
    let mut agent = fixture::capacity_fixture(store).await;
    // Fill raw legacy history before real gated adoption. Bootstrap computes
    // every leaf/projection from these rows; no fabricated head certifies them.
    let rows: Vec<_> = (agent.conversation.last_seq + 1..10_000)
        .map(|seq| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq,
                appended_at: NOW.into(),
                kind: AgentLogKind::ConversationGap { frames: 1 },
            })
            .unwrap()
        })
        .collect();
    store.agent_log.insert_many(rows).await.unwrap();
    agent.conversation.last_seq = 9_999;
    agent.revision += 1;
    replace_agent_fixture(store, &agent).await;
    store
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: "segmented-capacity-adoption".into(),
                fingerprint: [17; 32],
                appended_at: NOW.into(),
            },
            AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::PublishConnection,
                failure: agent.failure.clone().unwrap(),
            },
        )
        .await
        .unwrap();
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert!(crate::store::agent_log_segments::read_format(&row)
        .unwrap()
        .is_some());
    let head = Head::read(
        store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (head.active_count, head.last_seq, head.active_ordinal),
        (10_000, 10_000, 0)
    );
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
    (agent, caller, request)
}

pub(super) async fn set_gate(
    store: &MongoSandboxStore,
    name: writer_gate::GateName,
    policy: writer_gate::Policy,
) {
    let db = store.client.database(&store.agents.namespace().db);
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let current = writer_gate::read(&db, name, &mut session)
        .await
        .unwrap()
        .unwrap();
    writer_gate::replace(&db, name, current.revision, policy, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
}

pub(super) async fn close_gates(store: &MongoSandboxStore) {
    for gate in [
        writer_gate::GateName::Segments,
        writer_gate::GateName::CapacityRecovery,
    ] {
        set_gate(store, gate, writer_gate::Policy::default()).await;
    }
}

pub(super) async fn read_admission(
    store: &MongoSandboxStore,
    request: &RecoveryRequest,
) -> writer_admission::Admission {
    writer_admission::Admission::read(
        store
            .client
            .database(&store.agents.namespace().db)
            .collection::<Document>(writer_admission::COLLECTION)
            .find_one(doc! {"publication_key":request.key().unwrap()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

#[derive(Debug, PartialEq)]
pub(super) struct Snapshot {
    pub agent: Agent,
    pub envelope: Document,
    pub head: Head,
    pub projection: Document,
    pub carrier: Option<Document>,
    pub publication: Option<Document>,
    pub publications: Vec<Document>,
    pub seals: Vec<Document>,
    pub evidence: Vec<Document>,
    pub parents: Vec<Document>,
    pub logs_digest: mongodb::bson::Binary,
    pub logs_count: usize,
}

pub(super) async fn snapshot(store: &MongoSandboxStore, request: &RecoveryRequest) -> Snapshot {
    // A single majority-committed snapshot avoids mixing stable timestamps
    // after a committed-but-unknown publication.
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":request.agent_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let agent = crate::store::agents::read_agent(&row).unwrap();
    let mut parents = Vec::new();
    for (collection, id) in [
        (&store.sandboxes, agent.sandbox_id.as_str()),
        (&store.workspaces, agent.workspace_id.as_str()),
        (&store.worktrees, agent.worktree_id.as_str()),
    ] {
        parents.push(
            collection
                .find_one(doc! {"_id":id})
                .session(&mut session)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let mut groups = Vec::new();
    for collection in [
        &store.segment_logs.publications,
        &store.segment_logs.seals,
        &store.segment_logs.evidence,
        &store.agent_log,
    ] {
        let mut cursor = collection
            .find(doc! {})
            .sort(doc! {"_id":1})
            .session(&mut session)
            .await
            .unwrap();
        groups.push(
            cursor
                .stream(&mut session)
                .try_collect::<Vec<Document>>()
                .await
                .unwrap(),
        );
    }
    let logs = groups.pop().unwrap();
    let evidence = groups.pop().unwrap();
    let seals = groups.pop().unwrap();
    let publications = groups.pop().unwrap();
    let publication = publications
        .iter()
        .find(|row| row.get_str("publication_key").ok() == Some(request.key().unwrap().as_str()))
        .cloned();
    let snapshot = Snapshot {
        agent,
        envelope: row,
        head: Head::read(
            store
                .segment_logs
                .heads
                .find_one(doc! {"_id":request.agent_id.as_str()})
                .session(&mut session)
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap(),
        projection: store
            .segment_logs
            .projections
            .find_one(doc! {"_id":request.agent_id.as_str()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap(),
        carrier: records::collection(store)
            .find_one(doc! {"_id":request.key().unwrap()})
            .session(&mut session)
            .await
            .unwrap(),
        publication,
        publications,
        seals,
        evidence,
        parents,
        logs_digest: publication::result_digest(&logs).unwrap(),
        logs_count: logs.len(),
    };
    session.commit_transaction().await.unwrap();
    snapshot
}

pub(super) fn assert_admitted(before: &Snapshot, after: &Snapshot, work: &RecoveryWork) {
    let mut expected = before.agent.clone();
    expected.revision += 1;
    expected.work.next_fence += 1;
    expected.updated_at = work.request.requested_at.clone();
    assert_eq!(
        after.agent, expected,
        "storage recovery changes no inputs, receipts or conversation"
    );
    assert_eq!(after.agent.status, AgentStatus::Failed);
    assert_eq!(after.head.active_ordinal, before.head.active_ordinal + 1);
    assert_eq!(after.head.revision, before.head.revision + 1);
    assert_eq!(after.head.active_start_seq, before.head.last_seq + 1);
    assert_eq!((after.head.active_count, after.head.active_bytes), (0, 0));
    assert_eq!(after.head.last_seq, before.head.last_seq);
    assert_eq!(
        after.envelope.get_i64("document_schema_version").unwrap(),
        2
    );
    assert_eq!(
        after.envelope.get_i64("agent_log_head_revision").unwrap(),
        after.head.revision
    );
    assert_eq!(
        after.projection.get_i64("head_revision").unwrap(),
        after.head.revision
    );
    assert_eq!(
        after.projection.get_i64("through_seq").unwrap(),
        before.head.last_seq
    );
    assert_eq!(after.head.lifetime_bytes, before.head.lifetime_bytes);
    assert_eq!(after.seals.len(), before.seals.len() + 1);
    let seal = Seal::read(after.seals.last().unwrap().clone()).unwrap();
    assert_eq!(seal.end_seq, before.head.last_seq);
    assert_eq!(seal.start_seq, before.head.active_start_seq);
    assert_eq!(after.head.previous_seal.as_ref(), Some(&seal.seal_digest));
    assert_eq!(after.publications.len(), before.publications.len() + 1);
    for original in &before.publications {
        assert!(after.publications.contains(original));
    }
    assert_eq!(after.logs_count, before.logs_count);
    assert_eq!(
        after.logs_digest, before.logs_digest,
        "no input or synthetic log replay"
    );
    let carrier = after.carrier.as_ref().unwrap();
    assert_eq!(carrier.get_i64("format_version").unwrap(), 2);
    let stored_work: RecoveryWork =
        mongodb::bson::deserialize_from_document(carrier.get_document("work").unwrap().clone())
            .unwrap();
    assert_eq!(&stored_work, work);
    let result: records::ResultBinding =
        mongodb::bson::deserialize_from_document(carrier.get_document("result").unwrap().clone())
            .unwrap();
    let receipt = publication::Publication::read(after.publication.clone().unwrap()).unwrap();
    result.require_publication(work, &receipt).unwrap();
    assert_eq!(
        result.agent_digest,
        publication::result_digest(&after.agent).unwrap()
    );
    assert_eq!(
        result.logs_digest,
        publication::result_digest(&Vec::<AgentLogEntry>::new()).unwrap()
    );
    assert_eq!(result.published_at, work.request.requested_at);
    assert_eq!(
        receipt.fingerprint.bytes,
        work.admission_identity().unwrap().fingerprint
    );
    assert_eq!(receipt.active_ordinal, after.head.active_ordinal);
    assert_eq!(receipt.previous_seal, after.head.previous_seal);
}
