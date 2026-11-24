use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_publication_atomically_saves_agent_worktree_logs_and_event() {
    let (client, name) = database("agent_work_publication").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "publish").await;
    let claimed = ready_publication_claim(claimed, "conversation-1");
    replace_agent_fixture(&store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    let log_kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-1".into(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.last_seq += u64::try_from(log_kinds.len()).unwrap();

    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: worktree.clone(),
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: next.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: log_kinds.clone(),
        })
        .await
        .unwrap();

    assert_eq!(store.get_agent(&next.id).await.unwrap(), Some(next.clone()));
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    let logs = store.agent_log(&next.id, 0, 10).await.unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(
        logs.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        logs.iter()
            .map(|entry| entry.kind.clone())
            .collect::<Vec<_>>(),
        log_kinds
    );
    let events = store.sandbox_events(&parent.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        SandboxEventKind::AgentStatusChanged {
            agent_id: next.id,
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }
    );
    let agent_row = store
        .agents
        .find_one(doc! { "_id": logs[0].agent_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent_row.get_i64("last_seq").unwrap(), 2);
    assert!(agent_row.get_bool("work_pending").unwrap());
    assert_eq!(
        agent_row.get("work_lease_holder"),
        Some(&mongodb::bson::Bson::Null)
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_rejects_an_incorrect_agent_log_high_water_mark() {
    let (client, name) = database("agent_work_incorrect_log_high_water").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "incorrect_log_high_water").await;
    let next = advance_claimed_agent(&claimed);

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: None,
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_consumes_only_the_exact_durable_stop_intent() {
    let (client, name) = database("agent_work_stop_intent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, mut worktree, initial_claimed) =
        claimed_agent_work_fixture(&store, "stop_intent").await;
    let mut scheduled = initial_claimed.clone();
    scheduled
        .request_stop(AgentStopReason::Owner, NOW.into())
        .unwrap();
    store
        .save_agent(initial_claimed.revision, scheduled.clone())
        .await
        .unwrap();
    let claimed = store
        .claim_agent_work(&scheduled.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;

    let mut cleared = claimed.clone();
    cleared
        .work
        .advance("worker", fence, AgentWorkStep::CloseProcess, NOW.into())
        .unwrap();
    cleared.requested_stop_reason = None;
    cleared.revision += 1;
    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence,
                agent: cleared,
                worktree: None,
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&claimed.id).await.unwrap(),
        Some(claimed.clone())
    );

    let mut claimed = claimed;
    claimed.work.step = AgentWorkStep::PublishStopped;
    claimed.work.cleanup_not_before = None;
    replace_agent_fixture(&store, &claimed).await;
    let mut stopped = claimed.clone();
    stopped
        .work
        .advance("worker", fence, AgentWorkStep::None, MINUTE.into())
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.conversation.last_seq += 1;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Failed).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: stopped.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree,
            }),
            appended_at: MINUTE.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: stopped.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: AgentStopReason::Owner.as_str().into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(store.get_agent(&stopped.id).await.unwrap(), Some(stopped));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_stop_settlement_fences_parents_and_retains_partial_output() {
    use layer2_core::sandbox::store::{
        AgentWorkPublication, AgentWorkPublicationLineage, AgentWorkPublicationParents,
    };

    let (client, name) = database("owner_stop_settlement").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = ready_conversation_fixture(&store, "stop_settlement").await;
    store
        .agent_log
        .delete_many(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap();
    let history = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner prompt".into(),
        },
        AgentLogKind::AgentMessage {
            content: "partial response".into(),
        },
    ];
    store
        .agent_log
        .insert_many(history.iter().enumerate().map(|(index, kind)| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: (index + 1) as u64,
                appended_at: NOW.into(),
                kind: kind.clone(),
            })
            .unwrap()
        }))
        .await
        .unwrap();
    agent.status = AgentStatus::Working;
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    agent.conversation.active_turn = Some(3);
    agent.conversation.last_seq = history.len() as u64;
    agent.revision += 1;
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert("last_seq", agent.conversation.last_seq as i64);
    store.agents.update_one(
        doc! {"_id": agent.id.as_str()},
        doc! {"$set": fields, "$unset": {(crate::store::agent_log_integrity::INTEGRITY_FIELD): ""}},
    ).await.unwrap();

    let mut stopping = store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: NOW.into(),
        })
        .await
        .unwrap()
        .into_agent();
    stopping
        .work
        .schedule(AgentWorkStep::PublishStopped, NOW.into());
    stopping.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::CloseProcess,
        operation_id: format!("agent:{}:close:v1", agent.id),
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    });
    replace_agent_fixture(&store, &stopping).await;
    let claimed = store
        .claim_agent_work(&agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    assert!(store.has_owner_stop_receipt(&claimed).await.unwrap());

    let mut stopped = claimed.clone();
    let interrupted = stopped.settle_owner_stop_turn().unwrap();
    stopped
        .work
        .advance("worker", fence, AgentWorkStep::None, NOW.into())
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            NOW.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.conversation.last_seq += 2;
    let proposal = AgentWorkMutation {
        expected_agent_revision: claimed.revision,
        holder: "worker".into(),
        fence,
        agent: stopped.clone(),
        worktree: None,
        appended_at: NOW.into(),
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: agent.id.clone(),
            from: AgentStatus::Working,
            to: AgentStatus::Stopped,
        }),
        agent_logs: vec![
            interrupted,
            AgentLogKind::Stopped {
                reason: "owner".into(),
            },
        ],
    };
    let sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let observed = AgentWorkPublicationParents {
        sandbox_revision: sandbox.revision,
        workspace_revision: workspace.revision,
        worktree_revision: worktree.revision,
        lineage: AgentWorkPublicationLineage::Authorized,
    };
    assert_eq!(
        store.commit_agent_work(proposal.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    let stale = AgentWorkPublicationParents {
        sandbox_revision: observed.sandbox_revision + 1,
        ..observed
    };
    assert_eq!(
        store.try_publish_agent_work(proposal.clone(), stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(claimed));
    assert_eq!(
        store
            .try_publish_agent_work(proposal, observed)
            .await
            .unwrap(),
        AgentWorkPublication::Published,
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(stopped));
    let actual: Vec<_> = store
        .agent_log(&agent.id, 0, 20)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.kind)
        .collect();
    assert_eq!(&actual[..history.len()], history);
    assert_eq!(
        &actual[history.len()..],
        &[
            AgentLogKind::TurnInterrupted { turn_sequence: 3 },
            AgentLogKind::Stopped {
                reason: "owner".into()
            },
        ]
    );
    client.database(&name).drop().await.unwrap();
}
