use super::super::super::budget_tests::configure_metered;
use super::*;
use crate::store::{
    agent_log_segments::{self as segments, publication, writer_gate as gate},
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::store::{AgentLogPublicationIdentity, SandboxStoreError, StaleTurnSettlement},
};
use layer3_adapter::{
    stale_turn::{SealedOutput, StaleTurnSeal, SEAL_VERSION},
    AvailableCommandsUpdate, ContentBlock, ContentChunk, ConversationUpdate, RuntimeOutputReceipt,
    RuntimeOutputSource, SessionUpdate,
};

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_seal_settles_once_and_replays_after_checkpoint_advance() {
    for segmented in [false, true] {
        Box::pin(exercise_settlement(segmented, None)).await;
    }
}

#[tokio::test]
#[ignore = "requires the failpoint URI of a disposable MongoDB replica set"]
async fn stale_turn_seal_reconciles_an_unknown_commit() {
    let application = format!("stale-turn-commit-{}", uuid::Uuid::new_v4().simple());
    Box::pin(exercise_settlement(false, Some(&application))).await;
}

async fn exercise_settlement(segmented: bool, ambiguous: Option<&str>) {
    let (client, name) = database("stale_turn_terminal").await;
    let mut parent = active_sandbox("sbx_stale_turn");
    parent.lifecycle.home = Some(Box::new(
        crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home(&parent.id),
    ));
    let runtime = parent.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    runtime.boot_id = Some("b".repeat(32));
    let (original, agent, parent, execution, _, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        parent,
        Some((6, false)),
    )
    .await;
    if segmented {
        configure_metered(&original, &agent.owner).await;
        adopt(&client, &name, &original, &agent).await;
    }
    let ready = original.get_agent(&agent.id).await.unwrap().unwrap();
    let first = record(
        original
            .reserve_command(live_request(&ready, &parent, "seal-one"))
            .await
            .unwrap(),
    );
    let id = &first.receipt.envelope.command_id;
    let worker =
        AgentCommandWorker::new(original.clone(), execution, "sealed-worker".into()).unwrap();
    worker.run_one(id).await.unwrap();
    original.reconcile_command(id).await.unwrap();
    let current = original.get_agent(&agent.id).await.unwrap().unwrap();
    let budget_before = if segmented {
        Some(budget_turn(&original, &current).await)
    } else {
        None
    };
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    let writer = if let Some(application) = ambiguous {
        let mut options =
            ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                .await
                .unwrap();
        options.app_name = Some(application.to_owned());
        Client::with_options(options).unwrap()
    } else {
        client.clone()
    };
    let store = replica(&writer, &name).await;
    let peer_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let peer = replica(&peer_client, &name).await;
    let manager = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        Member {
            instance_id: "test-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 1,
        },
    )
    .await
    .unwrap();
    let owner = manager.acquire(&agent.id).await.unwrap().unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let (acks, _unused) = tokio::sync::mpsc::channel(4);
    let source = |cursor| RuntimeOutputSource {
        stream_id: "b7111111-1111-4111-8111-111111111111".into(),
        cursor,
        native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
        payload_sha256: format!("{cursor:064x}"),
    };
    let output = |cursor, update: ConversationUpdate| ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(source(cursor), acks.clone()),
        update: Some(Box::new(update).into()),
    };
    let mut assembly = AgentMessageAssembly::default();
    owner
        .run(async {
            recorder
                .record_update(
                    &agent.id,
                    1,
                    &mut assembly,
                    output(
                        1,
                        ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                            ContentChunk::new(ContentBlock::from("retained prefix")),
                        )),
                    ),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
            recorder
                .record_update(
                    &agent.id,
                    1,
                    &mut assembly,
                    output(
                        2,
                        ConversationUpdate::Said(SessionUpdate::AvailableCommandsUpdate(
                            AvailableCommandsUpdate::new(vec![]),
                        )),
                    ),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
        })
        .await
        .unwrap();

    let scan = StaleTurnStore::new(store.clone());
    let other = StaleTurnStore::new(peer.clone());
    let row = store
        .agent_commands
        .find_one(doc! {"_id": id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let future = DateTime::from_millis(DateTime::now().timestamp_millis() + 16 * 60 * 1000);
    let candidate = Candidate::from_row(&row, future).unwrap();
    let Observation::Quiet { evidence, .. } = scan.observe(&candidate, future).await.unwrap()
    else {
        panic!("expected a quiet turn with a durable checkpoint");
    };
    let target = StaleTurnSettlement::target(
        &evidence.snapshot,
        &evidence.parent,
        &evidence.command,
        &owner.token,
    )
    .unwrap();
    let seal = StaleTurnSeal {
        version: SEAL_VERSION,
        prompt_id: target.prompt().as_str().to_owned(),
        target,
        stop_receipt_id: "synthetic-seal".into(),
        output: SealedOutput::Through { source: source(2) },
    };
    let mut mismatched = seal.clone();
    mismatched.output = SealedOutput::Through { source: source(3) };
    let prepare = |seal, parent| {
        StaleTurnSettlement::prepare(
            evidence.snapshot.clone(),
            parent,
            evidence.command.clone(),
            owner.token.clone(),
            seal,
            layer2_core::now_rfc3339(),
        )
    };
    assert!(matches!(
        prepare(mismatched, evidence.parent.clone()),
        Err(SandboxStoreError::Conflict)
    ));
    let stale_parent = prepare(seal.clone(), evidence.parent.clone()).unwrap();
    let mut changed = evidence.parent.clone();
    changed.revision += 1;
    changed.updated_at = layer2_core::now_rfc3339();
    peer.save_sandbox(evidence.parent.revision, changed.clone())
        .await
        .unwrap();
    owner
        .run(async {
            assert_eq!(
                scan.settle(&stale_parent).await,
                Err(SandboxStoreError::Conflict)
            );
        })
        .await
        .unwrap();
    let proof = prepare(seal, changed).unwrap();
    assert_eq!(scan.settle(&proof).await, Err(SandboxStoreError::Conflict));
    owner
        .run(async {
            if let Some(application) = ambiguous {
                let admin = client.database("admin");
                let entries = |reply: mongodb::bson::Document| {
                    reply
                        .get_i64("count")
                        .or_else(|_| reply.get_i32("count").map(i64::from))
                        .unwrap()
                };
                let before = admin
                    .run_command(doc! {
                        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
                        "data":{"failCommands":["commitTransaction"],"appName":application,
                        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
                        "errmsg":"injected stale-turn commit ambiguity"}}
                    })
                    .await
                    .unwrap();
                let result = scan.settle(&proof).await;
                let after = admin
                    .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
                    .await
                    .unwrap();
                assert_eq!(entries(after) - entries(before), 5);
                result.unwrap();
            } else {
                // Two clients contend for the same turn with the same qualified writer.
                let (left, right) = tokio::join!(scan.settle(&proof), other.settle(&proof));
                assert!(left.is_ok() || right.is_ok(), "{left:?}, {right:?}");
                assert!(
                    left.is_ok() || left == Err(SandboxStoreError::Conflict),
                    "{left:?}"
                );
                assert!(
                    right.is_ok() || right == Err(SandboxStoreError::Conflict),
                    "{right:?}"
                );
            }
            assert_eq!(
                peer.reconcile_command(id).await.unwrap().status,
                AgentResponseStatus::Failed
            );
            recorder
                .record_update(
                    &agent.id,
                    1,
                    &mut assembly,
                    output(
                        3,
                        ConversationUpdate::Said(SessionUpdate::AvailableCommandsUpdate(
                            AvailableCommandsUpdate::new(vec![]),
                        )),
                    ),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
            assert_ne!(
                peer.agent_message_checkpoint(&agent.id)
                    .await
                    .unwrap()
                    .as_ref(),
                Some(proof.checkpoint())
            );
            other.settle(&proof).await.unwrap();
        })
        .await
        .unwrap();
    let result = peer.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(result.status, AgentStatus::Ready);
    assert_eq!(result.conversation.active_turn, None);
    if let Some(mut expected) = budget_before {
        let reservation = expected.get_document_mut("reservation").unwrap();
        assert!(!reservation.get_bool("settled").unwrap());
        reservation.insert("settled", true);
        assert_eq!(budget_turn(&peer, &current).await, expected);
    }
    let logs = peer.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_turn_no_progress")).count(), 1);
    assert!(logs.iter().any(|entry| matches!(&entry.kind, AgentLogKind::AgentMessage {content} if content == "retained prefix")));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}

async fn budget_turn(store: &MongoSandboxStore, agent: &Agent) -> mongodb::bson::Document {
    let key = crate::store::budget::turn_key(agent, agent.conversation.active_turn.unwrap());
    store
        .model_budget
        .find_one(doc! {"_id": key})
        .await
        .unwrap()
        .unwrap()
        .get_document("record")
        .unwrap()
        .clone()
}

async fn replica(client: &Client, name: &str) -> Arc<MongoSandboxStore> {
    Arc::new(
        MongoSandboxStore::open(client, name)
            .await
            .unwrap()
            .enable_agent_ownership("test-owner".into())
            .await
            .unwrap(),
    )
}

async fn adopt(client: &Client, name: &str, store: &MongoSandboxStore, agent: &Agent) {
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
                key: "stale-turn-adoption".into(),
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
