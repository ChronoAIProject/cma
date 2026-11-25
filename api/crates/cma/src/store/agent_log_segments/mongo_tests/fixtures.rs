use super::*;
use layer2_core::ids::*;
use layer2_core::profiles::*;
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
};

pub(in crate::store) async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    assert!(prefix.is_ascii());
    // Keep the random identity intact and leave room for the cross-database
    // proof fixtures' `_other` suffix within Mongo's database-name limit.
    let prefix = &prefix[..prefix.len().min(11)];
    (
        Client::with_uri_str(uri).await.unwrap(),
        format!("cma_segments_{prefix}_{}", uuid::Uuid::new_v4().simple()),
    )
}

pub(in crate::store) fn agent() -> Agent {
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
            turn_timeout: None,
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

pub(in crate::store) async fn segmented_with_width(
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

pub(in crate::store) async fn install(
    store: &MongoSandboxStore,
    agent: Agent,
    entries: Vec<AgentLogEntry>,
    closed: i64,
    width: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    let agent = install_bounded(
        store,
        agent,
        entries.iter().cloned(),
        entries.len() as i64,
        closed,
        width,
    )
    .await;
    (agent, entries)
}

pub(super) async fn install_bounded(
    store: &MongoSandboxStore,
    mut agent: Agent,
    mut entries: impl Iterator<Item = AgentLogEntry>,
    count: i64,
    closed: i64,
    width: i64,
) -> Agent {
    const INSERT_BATCH: usize = 256;
    crate::store::migrations::agent_log_source_keys::prepare_for_test(
        &store.client.database(&store.agent_log.namespace().db),
    )
    .await;
    assert!(width > 0 && closed * width <= count);
    let incarnation = digest::incarnation(&agent);
    let mut previous = None;
    let mut lifetime_bytes = 0;
    let mut active_bytes = 0;
    let active_count = count - closed * width;
    let collection_rows = store.agent_log.clone_with_type::<RawDocumentBuf>();
    for ordinal in 0..=closed {
        let start = ordinal * width + 1;
        let size = if ordinal == closed {
            active_count
        } else {
            width
        };
        let mut leaves = Vec::new();
        let mut bytes = 0;
        let mut batch = Vec::with_capacity(INSERT_BATCH);
        for seq in start..start + size {
            let entry = entries.next().expect("fixture row missing");
            assert_eq!(entry.seq, seq as u64);
            assert_eq!(entry.agent_id, agent.id);
            let raw = bson::serialize_to_vec(&log_document(&entry).unwrap()).unwrap();
            bytes += raw.len() as i64;
            if ordinal < closed {
                leaves.extend(digest::leaf(agent.id.as_str(), &incarnation, seq, &raw).bytes);
            }
            batch.push(RawDocumentBuf::from_bytes(raw).unwrap());
            if batch.len() == INSERT_BATCH {
                collection_rows.insert_many(batch.drain(..)).await.unwrap();
            }
        }
        if !batch.is_empty() {
            collection_rows.insert_many(batch).await.unwrap();
        }
        lifetime_bytes += bytes;
        if ordinal == closed {
            active_bytes = bytes;
            break;
        }
        let mut seal = Seal {
            id: Seal::identity(agent.id.as_str(), &incarnation, ordinal),
            format_version: 1,
            agent_id: agent.id.0.clone(),
            incarnation: incarnation.clone(),
            ordinal,
            start_seq: start,
            end_seq: start + size - 1,
            entry_count: size,
            encoded_bytes: bytes,
            previous_seal: previous,
            prefix_count: start + size - 1,
            prefix_bytes: lifetime_bytes,
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
        collection(store, SEGMENTS)
            .insert_one(bson::serialize_to_document(&seal).unwrap())
            .await
            .unwrap();
    }
    assert!(entries.next().is_none(), "unexpected fixture rows");
    agent.conversation.last_seq = count as u64;
    let head = Head {
        agent_id: agent.id.0.clone(),
        format_version: 1,
        incarnation: incarnation.clone(),
        revision: 1,
        active_ordinal: closed,
        active_start_seq: closed * width + 1,
        last_seq: count,
        active_count,
        active_bytes,
        previous_seal: previous,
        lifetime_count: count,
        lifetime_bytes,
        projection_format: 1,
        evidence_generation: "initial".into(),
        projection_through: count,
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
    store.agents.insert_one(row).await.unwrap();
    agent
}

pub(in crate::store) async fn certify(
    store: &MongoSandboxStore,
    agent: &Agent,
    entries: &[AgentLogEntry],
) {
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

/// Writer fixtures without tool events need the output index as well as the
/// semantic projection. Read-only fixtures intentionally certify less evidence.
pub(in crate::store) async fn certify_output(
    store: &MongoSandboxStore,
    agent: &Agent,
    entries: &[AgentLogEntry],
) {
    let head = Head::read(
        collection(store, HEADS)
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut index = None;
    let mut rows = std::collections::BTreeMap::new();
    for entry in entries {
        for row in projection::output::fold(
            &head,
            &mut index,
            entry,
            layer2_core::sandbox::agent_output_index::AgentOutputTool::None,
        )
        .unwrap()
        {
            rows.insert(row.get_str("_id").unwrap().to_owned(), row);
        }
    }
    if !rows.is_empty() {
        collection(store, EVIDENCE)
            .insert_many(rows.into_values())
            .await
            .unwrap();
    }
}
