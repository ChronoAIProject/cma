use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_stop_settlement_fences_parents_and_retains_partial_output() {
    for segmented in [false, true] {
        check_owner_stop_fences(false, segmented, false, false).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_stop_settlement_refuses_expired_lease_without_takeover() {
    for segmented in [false, true] {
        check_owner_stop_fences(true, segmented, false, false).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn owner_stop_settlement_reconciles_a_lost_acknowledgement() {
    for segmented in [false, true] {
        check_owner_stop_fences(false, segmented, true, false).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_stop_finalizes_a_published_text_prefix_once() {
    for segmented in [false, true] {
        check_owner_stop_fences(false, segmented, false, true).await;
    }
}

async fn check_owner_stop_fences(expire: bool, segmented: bool, lost_ack: bool, incremental: bool) {
    use crate::store::budget_test_fixture::OwnerStopBudgetFixture;
    use layer2_core::sandbox::store::{
        AgentLogPublicationIdentity, AgentWorkPublication, AgentWorkPublicationLineage,
        AgentWorkPublicationParents,
    };
    use std::time::{Duration, SystemTime};

    let (mut client, name) = database("owner_stop_settlement").await;
    let uncertain_replies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    if lost_ack {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let mut options = ClientOptions::parse(uri).await.unwrap();
        options.app_name = Some(name.clone());
        let seen = Arc::clone(&uncertain_replies);
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Succeeded(event) = event {
                if event.command_name == "commitTransaction"
                    && event
                        .reply
                        .get_document("writeConcernError")
                        .is_ok_and(|error| error.get_i32("code").ok() == Some(50))
                {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        client = Client::with_options(options).unwrap();
    }
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = ready_conversation_fixture(&store, "stop_settlement").await;
    store
        .agent_log
        .delete_many(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap();
    let mut history = vec![
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
    ];
    if !incremental {
        history.push(AgentLogKind::AgentMessage {
            content: "partial response".into(),
        });
    }
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

    if segmented {
        use crate::store::agent_log_segments::publication::request_digest;
        use crate::store::agent_worker_transaction::skill_result_test_support::{
            assert_format, enable_segments,
        };

        enable_segments(&store, &agent).await;
        let adopted = AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        };
        store
            .append_agent_log_keyed(
                &agent.id,
                &AgentLogPublicationIdentity {
                    key: "owner-stop-adoption".into(),
                    appended_at: layer2_core::now_rfc3339(),
                    fingerprint: request_digest(&adopted).unwrap().bytes.try_into().unwrap(),
                },
                adopted.clone(),
            )
            .await
            .unwrap();
        assert_format(&store, &agent.id, true).await;
        history.push(adopted);
        agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    }
    if incremental {
        use layer2_core::sandbox::{
            AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
        };
        use layer3_adapter::{ContentBlock, ContentChunk, ConversationUpdate, SessionUpdate};
        let recorder = DurableAgentUpdateRecorder::with_incremental_output_policy(
            Arc::new(store.clone()),
            Arc::default(),
            None,
            false,
            false,
            None,
            true,
        );
        let result = recorder
            .record_update(
                &agent.id,
                1,
                &mut AgentMessageAssembly::default(),
                ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::from("partial response"),
                ))),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let suffix = AgentLogKind::AgentMessageDelta {
            offset: 0,
            content: "partial response".into(),
        };
        assert_eq!(result.appended.len(), 1);
        assert_eq!(result.appended[0].kind, suffix);
        history.push(suffix);
        agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    }
    let budget = OwnerStopBudgetFixture::reserve(&store, &agent, 3).await;

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
    let started_at = SystemTime::now();
    let claimed_at = layer2_core::rfc3339_at(started_at);
    let lease_duration = Duration::from_secs(if expire { 2 } else { 60 });
    let expires_at = layer2_core::rfc3339_at(started_at + lease_duration);
    let claimed = store
        .claim_agent_work(&agent.id, "worker", claimed_at.clone(), expires_at)
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    assert!(store.has_owner_stop_receipt(&claimed).await.unwrap());

    let mut stopped = claimed.clone();
    let interrupted = stopped.settle_owner_stop_turn().unwrap();
    stopped
        .work
        .advance("worker", fence, AgentWorkStep::None, claimed_at.clone())
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            claimed_at.clone(),
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
        appended_at: claimed_at,
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
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(claimed.clone())
    );
    if expire {
        let wait = (started_at + lease_duration)
            .duration_since(SystemTime::now())
            .unwrap_or_default();
        tokio::time::sleep(wait).await;
        assert_eq!(
            store.try_publish_agent_work(proposal, observed).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(claimed));
        let actual: Vec<_> = store
            .agent_log(&agent.id, 0, 20)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(actual, history);
        budget.assert_held(&store).await;
        client.database(&name).drop().await.unwrap();
        return;
    }
    let admin = client.database("admin");
    if lost_ack {
        admin
            .run_command(doc! {
                "configureFailPoint": "failCommand",
                "mode": {"times": 5_i32},
                "data": {
                    "failCommands": ["commitTransaction"],
                    "appName": &name,
                    "writeConcernError": {
                        "code": 50_i32, "codeName": "MaxTimeMSExpired",
                        "errmsg": "injected owner Stop publication ambiguity",
                    },
                },
            })
            .await
            .unwrap();
    }
    let published = store.try_publish_agent_work(proposal, observed).await;
    if lost_ack {
        admin
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
            .await
            .unwrap();
        assert!(uncertain_replies.load(Ordering::SeqCst) > 0);
    }
    assert_eq!(published.unwrap(), AgentWorkPublication::Published);
    if incremental {
        stopped.conversation.last_seq += 1;
    }
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(stopped));
    let actual: Vec<_> = store
        .agent_log(&agent.id, 0, 20)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.kind)
        .collect();
    assert_eq!(&actual[..history.len()], history);
    budget.assert_settled_once(&store).await;
    let mut terminal = vec![];
    if incremental {
        terminal.push(AgentLogKind::AgentMessage {
            content: "partial response".into(),
        });
    }
    terminal.extend([
        AgentLogKind::TurnInterrupted { turn_sequence: 3 },
        AgentLogKind::Stopped {
            reason: "owner".into(),
        },
    ]);
    assert_eq!(&actual[history.len()..], terminal);
    client.database(&name).drop().await.unwrap();
}
