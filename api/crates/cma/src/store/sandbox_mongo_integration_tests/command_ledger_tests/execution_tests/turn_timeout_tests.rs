use super::*;
use layer2_core::sandbox::{store::AgentConversationStore, turn_timeout::*};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, PromptResponse, SessionUpdate, TurnStopReason,
};
use mongodb::bson::doc;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn turn_policy_terminal_is_atomic_and_replays_after_restart() {
    for segmented in [false, true] {
        Box::pin(exercise(segmented, false, None, true)).await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn turn_policy_terminal_loses_to_a_committed_result() {
    for segmented in [false, true] {
        Box::pin(exercise(segmented, true, None, true)).await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn turn_policy_uncertain_commit_reconciles_without_repeating_the_terminal() {
    for segmented in [false, true] {
        for committed in [false, true] {
            Box::pin(exercise(segmented, false, Some(committed), true)).await;
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn turn_policy_refuses_an_incomplete_history() {
    Box::pin(exercise(false, false, None, false)).await;
}

async fn exercise(segmented: bool, result_first: bool, uncertain: Option<bool>, trusted: bool) {
    use crate::store::turn_timeouts::discovery;
    let (client, name) = database("turn_policy").await;
    let (store, agent, parent, execution, _, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_turn_policy"),
        trusted.then_some((6, false)),
    )
    .await;
    assert!(discovery::page(&store, None, i64::MAX)
        .await
        .unwrap()
        .is_empty());
    if segmented {
        let budget = super::super::budget_tests::configure_metered(&store, &agent.owner).await;
        // Keep the uncertain first turn charged while admitting one queued input.
        store.budget_settings.update_one(doc! {"_id":"platform"}, doc! {"$set":{
            "settings.model_budget.owners.0.policy.owner_day_limit": (budget.turn_limit * 2) as i64
        }}).await.unwrap();
        super::stale_turn_tests::settlement::adopt(&client, &name, &store, &agent).await;
    }
    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    let first = record(
        store
            .reserve_command(live_request(&ready, &parent, "timeout-one"))
            .await
            .unwrap(),
    );
    let id = &first.receipt.envelope.command_id;
    AgentCommandWorker::new(store.clone(), execution, "timeout-worker".into())
        .unwrap()
        .run_one(id)
        .await
        .unwrap();
    let command = store.reconcile_command(id).await.unwrap();
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let dispatch = command.work.dispatch.as_ref().unwrap();
    let now = layer2_core::now_rfc3339();
    let now_ms = layer2_core::seconds_from_rfc3339(&now).unwrap() as i64 * 1_000;
    let timeout = TurnTimeout::admit(
        TurnTimeoutBinding {
            agent_id: current.id.clone(),
            creation_key: current.creation_key.clone(),
            creation_fingerprint: current.creation_fingerprint.clone(),
            command_id: id.clone(),
            turn_sequence: dispatch.turn_sequence,
            operation_sequence: dispatch.operation_sequence,
            runtime_generation: dispatch.runtime_generation,
            runtime_boot: "b".repeat(32),
            conversation_id: current.conversation.conversation_id.clone().unwrap(),
            prompt_id: layer3_adapter::PromptIdentity::new(
                current.conversation.conversation_id.as_ref().unwrap(),
                dispatch.runtime_generation,
                dispatch.turn_sequence,
            )
            .as_str()
            .into(),
        },
        uuid::Uuid::new_v4().to_string(),
        TurnTimeoutCause::ServiceDeadline,
        now_ms - 1_000,
        now_ms,
    )
    .unwrap();
    // Fixture admission pins an already-known dispatch. Production policy
    // admission is separately gated and must precede eligible runtime work.
    client
        .database(&name)
        .run_command(doc! {"createIndexes":"sandbox_credentials", "indexes":[{
            "name":"sandbox_credentials_generation", "key":{"sandbox_id":1,"generation":1},
            "unique":true
        }]})
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    crate::store::work_schema::floor::retain(&store, &mut session)
        .await
        .unwrap();
    crate::store::turn_timeouts::floor::retain(&store, &mut session)
        .await
        .unwrap();
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":{
        "record.conversation.turn_timeout":mongodb::bson::serialize_to_bson(&timeout).unwrap()
    }}).session(&mut session).await.unwrap();
    assert!(matches!(
        crate::store::sandbox_mongo::commit_transaction(&mut session).await,
        crate::store::sandbox_mongo::CommitOutcome::Committed
    ));
    assert!(discovery::page(&store, None, now_ms).await.is_err());
    client
        .database(&name)
        .run_command(doc! {
            "createIndexes":"agents", "indexes":[discovery::index()]
        })
        .await
        .unwrap();
    let due = discovery::page(&store, None, now_ms).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, agent.id);
    assert!(discovery::page(&store, due.first(), now_ms)
        .await
        .unwrap()
        .is_empty());
    assert!(discovery::page(&store, None, now_ms - 1)
        .await
        .unwrap()
        .is_empty());
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    let (output_acks, mut acknowledgements) = tokio::sync::mpsc::channel(4);
    let output_receipt = layer3_adapter::RuntimeOutputReceipt::new(
        layer3_adapter::RuntimeOutputSource {
            stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
            cursor: 1,
            native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
            payload_sha256: "a".repeat(64),
        },
        output_acks.clone(),
    );
    recorder
        .record_update(
            &agent.id,
            dispatch.runtime_generation,
            &mut assembly,
            ConversationUpdate::RuntimeOutput {
                receipt: output_receipt.clone(),
                update: Some(layer3_adapter::RuntimeOutputUpdate::new(
                    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        ContentBlock::from("retained partial reply"),
                    ))),
                    false,
                )),
            },
            now.clone(),
        )
        .await
        .unwrap();
    let snapshot = store
        .agent_message_snapshot(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot
            .checkpoint
            .as_ref()
            .unwrap()
            .output
            .as_ref()
            .unwrap()
            .cursor,
        1
    );
    // The transport consumer acknowledges the actual durable output receipt.
    // Policy publication below must neither move this cursor nor emit another ACK.
    output_receipt.acknowledge().await.unwrap();
    assert_eq!(acknowledgements.try_recv().unwrap().cursor, 1);
    let proof = TurnTimeoutSettlement::prepare(snapshot, command.clone(), 1, now.clone()).unwrap();
    assert_eq!(
        store
            .commit_agent_conversation(proof.mutation().clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .commit_agent_message_update(
                proof.snapshot().agent.clone(),
                proof.snapshot().checkpoint.clone(),
                proof.checkpoint().clone(),
                Some(proof.mutation().clone()),
            )
            .await,
        Err(SandboxStoreError::Conflict)
    );
    if !trusted {
        assert_eq!(
            store.commit_turn_timeout(&proof).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store
                .agent_message_snapshot(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .agent,
            proof.snapshot().agent
        );
        assert!(!store
            .get_command(id)
            .await
            .unwrap()
            .unwrap()
            .status
            .is_terminal());
        client.database(&name).drop().await.unwrap();
        return;
    }
    if result_first {
        recorder
            .record_update(
                &agent.id,
                dispatch.runtime_generation,
                &mut assembly,
                ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
                now,
            )
            .await
            .unwrap();
        assert!(store.commit_turn_timeout(&proof).await.is_err());
        assert_eq!(
            store.reconcile_command(id).await.unwrap().status,
            AgentResponseStatus::Completed
        );
    } else {
        let peer = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        if let Some(committed) = uncertain {
            let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
            let admin = Client::with_uri_str(&uri).await.unwrap();
            let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
            let app = format!("turn-policy-{}", uuid::Uuid::new_v4().simple());
            options.app_name = Some(app.clone());
            let candidate = Client::with_options(options).unwrap();
            let uncertain_store = MongoSandboxStore::open(&candidate, &name).await.unwrap();
            let mut data = if committed {
                doc! {"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"synthetic uncertain policy commit"}}
            } else {
                doc! {"errorCode":50_i32,"errorLabels":["UnknownTransactionCommitResult"]}
            };
            data.insert("appName", app);
            data.insert("failCommands", vec!["commitTransaction"]);
            let before = admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":{"times":5_i32}, "data":data
                })
                .await
                .unwrap();
            let outcome = uncertain_store.commit_turn_timeout(&proof).await;
            let after = admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":"off"
                })
                .await
                .unwrap();
            let entries = |reply: mongodb::bson::Document| {
                reply
                    .get_i64("count")
                    .or_else(|_| reply.get_i32("count").map(i64::from))
                    .unwrap()
            };
            assert_eq!(entries(after) - entries(before), 5);
            if committed {
                // Injecting a write-concern error also skips the majority wait.
                // A bounded readback may therefore still be inconclusive. Prove
                // this transaction's receipt becomes majority-visible before
                // allowing any retry to perform another mutation.
                assert!(
                    outcome.is_ok() || matches!(outcome, Err(SandboxStoreError::Unavailable(_))),
                    "{outcome:?}"
                );
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        let row = store
                            .agent_commands
                            .find_one(doc! {"_id":id.as_str()})
                            .await
                            .unwrap()
                            .unwrap();
                        if row.contains_key("turn_policy_publication") {
                            assert_eq!(
                                crate::store::agent_commands::records::read(&row)
                                    .unwrap()
                                    .status,
                                AgentResponseStatus::Failed
                            );
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("original policy transaction did not become majority-visible");
            } else {
                assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
            }
        } else {
            let (a, b) = tokio::join!(
                store.commit_turn_timeout(&proof),
                crate::sandbox::turn_timeout_worker::settle_candidate(&peer, &due[0])
            );
            assert!(a.is_ok() || b.is_ok(), "{a:?} {b:?}");
        }
        // Independent process state must not be needed to recognize the commit.
        let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
        if uncertain == Some(false) {
            // A lost commit reply does not abort the server transaction. A new
            // worker retains the action while its predecessor's locks expire.
            tokio::time::timeout(std::time::Duration::from_secs(120), async {
                loop {
                    match crate::sandbox::turn_timeout_worker::settle_candidate(&restarted, &due[0])
                        .await
                    {
                        Ok(()) => break,
                        Err(SandboxStoreError::Conflict | SandboxStoreError::Unavailable(_)) => {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                        Err(error) => {
                            panic!("unexpected timeout reconciliation refusal: {error:?}")
                        }
                    }
                }
            })
            .await
            .expect("durable policy reconciliation remained blocked after transaction expiry");
        }
        restarted.commit_turn_timeout(&proof).await.unwrap();
        assert!(matches!(
            acknowledgements.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        assert!(discovery::page(&restarted, None, i64::MAX)
            .await
            .unwrap()
            .is_empty());
        let failed = restarted.get_command(id).await.unwrap().unwrap();
        assert_eq!(failed.status, AgentResponseStatus::Failed);
        assert_eq!(
            failed.work.observation.as_ref().unwrap().delivery,
            command.work.observation.as_ref().unwrap().delivery
        );
        let terminal = restarted.get_agent(&agent.id).await.unwrap().unwrap();
        let mut erased = terminal.clone();
        erased.revision += 1;
        erased.conversation.turn_timeout = None;
        assert_eq!(
            restarted.save_agent(terminal.revision, erased).await,
            Err(SandboxStoreError::Conflict)
        );
        assert!(terminal.conversation.active_turn.is_none());
        assert!(terminal
            .conversation
            .turn_timeout
            .as_ref()
            .unwrap()
            .blocks_dispatch());
        let logs = restarted.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(logs.iter().filter(|row| matches!(&row.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_turn_timeout")).count(), 1);
        assert!(logs.iter().any(|row| matches!(&row.kind, AgentLogKind::AgentMessage {content} if content == "retained partial reply")));
        assert_eq!(
            restarted
                .agent_message_checkpoint(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .output,
            proof.snapshot().checkpoint.as_ref().unwrap().output
        );
        let second = record(
            restarted
                .reserve_command(live_request(&terminal, &parent, "timeout-two"))
                .await
                .unwrap(),
        );
        assert!(restarted
            .claim_next_command(claim_request(
                &terminal,
                "next-worker",
                &layer2_core::now_rfc3339()
            ))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            restarted
                .get_command(&second.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentResponseStatus::Queued
        );
        crate::store::turn_timeouts::floor::verify(&client.database(&name))
            .await
            .unwrap();
        crate::store::work_schema::floor::verify(&client.database(&name))
            .await
            .unwrap();
        // The retained marker, not the lifetime of one Agent, establishes rollback floor.
        restarted
            .agents
            .delete_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap();
        crate::store::turn_timeouts::floor::verify(&client.database(&name))
            .await
            .unwrap();
        assert!(client
            .database(&name)
            .collection::<mongodb::bson::Document>("sandbox_credentials")
            .find_one(doc! {"document_schema_version":{"$nin":[
                1_i64,
                crate::store::work_schema::RENEWED_VERSION,
                crate::store::work_schema::PREPARED_VERSION,
            ]}})
            .await
            .unwrap()
            .is_some());
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}
