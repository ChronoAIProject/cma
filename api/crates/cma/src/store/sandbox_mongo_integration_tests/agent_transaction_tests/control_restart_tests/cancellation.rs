use super::*;
use crate::store::{
    agent_control_receipt::RESTART_PREDECESSOR_FIELD,
    agent_log_segments::{self as segments, publication, writer_gate as gate},
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, AgentRestartDispatch};
use layer3_adapter::{AgentOpenRequest, AgentProcessAction};

pub(super) async fn exercise_dispatched_teardown(
    store: &MongoSandboxStore,
    stopped: &Agent,
    armed: AgentRestartDispatch,
    acceptance: &layer2_core::sandbox::store::AgentRestartAcceptance,
    now: SystemTime,
) {
    use layer2_core::sandbox::store::{
        AgentRestartTeardownPublication, AgentWorkPublicationLineage, AgentWorkPublicationParents,
    };
    let stop = AgentStopRequest {
        target: AgentControlTarget::from(&armed.claimed),
        reason: AgentStopReason::Owner,
        requested_at: armed.dispatched_at.clone(),
    };
    let scheduled = store
        .schedule_agent_stop(stop.clone())
        .await
        .unwrap()
        .into_agent();
    assert_eq!(scheduled.status, AgentStatus::Starting);
    assert_eq!(scheduled.work.step, AgentWorkStep::RestartTeardown);
    assert_eq!(scheduled.work.cleanup_debt, armed.claimed.work.cleanup_debt);
    assert!(scheduled.work.lease.is_none());
    assert!(matches!(
        store.schedule_agent_stop(stop).await.unwrap(),
        AgentControlResult::Existing(agent) if agent == scheduled
    ));
    assert_eq!(
        store.prepare_agent_restart_dispatch(armed.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    let claimed = store
        .claim_agent_work(
            &scheduled.id,
            "teardown-worker",
            armed.dispatched_at.clone(),
            layer2_core::rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let teardown = store.agent_restart_teardown(&claimed).await.unwrap();
    assert_eq!(teardown.open, armed.open);
    assert_eq!(&teardown.acceptance, acceptance);
    let lease = claimed.work.lease.as_ref().unwrap();
    assert!(lease.fence > armed.claimed.work.lease.as_ref().unwrap().fence);
    let sandbox = store
        .get_sandbox(&claimed.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let acknowledgement = teardown.fence(lease.fence, worktree.path, false);
    let publication = AgentRestartTeardownPublication {
        claimed: claimed.clone(),
        teardown,
        acknowledgement,
        parents: AgentWorkPublicationParents {
            sandbox_revision: sandbox.revision,
            workspace_revision: workspace.revision,
            worktree_revision: worktree.revision,
            lineage: AgentWorkPublicationLineage::Authorized,
        },
        published_at: armed.dispatched_at,
    };
    let mut incorrect = publication.clone();
    incorrect.acknowledgement.predecessor.runtime.boot_id = "b".repeat(32);
    assert_eq!(
        store.publish_agent_restart_teardown(incorrect).await,
        Err(SandboxStoreError::Conflict)
    );
    let observed = store.get_agent(&claimed.id).await.unwrap().unwrap();
    assert_eq!(observed, claimed);
    assert!(observed.status.is_live());
    store
        .publish_agent_restart_teardown(publication.clone())
        .await
        .unwrap();
    let restored = store.get_agent(&claimed.id).await.unwrap().unwrap();
    assert_eq!(restored.status, AgentStatus::Stopped);
    assert_eq!(restored.stopped_at, stopped.stopped_at);
    assert_eq!(restored.conversation, stopped.conversation);
    assert_eq!(restored.work.receipts, stopped.work.receipts);
    assert!(restored.work.cleanup_debt.is_empty());
    assert_eq!(
        store.publish_agent_restart_teardown(publication).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut replay = request(&restored, &acceptance.source, "dispatch", [1; 32]);
    replay.observed_process = None;
    assert_eq!(
        &store.schedule_agent_restart(replay).await.unwrap(),
        acceptance
    );
    assert_eq!(store.get_agent(&restored.id).await.unwrap(), Some(restored));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_restart_cancellation_preserves_the_closed_process_in_both_log_formats() {
    for segmented in [false, true] {
        for delete in [false, true] {
            Box::pin(exercise_cancellation(segmented, delete)).await;
        }
    }
}

async fn exercise_cancellation(segmented: bool, delete: bool) {
    let (client, name) = database("owner_restart_cancel").await;
    crate::store::MongoOperationStore::open_for_test(&client, &name)
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (mut stopped, source) = stopped_agent(&store).await;
    if segmented {
        adopt(&client, &name, &store, &stopped).await;
        stopped = store.get_agent(&stopped.id).await.unwrap().unwrap();
    }
    let before = store
        .agents
        .find_one(doc! {"_id": stopped.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut original = request(&stopped, &source, "cancel-restart", [1; 32]);
    let now = SystemTime::now();
    let timestamp = layer2_core::rfc3339_at(now);
    original.requested_at.clone_from(&timestamp);
    let accepted = store
        .schedule_agent_restart(original.clone())
        .await
        .unwrap();
    let claimed = store
        .claim_agent_work(
            &stopped.id,
            "restart-worker",
            timestamp.clone(),
            layer2_core::rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let work = store.agent_restart_work(&claimed).await.unwrap();
    let sandbox = store
        .get_sandbox(&claimed.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let mut open = AgentOpenRequest::new(
        AgentProcessAction::Open.operation_id(claimed.id.as_str(), accepted.next_incarnation),
        crate::sandbox::runtime_options(&claimed, &sandbox, &worktree),
        Some(source.conversation_id.clone()),
    );
    open.restart_runtime = Some(source.runtime.clone());
    let delayed_dispatch = AgentRestartDispatch {
        claimed: claimed.clone(),
        work,
        open,
        dispatched_at: timestamp.clone(),
    };
    let target = AgentControlTarget::from(&claimed);
    let result = if delete {
        store
            .schedule_agent_delete(AgentDeleteRequest {
                target,
                requested_at: timestamp,
            })
            .await
    } else {
        store
            .schedule_agent_stop(AgentStopRequest {
                target,
                reason: AgentStopReason::Owner,
                requested_at: timestamp,
            })
            .await
    };
    let AgentControlResult::Scheduled(cancelled) = result.unwrap() else {
        panic!("the teardown must consume the restart atomically");
    };
    assert_eq!(cancelled.status, AgentStatus::Stopped);
    assert_eq!(cancelled.stopped_at, stopped.stopped_at);
    assert_eq!(cancelled.stop_reason, stopped.stop_reason);
    assert_eq!(cancelled.conversation, stopped.conversation);
    assert_eq!(cancelled.work.receipts, stopped.work.receipts);
    assert_eq!(cancelled.work.cleanup_debt, stopped.work.cleanup_debt);
    assert_eq!(cancelled.work.next_fence, claimed.work.next_fence);
    assert!(cancelled.work.lease.is_none());
    assert_eq!(cancelled.revision, claimed.revision + 1);
    assert_eq!(
        cancelled.work.step,
        if delete {
            AgentWorkStep::EnsureProcessClosed
        } else {
            AgentWorkStep::None
        }
    );
    assert_eq!(
        store.prepare_agent_restart_dispatch(delayed_dispatch).await,
        Err(SandboxStoreError::Conflict)
    );
    original.target = AgentControlTarget::from(&cancelled);
    original.observed_process = None;
    assert_eq!(
        store.schedule_agent_restart(original).await.unwrap(),
        accepted
    );
    assert_eq!(
        store.get_agent(&stopped.id).await.unwrap(),
        Some(cancelled.clone())
    );
    assert_eq!(
        store
            .agents
            .count_documents(doc! {
                "workspace_id": stopped.workspace_id.as_str(),
                "status": {"$in": crate::store::agents::LIVE_STATUSES},
            })
            .await
            .unwrap(),
        0
    );
    let row = store
        .agents
        .find_one(doc! {"_id": stopped.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get(STOP_RECEIPT_FIELD), before.get(STOP_RECEIPT_FIELD));
    assert!(!row.contains_key(RESTART_PREDECESSOR_FIELD));
    assert!(!row.contains_key("control_restart_active_v1"));
    assert!(!row.contains_key("control_restart_dispatch_v1"));
    assert!(read_control_receipts(&row, &cancelled).is_ok());
    client.database(&name).drop().await.unwrap();
}

pub(super) async fn adopt(client: &Client, name: &str, store: &MongoSandboxStore, agent: &Agent) {
    use crate::store::agent_log::log_document;
    for (seq, kind) in [
        (
            1,
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        ),
        (
            2,
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: agent.conversation.runtime_generation.unwrap(),
            },
        ),
    ] {
        let row = log_document(&AgentLogEntry {
            agent_id: agent.id.clone(),
            seq,
            appended_at: NOW.into(),
            kind,
        })
        .unwrap();
        let replaced = store
            .agent_log
            .replace_one(
                doc! {"agent_id": agent.id.as_str(), "seq": i64::try_from(seq).unwrap()},
                row,
            )
            .await
            .unwrap();
        assert_eq!(replaced.matched_count, 1);
    }
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$unset": {crate::store::agent_log_integrity::INTEGRITY_FIELD: ""}},
        )
        .await
        .unwrap();
    let db = client.database(name);
    segments::schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &db,
        gate::GateName::Segments,
        0,
        gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let kind = AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    };
    store
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: "restart-cancellation-adoption".into(),
                appended_at: layer2_core::now_rfc3339(),
                fingerprint: publication::request_digest(&kind)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
            },
            kind,
        )
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
}
