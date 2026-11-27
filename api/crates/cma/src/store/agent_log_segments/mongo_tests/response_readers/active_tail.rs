use super::*;
use futures_util::FutureExt as _;
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, SandboxStoreError};
use std::panic::AssertUnwindSafe;

mod fixture;
mod measure;
mod replay_interleavings;

#[tokio::test]
#[ignore = "requires a disposable replica set with profiler access"]
async fn retained_reader_characterizes_active_tail_cost_with_fixed_output() {
    let mut expected = None;
    for round in 0..5 {
        for case in fixture::CASES {
            let output = run(case, round, false).await;
            if let Some(expected) = &expected {
                assert_eq!(
                    &output, expected,
                    "retained output bytes change with active history"
                );
            } else {
                expected = Some(output);
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set with profiler access"]
async fn keyed_append_characterizes_active_tail_cost_and_replays_original_receipt() {
    for round in 0..5 {
        for case in fixture::CASES {
            run(case, round, true).await;
        }
    }
}

async fn run(case: fixture::Case, round: usize, append: bool) -> Vec<u8> {
    let (client, trace) = measure::client().await;
    let name = format!("cma_active_tail_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let result = AssertUnwindSafe(async {
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let fixture = fixture::install(&store, case).await;
        let store = prepare_active_proof(store, &db, &fixture.agent).await;
        let persisted = store.agent_log.count_documents(doc! {}).await.unwrap();
        assert_eq!(persisted, case.rows as u64 + 256);
        if append {
            let kind = AgentLogKind::ModeChanged {
                mode: "appended value".into(),
            };
            let identity = AgentLogPublicationIdentity {
                key: "active-tail-original".into(),
                appended_at: NOW.into(),
                fingerprint: segments::publication::request_digest(&kind)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
            };
            let (entry, measured) = measure::sample(
                &db,
                &trace,
                case,
                &fixture,
                round,
                "append",
                store.append_agent_log_keyed(&fixture.agent.id, &identity, kind.clone()),
            )
            .await;
            let entry = entry.expect("original keyed append succeeds");
            assert!(
                measured.docs_examined <= 100,
                "ordinary append examined {} documents",
                measured.docs_examined
            );
            assert!(measured.wire.reply_bytes < 2 * 1024 * 1024);
            assert_eq!(measured.before.active_count, case.rows);
            assert_eq!(measured.before.active_bytes, fixture.active_bytes);
            assert_eq!(measured.after.last_seq, measured.before.last_seq + 1);
            assert!(
                !measured.rollover,
                "fixture must retain its near-cap active tail"
            );
            assert_eq!(measured.after.active_count, case.rows + 1);
            let appended_shape = measured.after;
            assert_eq!(entry.seq, fixture.agent.conversation.last_seq + 1);
            assert_eq!(entry.kind, kind);
            assert_eq!(measured.wire.failed_commands, 0);
            assert!(measured
                .wire
                .commands
                .get("commitTransaction")
                .is_some_and(|count| *count > 0));
            let (replay, measured) = measure::sample(
                &db,
                &trace,
                case,
                &fixture,
                round,
                "replay",
                store.append_agent_log_keyed(&fixture.agent.id, &identity, kind),
            )
            .await;
            assert_eq!(replay.unwrap(), entry);
            assert!(
                measured.docs_examined <= 100,
                "keyed replay examined {} documents",
                measured.docs_examined
            );
            assert_eq!(measured.before, appended_shape);
            assert_no_writes(&measured);
            assert_eq!(
                store.agent_log.count_documents(doc! {}).await.unwrap(),
                persisted + 1
            );
            assert_eq!(
                collection(&store, PUBLICATIONS)
                    .count_documents(doc! {
                        "agent_id":fixture.agent.id.as_str(),"publication_key":&identity.key,
                    })
                    .await
                    .unwrap(),
                1
            );
            serde_json::to_vec(&entry).unwrap()
        } else {
            let (actual, measured) = measure::sample(
                &db,
                &trace,
                case,
                &fixture,
                round,
                "reader",
                read_target(&store, &fixture.agent),
            )
            .await;
            let actual =
                actual.expect("complete retained source opens and yields the requested output");
            let expected = [
                [3, 202, 203, 204]
                    .into_iter()
                    .map(|seq| fixture.target[seq - 1].clone())
                    .collect::<Vec<_>>(),
                fixture.target[3..35].to_vec(),
            ]
            .concat();
            assert_eq!(actual, expected);
            assert_eq!(measured.before.active_count, case.rows);
            assert_eq!(measured.before.active_bytes, fixture.active_bytes);
            assert_eq!(measured.consumed_source[1], 0);
            assert!(
                measured.consumed_source[2] >= 36,
                "the requested output must be consumed"
            );
            assert!(measured.docs_examined > 0 && measured.keys_examined > 0);
            assert_no_writes(&measured);
            assert_eq!(
                store.agent_log.count_documents(doc! {}).await.unwrap(),
                persisted
            );
            let (page, measured) =
                measure::sample(&db, &trace, case, &fixture, round, "page", async {
                    store
                        .agent_log_page_projection(&fixture.agent, 3, 200)
                        .await?
                        .ok_or_else(|| SandboxStoreError::Unavailable("source page absent".into()))
                })
                .await;
            let page = page.expect("the public log port returns the requested 200-entry page");
            assert_eq!(page, fixture.target[3..203]);
            assert_eq!(measured.before.active_count, case.rows);
            assert_eq!(measured.before.active_bytes, fixture.active_bytes);
            assert!(measured.consumed_source[2] >= 200);
            assert_eq!(measured.consumed_source[1], 0);
            assert!(measured.docs_examined > 0 && measured.keys_examined > 0);
            assert!(
                measured.docs_examined <= 250,
                "200-entry sealed page examined {} documents in {}",
                measured.docs_examined,
                case.name
            );
            assert_no_writes(&measured);
            let (active_page, measured) =
                measure::sample(&db, &trace, case, &fixture, round, "page", async {
                    store
                        .agent_log_page_projection(&fixture.agent, 256, 200)
                        .await?
                        .ok_or_else(|| SandboxStoreError::Unavailable("active page absent".into()))
                })
                .await;
            let active_page = active_page.unwrap();
            assert!(!active_page.is_empty());
            assert_eq!(active_page, fixture.active[..active_page.len()]);
            assert!(active_page.len() <= 200);
            assert!(serde_json::to_vec(&active_page).unwrap().len() <= 4 * 1024 * 1024);
            assert!(
                measured.docs_examined <= 250,
                "200-entry active page examined {} documents in {}",
                measured.docs_examined,
                case.name
            );
            assert_no_writes(&measured);
            // The final sealed row and first active row use one captured source.
            let boundary = store
                .agent_log_page_projection(&fixture.agent, 255, 2)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                boundary,
                [fixture.target[255].clone(), fixture.active[0].clone()]
            );
            serde_json::to_vec(&[actual, page].concat()).unwrap()
        }
    })
    .catch_unwind()
    .await;
    let cleanup = db.drop().await;
    match result {
        Ok(value) => {
            cleanup.expect("remove owned fixture database");
            value
        }
        Err(original) => {
            if let Err(error) = cleanup {
                eprintln!("fixture cleanup after original failure: {error}");
            }
            std::panic::resume_unwind(original)
        }
    }
}

async fn read_target(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut reader = store
        .agent_output_reader(agent, Some(3))
        .await?
        .ok_or_else(|| SandboxStoreError::Unavailable("retained reader absent".into()))?;
    let turn = reader
        .metadata()
        .turn
        .clone()
        .expect("target turn metadata");
    assert_eq!(turn.facts.model.as_deref(), Some("original-model"));
    assert_eq!(turn.terminal_sequence, Some(203));
    assert_eq!(turn.usage_sequence, Some(204));
    let changes = reader.changes(0, 8).await?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].source_sequence, 202);
    let mut rows = reader.points(&[3, 202, 203, 204]).await?;
    let page = reader.source_page(3, 32).await?;
    assert_eq!(page.len(), 32);
    rows.extend(page);
    assert!(reader.changes(1, 8).await?.is_empty());
    Ok(rows)
}

fn assert_no_writes(observed: &measure::Observation) {
    assert_eq!(observed.before, observed.after);
    assert_eq!(observed.wire.failed_commands, 0);
    for name in [
        "insert",
        "update",
        "delete",
        "findAndModify",
        "commitTransaction",
        "abortTransaction",
    ] {
        assert!(
            !observed.wire.commands.contains_key(name),
            "unexpected {name} in {}",
            observed.operation
        );
    }
}

async fn prepare_active_proof(
    store: MongoSandboxStore,
    db: &mongodb::Database,
    agent: &Agent,
) -> MongoSandboxStore {
    use crate::store::migrations::agent_log_active_proofs as migration;
    for (_, index) in migration::inspect(db).await.unwrap() {
        migration::create(db, index).await.unwrap();
    }
    let mut store = store;
    store.active_log_proofs = true;
    super::super::super::active_proof::prepare(&store.client, db, agent.id.as_str())
        .await
        .unwrap();
    store
}

#[tokio::test]
#[ignore = "requires a disposable replica set with profiler access"]
async fn active_proof_rejects_bad_leaves_and_recovers_after_predecessor_publication() {
    let (client, trace) = measure::client().await;
    let name = format!("cma_active_proof_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let old = MongoSandboxStore::open(&client, &name).await.unwrap();
    let case = fixture::CASES[1];
    let fixture = fixture::install(&old, case).await;
    let active = prepare_active_proof(old.clone(), &db, &fixture.agent).await;
    let source = db.collection::<mongodb::bson::RawDocumentBuf>("agent_log");
    let filter = doc! {"agent_id":fixture.agent.id.as_str(),"seq":257_i64};
    let original = source.find_one(filter.clone()).await.unwrap().unwrap();
    db.collection::<Document>("agent_log")
        .update_one(
            filter.clone(),
            doc! {"$set":{"record.appended_at":"2026-11-02T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert!(active
        .agent_log_page_projection(&fixture.agent, 256, 1)
        .await
        .is_err());
    source.replace_one(filter, original).await.unwrap();
    assert_eq!(
        active
            .agent_log_page_projection(&fixture.agent, 256, 1)
            .await
            .unwrap()
            .unwrap(),
        fixture.active[..1]
    );
    let kind = AgentLogKind::ModeChanged {
        mode: "predecessor".into(),
    };
    let identity = AgentLogPublicationIdentity {
        key: "predecessor-publication".into(),
        appended_at: NOW.into(),
        fingerprint: segments::publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    };
    let (preparation, appended) = tokio::join!(
        super::super::super::active_proof::prepare(&client, &db, fixture.agent.id.as_str()),
        old.append_agent_log_keyed(&fixture.agent.id, &identity, kind.clone()),
    );
    let appended = match appended {
        Ok(entry) => entry,
        Err(_) => old
            .append_agent_log_keyed(&fixture.agent.id, &identity, kind)
            .await
            .unwrap(),
    };
    let _ = preparation;
    let row = old
        .agents
        .find_one(doc! {"_id":fixture.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let current = crate::store::agents::read_agent(&row).unwrap();
    assert_eq!(current.conversation.last_seq, appended.seq);
    // The predecessor did not update the new collection. The current reader
    // must fall back to the original full validation before a fresh preparation.
    let mut restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    restarted.active_log_proofs = true;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let proof = segments::capture::capture(
        &restarted,
        &current,
        &mut session,
        segments::capture::CaptureMode::Snapshot,
    )
    .await
    .unwrap();
    assert_eq!(proof.latest_entry(), Some(&appended));
    super::super::super::active_proof::prepare(&client, &db, current.id.as_str())
        .await
        .unwrap();
    let current_fixture = fixture::Fixture {
        agent: current.clone(),
        active_bytes: fixture.active_bytes,
        target: fixture.target,
        active: fixture.active,
    };
    let (page, observed) = measure::sample(&db, &trace, case, &current_fixture, 0, "page", async {
        restarted
            .agent_log_page_projection(&current, current.conversation.last_seq - 1, 1)
            .await?
            .ok_or_else(|| SandboxStoreError::Unavailable("active page absent".into()))
    })
    .await;
    assert_eq!(page.unwrap(), vec![appended]);
    assert!(observed.docs_examined <= 50);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set with profiler access"]
async fn early_sealed_page_refuses_missing_or_mismatched_terminal_seal() {
    let (client, trace) = measure::client().await;
    let name = format!("cma_sealed_boundary_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, rows) = super::super::fixtures::segmented_with_width(&store, 3, 256, 100).await;
    super::super::fixtures::certify(&store, &agent, &rows).await;
    let f = fixture::Fixture {
        agent: agent.clone(),
        target: rows[..256].to_vec(),
        active: rows[768..].to_vec(),
        active_bytes: 0,
    };
    let (page, observed) = measure::sample(&db, &trace, fixture::CASES[0], &f, 0, "page", async {
        store
            .agent_log_page_projection(&agent, 3, 200)
            .await?
            .ok_or_else(|| SandboxStoreError::Unavailable("sealed page absent".into()))
    })
    .await;
    assert_eq!(page.unwrap(), rows[3..203]);
    assert!(observed.docs_examined <= 250);
    let seals = db.collection::<Document>(SEGMENTS);
    let filter = doc! {"agent_id":agent.id.as_str(),"ordinal":2_i64};
    let terminal = seals.find_one(filter.clone()).await.unwrap().unwrap();
    seals.delete_one(filter).await.unwrap();
    assert!(store
        .agent_log_page_projection(&agent, 3, 200)
        .await
        .is_err());
    seals.insert_one(terminal).await.unwrap();
    let heads = db.collection::<Document>(HEADS);
    heads
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":{"previous_seal":segments::digest::binary([0_u8;32])}},
        )
        .await
        .unwrap();
    assert!(store
        .agent_log_page_projection(&agent, 3, 200)
        .await
        .is_err());
    db.drop().await.unwrap();
}
