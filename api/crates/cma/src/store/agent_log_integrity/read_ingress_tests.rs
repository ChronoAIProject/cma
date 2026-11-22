use super::super::super::proof::summary_tests::{metadata, policy};
use super::*;
use crate::store::agents::agent_document;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use crate::store::sandbox_mongo::start_transaction;

fn plan() -> AllocationPlan {
    AllocationPlan::checked("proven Agent source", &[50 * 1024 * 1024]).unwrap()
}

fn summary_row(agent: &Agent) -> Document {
    let mut row = agent_document(agent).unwrap();
    row.insert(
        INTEGRITY_FIELD,
        metadata(agent, [0; HASH_BYTES]).to_document().unwrap(),
    );
    row
}

async fn empty_agent(store: &MongoSandboxStore, name: &str) -> Agent {
    let (_, mut agent) = fixture(store, name).await;
    agent.conversation.last_seq = 0;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    store
        .agent_log
        .delete_many(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap();
    store
        .agents
        .replace_one(doc! {"_id":agent.id.as_str()}, summary_row(&agent))
        .await
        .unwrap();
    agent
}

#[test]
fn reader_flags_preserve_envelope_validation_and_metadata_only_contexts() {
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for row in [
        doc! {"document_schema_version":1_i32},
        doc! {"document_schema_version":1_i64, QUARANTINE_FIELD:false},
        doc! {"document_schema_version":2_i32, "agent_log_format":"segmented_v1",
        "agent_log_incarnation":"a".repeat(64), "agent_log_head_revision":1_i64},
        doc! {"document_schema_version":3_i64},
        doc! {"document_schema_version":2_i64},
        doc! {"document_schema_version":1_i64, "agent_log_format":"segmented_v1"},
        doc! {QUARANTINE_FIELD:true},
        doc! {},
    ] {
        let raw = RawDocumentBuf::try_from(&row).unwrap();
        let expected = crate::store::agent_log_segments::read_format(&row);
        let actual = row::decode::<ReaderFlagsSchema<true>>(&raw, &policy);
        match (expected, actual) {
            (Ok(format), Ok(flags)) => {
                assert_eq!(flags.segmented, format.is_some());
                assert_eq!(flags.quarantined, row.contains_key(QUARANTINE_FIELD));
                let clone = flags.clone();
                let held = root.available_bytes();
                drop(flags);
                assert_eq!(root.available_bytes(), held);
                drop(clone);
            }
            (Err(expected), Err(row::RowError::Domain(actual))) => assert_eq!(*actual, expected),
            other => panic!("reader flags changed original validation: {other:?}"),
        }
        let flags = row::decode::<ReaderFlagsSchema<false>>(&raw, &policy).unwrap();
        assert!(!flags.segmented);
        assert_eq!(flags.quarantined, row.contains_key(QUARANTINE_FIELD));
        drop(flags);
        assert_eq!(root.available_bytes(), idle);
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn proven_agent_matches_legacy_format_refusal_and_feature_gating() {
    let (client, name, mut store) = database().await;
    let agent = empty_agent(&store, "sbx_proven_matrix").await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for percent in [0, 100] {
        store = store.with_fleet_log_percentage(percent);
        assert_eq!(store.bounded_log_enabled(&agent.id), percent == 100);
        for case in [
            "trusted",
            "absent",
            "future",
            "stale",
            "invalid",
            "quarantine",
            "segmented",
            "corrupt",
            "missing",
        ] {
            let mut row = summary_row(&agent);
            match case {
                "absent" => {
                    row.remove(INTEGRITY_FIELD);
                }
                "future" => {
                    row.get_document_mut(INTEGRITY_FIELD)
                        .unwrap()
                        .insert("format", 2_i64);
                }
                "stale" => {
                    row.get_document_mut(INTEGRITY_FIELD)
                        .unwrap()
                        .insert("bound_agent_revision", 0_i64);
                }
                "invalid" => {
                    row.get_document_mut(INTEGRITY_FIELD)
                        .unwrap()
                        .get_document_mut(SEMANTIC_FIELD)
                        .unwrap()
                        .insert("valid", false);
                }
                "quarantine" => {
                    row.insert(QUARANTINE_FIELD, false);
                }
                "segmented" => {
                    row.insert("document_schema_version", 2_i64);
                    row.insert("agent_log_format", "segmented_v1");
                    row.insert(
                        "agent_log_incarnation",
                        crate::store::agent_log_segments::incarnation(&agent),
                    );
                    row.insert("agent_log_head_revision", 1_i64);
                }
                "corrupt" => {
                    row.insert("last_seq", -1_i64);
                }
                _ => {}
            }
            store
                .agents
                .replace_one(doc! {"_id":agent.id.as_str()}, row.clone())
                .await
                .unwrap();
            let id = if case == "missing" {
                AgentId("agt_proven_absent".into())
            } else {
                agent.id.clone()
            };
            let mut session = client.start_session().snapshot(true).await.unwrap();
            let expected = super::super::read_proven_agent(&store, &id, &mut session).await;
            let timestamp = session.snapshot_time();
            let actual = read_proven_agent_admitted(&store, &id, &mut session, &policy, plan())
                .unwrap()
                .await
                .unwrap()
                .transpose();
            match (expected, actual) {
                (Ok(expected), Ok(actual)) => {
                    assert_eq!(
                        expected.as_ref().map(|(a, b)| (a, *b)),
                        actual.as_ref().map(|(a, b)| (&**a, *b)),
                        "{case}"
                    );
                    assert_eq!(
                        actual.as_ref().map(|(_, b)| *b),
                        (case != "missing").then_some(case == "trusted" && percent == 100)
                    );
                }
                (Err(expected), Err(actual)) => assert!(
                    matches!(&*actual, ReadFailure::Domain(error) if *error == expected),
                    "{case}"
                ),
                other => panic!("proven reader outcome changed for {case}: {other:?}"),
            }
            assert_eq!(session.snapshot_time(), timestamp);
            assert_eq!(root.available_bytes(), idle);
            assert_eq!(
                store
                    .agents
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap(),
                Some(row)
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_context_keeps_metadata_only_semantics_and_the_callers_transaction() {
    let (client, name, mut store) = database().await;
    let agent = empty_agent(&store, "sbx_context_matrix").await;
    let original = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    for percent in [0, 100] {
        store = store.with_fleet_log_percentage(percent);
        for case in ["trusted", "absent", "quarantine", "corrupt", "extra"] {
            let mut row =
                doc! {INTEGRITY_FIELD: metadata(&agent, [0; HASH_BYTES]).to_document().unwrap()};
            match case {
                "absent" => {
                    row.remove(INTEGRITY_FIELD);
                }
                "quarantine" => {
                    row.insert(QUARANTINE_FIELD, false);
                }
                "corrupt" => {
                    row.get_document_mut(INTEGRITY_FIELD)
                        .unwrap()
                        .insert("head_seq", -1_i64);
                }
                _ => {}
            }
            let raw = RawDocumentBuf::try_from(&row).unwrap();
            start_transaction(&mut session).await.unwrap();
            store
                .agent_log
                .insert_one(doc! {"_id":"context-marker"})
                .session(&mut session)
                .await
                .unwrap();
            if case == "extra" {
                store
                    .agent_log
                    .insert_one(doc! {"_id":"context-extra", "agent_id":agent.id.as_str()})
                    .session(&mut session)
                    .await
                    .unwrap();
            }
            let expected =
                super::super::legacy_proof_for_context(&store, &agent, &row, &mut session, false)
                    .await;
            let actual = legacy_proof_for_context_admitted(
                &store,
                &agent,
                &raw,
                &mut session,
                false,
                &policy,
                plan(),
            )
            .unwrap()
            .await
            .unwrap()
            .transpose();
            match (expected, actual) {
                (Ok(expected), Ok(actual)) => {
                    assert_eq!(*actual, expected);
                    assert_eq!(*actual, case == "trusted" && percent == 100);
                }
                (Err(expected), Err(actual)) => assert!(
                    matches!(&*actual, ReadFailure::Domain(error) if *error == expected),
                    "{case}"
                ),
                other => panic!("legacy context changed for {case}: {other:?}"),
            }
            super::super::super::proof::ingress::assert_open_transaction(
                &store,
                &mut session,
                "context-marker",
            )
            .await;
            session.abort_transaction().await.unwrap();
            assert_eq!(root.available_bytes(), idle);
        }
    }
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap(),
        original
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn proven_agent_retains_old_data_and_snapshot_after_later_replacement() {
    let (client, name, store) = database().await;
    let store = store.with_fleet_log_percentage(100);
    let agent = empty_agent(&store, "sbx_proven_snapshot").await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut pinned = client.start_session().snapshot(true).await.unwrap();
    let old = read_proven_agent_admitted(&store, &agent.id, &mut pinned, &policy, plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    let original = old.as_ref().unwrap().0.clone();
    assert!(old.as_ref().unwrap().1);
    let timestamp = pinned.snapshot_time();
    let mut next = agent.clone();
    next.revision += 1;
    next.profile.content.system_prompt = "n".repeat(128 * 1024);
    let other = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    other
        .database(&name)
        .collection::<Document>(store.agents.name())
        .replace_one(doc! {"_id":agent.id.as_str()}, summary_row(&next))
        .await
        .unwrap();
    let same = read_proven_agent_admitted(&store, &agent.id, &mut pinned, &policy, plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(*same.as_ref().unwrap().0, agent);
    assert_eq!(pinned.snapshot_time(), timestamp);
    let mut fresh = client.start_session().snapshot(true).await.unwrap();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "only old Agent fits",
                &[available - original.requested_bytes() - 4096],
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        read_proven_agent_admitted(&store, &agent.id, &mut fresh, &policy, plan())
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    drop(pressure);
    let new = read_proven_agent_admitted(&store, &agent.id, &mut fresh, &policy, plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(*new.as_ref().unwrap().0, next);
    assert!(new.as_ref().unwrap().0.requested_bytes() > original.requested_bytes());
    drop((old, same, new, pinned, fresh));
    assert_eq!(*original, agent);
    assert!(root.available_bytes() < idle);
    drop(original);
    assert_eq!(root.available_bytes(), idle);
    other.shutdown().await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn proof_refusal_cancellation_and_driver_failures_keep_original_ownership() {
    let (client, name, store) = database().await;
    let agent = empty_agent(&store, "sbx_proven_failure").await;
    let raw = RawDocumentBuf::try_from(&summary_row(&agent)).unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    drop(read_proven_agent_admitted(&store, &agent.id, &mut session, &policy, plan()).unwrap());
    drop(
        legacy_proof_for_context_admitted(
            &store,
            &agent,
            &raw,
            &mut session,
            false,
            &policy,
            plan(),
        )
        .unwrap(),
    );
    assert_eq!(root.available_bytes(), idle);
    start_transaction(&mut session).await.unwrap();
    store
        .agent_log
        .insert_one(doc! {"_id":"proof-refusal-marker"})
        .session(&mut session)
        .await
        .unwrap();
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked(
                    "proof pressure",
                    &[policy.budget(class).available_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        match legacy_proof_for_context_admitted(
            &store,
            &agent,
            &raw,
            &mut session,
            false,
            &policy,
            plan(),
        ) {
            Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
            Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
        }
        drop(pressure);
        super::super::super::proof::ingress::assert_open_transaction(
            &store,
            &mut session,
            "proof-refusal-marker",
        )
        .await;
    }
    let mut nested = doc! {"leaf":true};
    for _ in 0..66 {
        nested = doc! {"nested":nested};
    }
    let deep = RawDocumentBuf::try_from(&nested).unwrap();
    assert!(matches!(
        legacy_proof_for_context_admitted(
            &store,
            &agent,
            &deep,
            &mut session,
            false,
            &policy,
            plan()
        )
        .unwrap()
        .await,
        Err(AdmittedError::Unsupported(_))
    ));
    super::super::super::proof::ingress::assert_open_transaction(
        &store,
        &mut session,
        "proof-refusal-marker",
    )
    .await;
    session.abort_transaction().await.unwrap();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some("cma-proven-reader-error".into());
    let reader = mongodb::Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&reader, &name).await.unwrap();
    let admin = client.database("admin");
    for snapshot in [true, false] {
        let mut session = reader.start_session().await.unwrap();
        if !snapshot {
            start_transaction(&mut session).await.unwrap();
        }
        admin.run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
            "failCommands":["find"], "appName":"cma-proven-reader-error", "errorCode":112, "errorLabels":["TransientTransactionError"]
        }}).await.unwrap();
        let error = if snapshot {
            read_proven_agent_admitted(&store, &agent.id, &mut session, &policy, plan())
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap_err()
        } else {
            legacy_proof_for_context_admitted(
                &store,
                &agent,
                &raw,
                &mut session,
                false,
                &policy,
                plan(),
            )
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err()
        };
        admin
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        let ReadFailure::Driver(driver) = &*error else {
            panic!("original driver error required")
        };
        assert!(driver.contains_label("TransientTransactionError"));
        let pointer = std::ptr::from_ref(driver.kind.as_ref());
        let moved = error.map(|e| match e {
            ReadFailure::Driver(e) => e,
            _ => unreachable!(),
        });
        assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
        assert!(moved.belongs_to(&policy));
        drop(moved);
        assert_eq!(root.available_bytes(), idle);
        let _ = session.abort_transaction().await;
    }
    drop(store);
    reader.shutdown().await;
    client.database(&name).drop().await.unwrap();
}
