#[path = "ownership_retry_desktop_tests.rs"]
mod desktop;

use super::ownership_retry_wake_tests::fixture;
use super::*;
use crate::sandbox::ownership_retry_hint::{
    AdvisoryDecision, HintWriteResult, OwnershipRetryHints,
};
use crate::store::agent_failure_pacing::{discovery, ownership::FIELD as HINT};
use futures_util::TryStreamExt;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_discovery_filters_dense_pages_and_matches_claim_predicate() {
    let (client, name, store, agent, _, observed) = fixture("retry_dense").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let mut rows = Vec::new();
    for index in 0..105 {
        let mut held = agent.clone();
        held.id = AgentId(format!("a_held_{index:03}"));
        held.creation_key = format!("held-{index}");
        held.worktree_id = WorktreeId(format!("held-tree-{index}"));
        held.work.lease = None;
        let mut row = crate::store::agents::agent_document(&held).unwrap();
        let mut hint = observed.document();
        hint.get_document_mut("key")
            .unwrap()
            .insert("worktree", held.worktree_id.as_str());
        row.insert(HINT, hint);
        if index % 2 == 0 {
            row.insert(FIELD, doc! {"class": CLASS, "agent_revision": held.revision as i64,
                "command_admission_generation": 0_i64, "attempts": held.work.attempts,
                "repeats": 0, "sandbox_revision": 0_i64, "workspace_revision": 0_i64, "worktree_revision": 0_i64,
                "ready_at": &held.work.ready_at, "until": layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(120))});
        }
        rows.push(row);
    }
    store.agents.insert_many(rows).await.unwrap();
    let mut runnable = agent.clone();
    runnable.id = AgentId("z_runnable".into());
    runnable.creation_key = "runnable".into();
    runnable.worktree_id = WorktreeId("runnable-tree".into());
    runnable.work.lease = None;
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&runnable).unwrap())
        .await
        .unwrap();
    for pipeline in [
        discovery::pipeline(&store, &now, 1, &Default::default()),
        discovery::owned_pipeline(&store, &now, 1, None, &Default::default()),
    ] {
        let rows: Vec<Document> = store
            .agents
            .aggregate(pipeline)
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_str("_id").unwrap(), runnable.id.as_str());
    }
    let plan = client.database(&name).run_command(doc! {"explain": {
        "aggregate": store.agents.name(), "pipeline": discovery::owned_pipeline(&store, &now, 1, None, &Default::default()),
        "cursor": {}, "maxTimeMS": 2000_i64,
    }, "verbosity": "executionStats"}).await.unwrap();
    let mut examined = Vec::new();
    explain_examined(&Bson::Document(plan.clone()), &mut examined);
    assert!(!examined.is_empty());
    assert!(
        examined.iter().all(|count| *count <= 106 * 8),
        "dense query work exceeded the fixture bound: {examined:?}"
    );
    let mut indexes = std::collections::BTreeSet::new();
    explain_indexes(&Bson::Document(plan), &mut indexes);
    assert!(
        indexes.contains("_id_"),
        "parent lookup must retain its indexed identity probe: {indexes:?}"
    );
    assert!(
        indexes.len() > 1,
        "Agent work discovery must use an existing work index: {indexes:?}"
    );
    println!("OWNERSHIP_RETRY_HINT dense_candidates=106 page_limit=1 indexes={indexes:?} examined={examined:?}");
    assert_eq!(
        store.preflight(&AgentId("a_held_000".into())).await,
        AdvisoryDecision::Hold
    );
    let row = store
        .agents
        .find_one(doc! {"_id": "a_held_000"})
        .await
        .unwrap()
        .unwrap();
    for value in [
        Bson::String("malformed".into()),
        Bson::Document(doc! {"until": "tomorrow"}),
        Bson::Array(vec![]),
        Bson::Null,
    ] {
        store
            .agents
            .update_one(doc! {"_id": "a_held_000"}, doc! {"$set": {HINT: value}})
            .await
            .unwrap();
        assert_eq!(
            store.preflight(&AgentId("a_held_000".into())).await,
            AdvisoryDecision::Proceed
        );
        let rows: Vec<Document> = store
            .agents
            .aggregate(discovery::pipeline(&store, &now, 1, &Default::default()))
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(rows[0].get_str("_id").unwrap(), "a_held_000");
    }
    store
        .agents
        .replace_one(doc! {"_id": "a_held_000"}, row)
        .await
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id": "a_held_000"},
            doc! {"$inc": {"agent_command_admission_fence": 1_i64}},
        )
        .await
        .unwrap();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = requests.clone();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if matches!(event, mongodb::event::command::CommandEvent::Started(_)) {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let measured_client = Client::with_options(options).unwrap();
    let measured = MongoSandboxStore::open(&measured_client, &name)
        .await
        .unwrap()
        .enable_agent_ownership("manager".into())
        .await
        .unwrap()
        .with_ownership_retry_hints(true);
    requests.store(0, Ordering::SeqCst);
    assert_eq!(
        measured.due_agent_work(&now, 1).await.unwrap(),
        vec![AgentId("a_held_000".into())]
    );
    let requests = requests.load(Ordering::SeqCst);
    assert!(
        requests <= 16,
        "dense discovery driver request bound exceeded: {requests}"
    );
    println!("OWNERSHIP_RETRY_HINT driver_requests={requests}");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_discovery_matches_retained_parent_envelope_readers() {
    let (client, name, store, agent, _, observed) = fixture("retry_parent_formats").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    let original = store
        .sandboxes
        .find_one(doc! {"_id": agent.sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let revision = original.get_i64("revision").unwrap();
    let now = layer2_core::rfc3339_at(SystemTime::now());
    for (label, fields, valid) in [
        ("legacy", doc! {"document_schema_version": 1_i32}, true),
        (
            "renewed",
            doc! {"document_schema_version": 3_i64,
            "record.work.authority_protocol": "renewed_v1"},
            true,
        ),
        (
            "prepared",
            doc! {"document_schema_version": 4_i64,
            "record.work.prepared_installation_digest": "a".repeat(64)},
            true,
        ),
        ("float_schema", doc! {"document_schema_version": 1.0}, false),
        ("float_revision", doc! {"revision": revision as f64}, false),
        ("int32_revision", doc! {"revision": revision as i32}, false),
        (
            "float_record_revision",
            doc! {"record.revision": revision as f64},
            false,
        ),
        (
            "missing_renewed_protocol",
            doc! {"document_schema_version": 3_i64},
            false,
        ),
        (
            "unknown_protocol",
            doc! {"record.work.authority_protocol": "future"},
            false,
        ),
        (
            "missing_prepared_proof",
            doc! {"document_schema_version": 4_i64},
            false,
        ),
        (
            "invalid_prepared_proof",
            doc! {"document_schema_version": 4_i64,
            "record.work.prepared_installation_digest": "invalid"},
            false,
        ),
    ] {
        store
            .sandboxes
            .replace_one(doc! {"_id": agent.sandbox_id.as_str()}, original.clone())
            .await
            .unwrap();
        store
            .sandboxes
            .update_one(
                doc! {"_id": agent.sandbox_id.as_str()},
                doc! {"$set": fields},
            )
            .await
            .unwrap();
        assert_eq!(
            store.get_sandbox(&agent.sandbox_id).await.is_ok(),
            valid,
            "{label}"
        );
        assert_eq!(
            store.preflight(&agent.id).await,
            if valid {
                AdvisoryDecision::Hold
            } else {
                AdvisoryDecision::Unavailable
            },
            "preflight {label}"
        );
        let rows: Vec<Document> = store
            .agents
            .aggregate(discovery::owned_pipeline(
                &store,
                &now,
                1,
                None,
                &Default::default(),
            ))
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(rows.is_empty(), valid, "discovery {label}");
    }
    client.database(&name).drop().await.unwrap();
}

fn explain_examined(value: &Bson, counts: &mut Vec<i64>) {
    match value {
        Bson::Document(document) => {
            if let Some(count) = document
                .get("totalDocsExamined")
                .and_then(|value| match value {
                    Bson::Int32(value) => Some(i64::from(*value)),
                    Bson::Int64(value) => Some(*value),
                    _ => None,
                })
            {
                counts.push(count);
            }
            for value in document.values() {
                explain_examined(value, counts);
            }
        }
        Bson::Array(values) => {
            for value in values {
                explain_examined(value, counts);
            }
        }
        _ => (),
    }
}

fn explain_indexes(value: &Bson, indexes: &mut std::collections::BTreeSet<String>) {
    match value {
        Bson::Document(document) => {
            if let Ok(name) = document.get_str("indexName") {
                indexes.insert(name.into());
            }
            if let Ok(names) = document.get_array("indexesUsed") {
                indexes.extend(names.iter().filter_map(Bson::as_str).map(str::to_owned));
            }
            for value in document.values() {
                explain_indexes(value, indexes);
            }
        }
        Bson::Array(values) => {
            for value in values {
                explain_indexes(value, indexes);
            }
        }
        _ => (),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_final_claim_rechecks_committed_hint_after_stale_preflight() {
    use crate::store::child_writer_claims::{persist, ChildWriterClaim};
    for hint_first in [true, false] {
        let (client, name, store, agent, authority, observed) = fixture("retry_claim").await;
        assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Proceed);
        let mut next = agent.clone();
        next.work.lease = None;
        let now = layer2_core::rfc3339_at(SystemTime::now());
        next.work
            .claim(
                "next".into(),
                &now,
                layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(600)),
            )
            .unwrap();
        next.revision += 1;
        if hint_first {
            assert_eq!(observed.write().await, HintWriteResult::Recorded);
        }
        let claimed = authority
            .run(persist(
                &store,
                agent.revision,
                ChildWriterClaim::Agent(Box::new(next.clone())),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed, !hint_first);
        if !hint_first {
            assert_eq!(observed.write().await, HintWriteResult::Stale);
        }
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().unwrap(),
            if hint_first { agent } else { next }
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_query_and_transaction_validation_agree_on_malformed_fields() {
    use crate::store::agent_failure_pacing::ownership;
    use mongodb::bson::DateTime;
    let (client, name, mut store, agent, _, observed) = fixture("retry_malformed").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    let document = observed.document();
    let at = *document.get_datetime("observed_at").unwrap();
    store.ownership_retry_hint_now = Some(at);
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let original = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let mut cases = vec![("valid", document.clone(), true)];
    for (field, value) in [
        ("version", Bson::Int64(2)),
        ("version", Bson::Double(1.0)),
        ("reason", Bson::String("unknown".into())),
        (
            "observed_at",
            Bson::DateTime(DateTime::from_millis(i64::MIN)),
        ),
        ("until", Bson::DateTime(DateTime::from_millis(i64::MAX))),
        ("until", Bson::DateTime(at)),
        ("until", Bson::String("tomorrow".into())),
        ("observation", Bson::String("not-a-uuid".into())),
        ("claimed_revision", Bson::Int64(0)),
        ("owner_epoch", Bson::Double(1.0)),
        ("owner_protocol", Bson::Int64(2)),
        ("receiver_lease_ms", Bson::Int64(10001)),
        ("retry_classifier_version", Bson::Int64(2)),
        ("future_field", Bson::Boolean(true)),
    ] {
        let mut malformed = document.clone();
        malformed.insert(field, value);
        cases.push((field, malformed, false));
    }
    for (field, value) in [
        ("owner", Bson::String(String::new())),
        ("runtime_id", Bson::String("x".repeat(4097))),
        ("generation", Bson::Int64(-1)),
        ("attempts", Bson::Int64(i64::MAX)),
        ("high_water", Bson::Double(1.0)),
        ("command_generation", Bson::Double(0.0)),
        ("boot_id", Bson::Array(vec![])),
        ("provision_generation", Bson::Int64(0)),
        ("future_field", Bson::Null),
    ] {
        let mut malformed = document.clone();
        malformed
            .get_document_mut("key")
            .unwrap()
            .insert(field, value);
        cases.push((field, malformed, false));
    }
    let mut future = document.clone();
    future.insert(
        "observed_at",
        DateTime::from_millis(at.timestamp_millis() + 1),
    );
    future.insert(
        "until",
        DateTime::from_millis(at.timestamp_millis() + 60_001),
    );
    cases.push(("future", future, false));
    for (field, hint, held) in cases {
        let mut row = original.clone();
        row.insert(HINT, hint);
        assert_eq!(
            ownership::holds(&row, &agent, &parent, at),
            held,
            "transaction {field}"
        );
        store
            .agents
            .replace_one(doc! {"_id": agent.id.as_str()}, row)
            .await
            .unwrap();
        assert_eq!(
            store.preflight(&agent.id).await,
            if held {
                AdvisoryDecision::Hold
            } else {
                AdvisoryDecision::Proceed
            },
            "preflight {field}"
        );
        let ids: Vec<Document> = store
            .agents
            .aggregate(discovery::owned_pipeline(
                &store,
                &now,
                1,
                None,
                &Default::default(),
            ))
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(ids.is_empty(), held, "discovery {field}");
    }
    // A numeric BSON equality must not treat a malformed admission generation
    // as the integer that the transactional decoder requires.
    for value in [Bson::Double(0.0), Bson::Int32(0), Bson::Null] {
        let mut row = original.clone();
        row.insert("agent_command_admission_fence", value);
        assert!(!ownership::holds(&row, &agent, &parent, at));
        store
            .agents
            .replace_one(doc! {"_id": agent.id.as_str()}, row)
            .await
            .unwrap();
        assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Proceed);
        assert!(!store
            .agents
            .aggregate(discovery::owned_pipeline(
                &store,
                &now,
                1,
                None,
                &Default::default()
            ))
            .await
            .unwrap()
            .try_collect::<Vec<Document>>()
            .await
            .unwrap()
            .is_empty());
    }
    client.database(&name).drop().await.unwrap();
}
