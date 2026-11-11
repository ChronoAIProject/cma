use super::*;
use layer2_core::ids::*;
use layer2_core::profiles::*;
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
};

pub(super) async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    (
        Client::with_uri_str(uri).await.unwrap(),
        format!("cma_segments_{prefix}_{}", uuid::Uuid::new_v4().simple()),
    )
}

pub(super) fn agent() -> Agent {
    Agent {
        id: AgentId("agt_segment_fixture".into()),
        worktree_id: WorktreeId("wkt_segment_fixture".into()),
        workspace_id: WorkspaceId("wks_segment_fixture".into()),
        sandbox_id: SandboxId("sbx_segment_fixture".into()),
        owner: SubjectId("fixture".into()),
        creation_key: "segment-fixture".into(),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_fixture".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

pub(super) async fn segmented(
    store: &MongoSandboxStore,
    closed: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    segmented_with_width(store, closed, 2, 2).await
}

pub(super) async fn segmented_with_width(
    store: &MongoSandboxStore,
    closed: i64,
    width: i64,
    active: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    assert!((1..=MAX_ENTRIES).contains(&width));
    assert!((0..=MAX_ENTRIES).contains(&active));
    let agent = agent();
    let entries: Vec<_> = (1..=closed * width + active)
        .map(|seq| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: seq as u64,
            appended_at: NOW.into(),
            kind: if seq == 1 {
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                }
            } else {
                AgentLogKind::ModeChanged {
                    mode: "code".into(),
                }
            },
        })
        .collect();
    install(store, agent, entries, closed, width).await
}

pub(super) async fn install(
    store: &MongoSandboxStore,
    mut agent: Agent,
    entries: Vec<AgentLogEntry>,
    closed: i64,
    width: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    store
        .agent_log
        .insert_many(entries.iter().map(|e| log_document(e).unwrap()))
        .await
        .unwrap();
    let raw: Vec<RawDocumentBuf> = store
        .agent_log
        .clone_with_type()
        .find(doc! {"agent_id":agent.id.as_str()})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let incarnation = digest::incarnation(&agent);
    let mut previous = None;
    let mut prefix_bytes = 0;
    let mut seals = Vec::new();
    for ordinal in 0..closed {
        let start = ordinal * width + 1;
        let rows = &raw[(ordinal * width) as usize..((ordinal + 1) * width) as usize];
        let bytes = rows.iter().map(|r| r.as_bytes().len() as i64).sum::<i64>();
        prefix_bytes += bytes;
        let leaves = rows
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                digest::leaf(
                    agent.id.as_str(),
                    &incarnation,
                    start + i as i64,
                    r.as_bytes(),
                )
                .bytes
            })
            .collect::<Vec<_>>();
        let mut seal = Seal {
            id: Seal::identity(agent.id.as_str(), &incarnation, ordinal),
            format_version: 1,
            agent_id: agent.id.0.clone(),
            incarnation: incarnation.clone(),
            ordinal,
            start_seq: start,
            end_seq: start + width - 1,
            entry_count: width,
            encoded_bytes: bytes,
            previous_seal: previous,
            prefix_count: start + width - 1,
            prefix_bytes,
            leaf_digests: digest::binary(leaves),
            seal_digest: digest::binary(vec![]),
            digest_version: digest::VERSION.into(),
            sealing_operation: format!("fixture:{ordinal}"),
            source_format: "legacy_v1".into(),
            storage_kind: "mongo".into(),
        };
        seal.seal_digest = seal.digest();
        seal.validate().unwrap();
        previous = Some(seal.seal_digest.clone());
        seals.push(bson::serialize_to_document(&seal).unwrap());
    }
    if !seals.is_empty() {
        collection(store, SEGMENTS)
            .insert_many(seals)
            .await
            .unwrap();
    }
    let active_bytes = raw[(closed * width) as usize..]
        .iter()
        .map(|r| r.as_bytes().len() as i64)
        .sum::<i64>();
    agent.conversation.last_seq = entries.len() as u64;
    let head = Head {
        agent_id: agent.id.0.clone(),
        format_version: 1,
        incarnation: incarnation.clone(),
        revision: 1,
        active_ordinal: closed,
        active_start_seq: closed * width + 1,
        last_seq: entries.len() as i64,
        active_count: entries.len() as i64 - closed * width,
        active_bytes,
        previous_seal: previous,
        lifetime_count: entries.len() as i64,
        lifetime_bytes: prefix_bytes + active_bytes,
        projection_format: 1,
        evidence_generation: "initial".into(),
        projection_through: entries.len() as i64,
        migration_id: segments::MIGRATION.into(),
        required_reader: segments::READER.into(),
    };
    head.validate().unwrap();
    collection(store, HEADS)
        .insert_one(bson::serialize_to_document(&head).unwrap())
        .await
        .unwrap();
    let mut row = crate::store::agents::agent_document(&agent).unwrap();
    row.insert("document_schema_version", 2_i64);
    row.insert("agent_log_format", segments::FORMAT);
    row.insert("agent_log_incarnation", incarnation);
    row.insert("agent_log_head_revision", 1_i64);
    store
        .agents
        .replace_one(doc! {"_id":agent.id.as_str()}, row)
        .await
        .unwrap();
    (agent, entries)
}

pub(super) async fn certify(store: &MongoSandboxStore, agent: &Agent, entries: &[AgentLogEntry]) {
    let head = Head::read(
        collection(store, HEADS)
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut state = layer2_core::sandbox::AgentLogProjectionState::default();
    for entry in entries {
        state
            .observe(
                agent,
                entry,
                layer2_core::sandbox::AgentLogProjectionState::queued_owner(entry),
            )
            .unwrap();
    }
    state.validate_aggregate(agent).unwrap();
    let projection = segments::projection::Projection {
        agent_id: agent.id.0.clone(),
        format_version: 1,
        incarnation: head.incarnation,
        evidence_generation: head.evidence_generation,
        through_seq: head.last_seq,
        head_revision: head.revision,
        record: state,
    };
    collection(store, PROJECTIONS)
        .insert_one(bson::serialize_to_document(&projection).unwrap())
        .await
        .unwrap();
}
