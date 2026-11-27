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

mod competing;
mod publication_diagnostics;
use competing::{budget_turn, Competition, Race, SealProbe};

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_seal_settles_once_and_replays_after_checkpoint_advance() {
    for segmented in [false, true] {
        Box::pin(exercise_settlement(segmented, None, None, false)).await;
    }
}

#[tokio::test]
#[ignore = "requires the failpoint URI of a disposable MongoDB replica set"]
async fn stale_turn_seal_reconciles_an_unknown_commit() {
    let application = format!("stale-turn-commit-{}", uuid::Uuid::new_v4().simple());
    Box::pin(exercise_settlement(false, Some(&application), None, true)).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_seal_loses_to_competing_writers() {
    for race in [Race::Canonical, Race::Interrupt, Race::Successor] {
        Box::pin(exercise_settlement(true, None, Some(race), false)).await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_seal_refuses_cumulative_evidence_over_budget() {
    Box::pin(exercise_settlement(true, None, Some(Race::Oversize), false)).await;
}

async fn exercise_settlement(
    segmented: bool,
    ambiguous: Option<&str>,
    race: Option<Race>,
    partial: bool,
) {
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
        Some((competing::history_rows(race), false)),
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

    let seal_probe = (race == Some(Race::Predecessor)).then(SealProbe::default);
    let writer = if let Some(probe) = &seal_probe {
        probe.client().await
    } else if let Some(application) = ambiguous {
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
    let mut final_cursor = 2;
    owner
        .run(async {
            let boundary_effect = if partial {
                ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::from(""),
                )))
            } else {
                ConversationUpdate::Said(SessionUpdate::AvailableCommandsUpdate(
                    AvailableCommandsUpdate::new(vec![]),
                ))
            };
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
                    output(2, boundary_effect),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
            if race == Some(Race::Oversize) {
                let mut text = "a".repeat(220 * 1024);
                for cursor in [3, 5] {
                    recorder
                        .record_update(
                            &agent.id,
                            1,
                            &mut assembly,
                            output(
                                cursor,
                                ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                                    ContentChunk::new(ContentBlock::from(std::mem::take(
                                        &mut text,
                                    ))),
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
                                cursor + 1,
                                ConversationUpdate::Said(SessionUpdate::AvailableCommandsUpdate(
                                    AvailableCommandsUpdate::new(vec![]),
                                )),
                            ),
                            layer2_core::now_rfc3339(),
                        )
                        .await
                        .unwrap();
                    text = "b".repeat(220 * 1024);
                }
                final_cursor = 7;
                recorder
                    .record_update(
                        &agent.id,
                        1,
                        &mut assembly,
                        output(
                            final_cursor,
                            ConversationUpdate::Said(SessionUpdate::AvailableCommandsUpdate(
                                AvailableCommandsUpdate::new(vec![]),
                            )),
                        ),
                        layer2_core::now_rfc3339(),
                    )
                    .await
                    .unwrap();
            }
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
    if partial && race == Some(Race::Canonical) {
        crate::sandbox::assert_partial_does_not_request_stop(
            store.clone(),
            manager,
            &owner,
            &candidate,
            future,
        )
        .await;
    }
    let mut budget = ProofBudget::default();
    let Observation::Quiet { evidence, .. } = scan
        .observe_for_terminal(&candidate, future, &mut budget)
        .await
        .unwrap()
    else {
        panic!("expected a quiet turn with a durable checkpoint");
    };
    assert_eq!(
        !evidence
            .snapshot
            .checkpoint
            .as_ref()
            .unwrap()
            .assembly
            .is_empty(),
        partial
    );
    let target = StaleTurnSettlement::receipt_target(
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
        output: SealedOutput::Through {
            source: source(final_cursor),
        },
    };
    if let Some(race) = race {
        let proof = StaleTurnSettlement::prepare(
            evidence.snapshot,
            evidence.parent,
            evidence.command,
            owner.token.clone(),
            seal,
            layer2_core::now_rfc3339(),
        )
        .unwrap();
        let result = Competition {
            client: &peer_client,
            peer: &peer,
            name: &name,
            scan: &scan,
            owner: &owner,
            proof: &proof,
            source: source(final_cursor + 1),
            budget_before: budget_before.as_ref().unwrap(),
            seal_probe,
        };
        if race == Race::Oversize {
            assert!(matches!(
                scan.observe(&candidate, future, &mut budget).await.unwrap(),
                Observation::Quiet { .. }
            ));
        }
        Box::pin(result.run(race, &mut budget)).await;
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        client.database(&name).drop().await.unwrap();
        return;
    }
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
    let publication_log = publication_diagnostics::Log::capture();
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
                scan.settle(&stale_parent, &mut ProofBudget::default())
                    .await,
                Err(SandboxStoreError::Conflict)
            );
        })
        .await
        .unwrap();
    let proof = prepare(seal, changed).unwrap();
    assert_eq!(
        scan.settle(&proof, &mut ProofBudget::default()).await,
        Err(SandboxStoreError::Conflict)
    );
    publication_log.assert_empty();
    owner
        .run(async {
            if let Some(application) = ambiguous {
                competing::settle_with_unknown_commit(&client, application, &scan, &proof).await;
            } else {
                let mut local_budget = ProofBudget::default();
                let mut peer_budget = ProofBudget::default();
                let (left, right) = tokio::join!(
                    scan.settle(&proof, &mut local_budget),
                    other.settle(&proof, &mut peer_budget)
                );
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
            other
                .settle(&proof, &mut ProofBudget::default())
                .await
                .unwrap();
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
    publication_log
        .assert_terminal(&peer, &agent.id, id, ambiguous.is_some())
        .await;
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}

pub(super) async fn replica(client: &Client, name: &str) -> Arc<MongoSandboxStore> {
    Arc::new(
        MongoSandboxStore::open(client, name)
            .await
            .unwrap()
            .enable_agent_ownership("test-owner".into())
            .await
            .unwrap(),
    )
}

pub(in crate::store) async fn adopt(
    client: &Client,
    name: &str,
    store: &MongoSandboxStore,
    agent: &Agent,
) {
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

mod recorder_checkpoint;
