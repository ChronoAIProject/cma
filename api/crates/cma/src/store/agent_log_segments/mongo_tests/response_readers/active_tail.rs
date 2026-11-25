use super::*;
use futures_util::FutureExt as _;
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, SandboxStoreError};
use std::panic::AssertUnwindSafe;

mod fixture;
mod measure;

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
            assert_no_writes(&measured);
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
