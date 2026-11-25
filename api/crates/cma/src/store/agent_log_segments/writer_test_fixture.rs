use super::*;
use crate::store::agent_log::log_document;
use crate::store::agent_log_segments::{
    digest, projection::Projection, publication, schema, writer_plan, writer_projection,
};
use layer2_core::sandbox::{AgentLogKind, AgentLogProjectionState};
use mongodb::{bson::RawDocumentBuf, Client};

pub(super) const NOW: &str = "2026-11-11T00:00:00Z";

pub(super) struct Fixture {
    pub client: Client,
    pub store: MongoSandboxStore,
    pub agent: Agent,
    pub head: Head,
    pub state: AgentLogProjectionState,
    pub leaves: Binary,
}

pub(super) async fn fixture(count: i64) -> Fixture {
    named_fixture(count, None, "agt_delta_fixture").await
}

pub(super) async fn named_fixture(count: i64, database: Option<&str>, id: &str) -> Fixture {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = database
        .map(str::to_owned)
        .unwrap_or_else(|| format!("cma_writer_{}", uuid::Uuid::new_v4().simple()));
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let (mut agent, mut head, mut state) = writer_projection::tests::opened();
    agent.id = layer2_core::AgentId(id.into());
    agent.creation_key = format!("manual:{id}");
    agent.worktree_id = layer2_core::WorktreeId(format!("wt_{id}"));
    head.agent_id = id.into();
    head.incarnation = digest::incarnation(&agent);
    let mut raw = Vec::new();
    let mut leaves = Vec::new();
    let mut bytes = 0;
    for seq in 1..=count {
        let kind = match seq {
            1 => AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
            2 => AgentLogKind::ConversationOpened {
                conversation_id: "native-fixture".into(),
                runtime_generation: 1,
            },
            _ => AgentLogKind::ConversationGap { frames: 1 },
        };
        let entry = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: seq as u64,
            appended_at: NOW.into(),
            kind,
        };
        let row = bson::serialize_to_vec(&log_document(&entry).unwrap()).unwrap();
        leaves.extend(digest::leaf(&head.agent_id, &head.incarnation, seq, &row).bytes);
        bytes += row.len() as i64;
        raw.push(RawDocumentBuf::from_bytes(row).unwrap());
    }
    agent.conversation.last_seq = count as u64;
    state.through_seq = count as u64;
    head.last_seq = count;
    head.active_count = count;
    head.active_bytes = bytes;
    head.lifetime_count = count;
    head.lifetime_bytes = bytes;
    head.projection_through = count;
    let mut row = crate::store::agents::agent_document(&agent).unwrap();
    row.insert("document_schema_version", 2_i64);
    row.insert("agent_log_format", FORMAT);
    row.insert("agent_log_incarnation", &head.incarnation);
    row.insert("agent_log_head_revision", head.revision);
    store.agents.insert_one(row).await.unwrap();
    store
        .agent_log
        .clone_with_type::<RawDocumentBuf>()
        .insert_many(raw)
        .await
        .unwrap();
    collection(&store, HEADS)
        .insert_one(bson::serialize_to_document(&head).unwrap())
        .await
        .unwrap();
    collection(&store, PROJECTIONS)
        .insert_one(
            bson::serialize_to_document(&Projection {
                agent_id: head.agent_id.clone(),
                format_version: 1,
                incarnation: head.incarnation.clone(),
                evidence_generation: head.evidence_generation.clone(),
                through_seq: count,
                head_revision: head.revision,
                record: state.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    Fixture {
        client,
        store,
        agent,
        head,
        state,
        leaves: digest::binary(leaves),
    }
}

impl Fixture {
    pub fn prepared(&self, scope: &TransactionScope<'_>) -> PreparedPublication {
        let logs = [AgentLogEntry {
            agent_id: self.agent.id.clone(),
            seq: self.head.last_seq as u64 + 1,
            appended_at: NOW.into(),
            kind: AgentLogKind::ConversationGap { frames: 1 },
        }];
        let mut next = self.agent.clone();
        next.revision += 1;
        next.conversation.last_seq += 1;
        let batch = EncodedBatch::new(&self.head, &logs).unwrap();
        let plan = writer_plan::plan(
            &self.head,
            self.leaves.clone(),
            &batch,
            AppendCapacity::Normal,
            writer_plan::RolloverPermission::Normal,
            SourceFormat::Segmented,
            "fixture-publication",
        )
        .unwrap();
        let delta = writer_projection::plan(
            &self.agent,
            &next,
            &self.head,
            &plan.head,
            &self.state,
            &logs,
            SourceFormat::Segmented,
        )
        .unwrap();
        let operation = PublicationIdentity {
            key: "fixture-publication".into(),
            fingerprint: publication::request_digest(&logs).unwrap(),
            appended_at: NOW.into(),
        };
        assert!(
            delta.evidence.is_empty(),
            "gap fixture has no entity writes"
        );
        let receipt = Publication::new(
            &self.agent,
            &next,
            &plan.head,
            &operation,
            publication::result_digest(&next).unwrap(),
        )
        .unwrap();
        PreparedPublication {
            transaction: scope.identity(),
            database: self.store.agents.namespace().db,
            current: self.agent.clone(),
            next,
            source: SourceFormat::Segmented,
            before: self.head.clone(),
            batch,
            plan,
            active_proof: None,
            delta,
            evidence: Vec::new(),
            receipt,
            admission: None,
            permit: None,
        }
    }

    pub async fn destroy(self) {
        self.client
            .database(&self.store.agents.namespace().db)
            .drop()
            .await
            .unwrap();
    }
}
