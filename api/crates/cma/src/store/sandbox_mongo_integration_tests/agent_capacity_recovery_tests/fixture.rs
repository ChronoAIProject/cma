//! Real source/gate fixture; native transport is supplied separately by each test.
use super::*;
use crate::store::agent_log_segments::{schema, writer_admission, writer_gate};
use layer2_core::sandbox::{RuntimeBinding, SandboxHome};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

pub(super) async fn capacity_fixture(store: &MongoSandboxStore) -> Agent {
    let mut agent = ready_conversation_fixture(store, "activation").await;
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "retained-runtime".into(),
        runtime_version: "fixture".into(),
        boot_id: Some("a".repeat(32)),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "empty".into(),
            template_generation: "seed".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "retained-home".into(),
        storage_generation: "original".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! {"_id": sandbox.id.as_str()},
            doc! {"$set":
            crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap()},
        )
        .await
        .unwrap();
    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
    worktree.base_commit = workspace.resolved_commit.unwrap();
    worktree.revision += 1;
    replace_worktree_fixture(store, &worktree).await;
    agent.status = AgentStatus::Failed;
    agent.work.step = AgentWorkStep::None;
    agent.work.lease = None;
    agent.failure = Some(TypedFailure {
        class: "agent_log_capacity_exceeded".into(),
        detail: "fixture".into(),
    });
    agent.revision += 1;
    agent.conversation.last_seq += 1;
    replace_agent_fixture(store, &agent).await;
    store
        .agent_log
        .insert_one(
            crate::store::agent_log::log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: agent.conversation.last_seq,
                appended_at: NOW.into(),
                kind: AgentLogKind::WorkStepFailed {
                    step: AgentWorkStep::PublishConnection,
                    failure: agent.failure.clone().unwrap(),
                },
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let db = store.client.database(&store.agents.namespace().db);
    schema::migrate(&db).await.unwrap();
    // Until the narrow S1 migration pin arrives, this fixture tests storage
    // transactions, not canonical schema/isolated-role qualification (P1-2).
    let existing = db.list_collection_names().await.unwrap();
    for name in [
        writer_gate::COLLECTION,
        writer_admission::COLLECTION,
        crate::store::agent_capacity_recovery::records::COLLECTION,
    ] {
        if !existing.iter().any(|value| value == name) {
            db.create_collection(name).await.unwrap();
        }
    }
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    for name in [
        writer_gate::GateName::Segments,
        writer_gate::GateName::CapacityRecovery,
    ] {
        writer_gate::replace(
            &db,
            name,
            0,
            writer_gate::Policy {
                enabled: true,
                agents: vec![agent.id.to_string()],
                ..Default::default()
            },
            &mut session,
        )
        .await
        .unwrap();
    }
    session.commit_transaction().await.unwrap();
    agent
}
