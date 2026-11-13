use super::*;
use crate::store::agent_log_segments::{
    mongo_tests::fixtures, publication::PublicationIdentity, writer_admission,
};

#[path = "bounds/measurement.rs"]
mod measurement;

async fn history(
    suffix: &str,
    closed: i64,
    active: i64,
) -> (
    Client,
    String,
    MongoSandboxStore,
    AgentAdmission,
    layer2_core::sandbox::Agent,
) {
    let (client, database, store, _, _, original) = fixture(suffix).await;
    let created = store
        .admit_agent(original.clone())
        .await
        .unwrap()
        .creation()
        .clone();
    schema::migrate(&client.database(&database)).await.unwrap();
    // Install a controlled segmented fixture while retaining the original
    // creation event and its already-created parent/worktree/provenance.
    store
        .agents
        .delete_one(doc! {"_id":created.agent.id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id":created.agent.id.as_str()})
        .await
        .unwrap();
    let mut entries = vec![created.initial_log.clone()];
    entries.extend((2..=closed * 2 + active).map(|seq| AgentLogEntry {
        agent_id: created.agent.id.clone(),
        seq: seq as u64,
        appended_at: NOW.into(),
        kind: AgentLogKind::ModeChanged {
            mode: "code".into(),
        },
    }));
    let (agent, entries) = fixtures::install(&store, created.agent, entries, closed, 2).await;
    fixtures::certify(&store, &agent, &entries).await;
    fixtures::certify_output(&store, &agent, &entries).await;
    let mut request = original;
    request.agent_id = AgentId::generate();
    request.worktree_id = WorktreeId::generate();
    request.creation_key = format!("trigger-reuse-{suffix}");
    request.origin = AgentOrigin::Trigger {
        trigger_id: format!("trg_{suffix}"),
        launch_id: format!("lch_{suffix}"),
    };
    request.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            request.creation_key.clone(),
            "Original prompt",
            NOW,
        )
        .unwrap(),
    );
    (client, database, store, request, agent)
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_reuse_source_work_stays_bounded_across_a_thousand_segments() {
    let mut baseline = None;
    for closed in [1, 10, 100, 1000] {
        let (client, database, _, request, agent) =
            history(&format!("reuse_scale_{closed}"), closed, 2).await;
        let (store, trace) = measurement::observed(&database).await;
        let started = std::time::Instant::now();
        let measured = crate::store::mongo_read_telemetry::isolated_test(async {
            let reused = store.admit_agent_reusing_profile(request).await.unwrap();
            assert_eq!(reused.creation().agent.id, agent.id);
            assert_eq!(
                reused.creation().agent.conversation.last_seq,
                agent.conversation.last_seq + 1
            );
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        let elapsed = started.elapsed();
        let trace = trace.lock().unwrap().clone();
        eprintln!(
            "reuse segments={closed} source={measured:?} wire={trace:?} sample_us={}",
            elapsed.as_micros()
        );
        assert_eq!(trace.source_rows as u64, measured[2]);
        assert_eq!(trace.source_bytes as u64, measured[3]);
        assert!(trace.commands <= 60 && trace.max_source_batch <= 64);
        assert!(trace.max_reply_bytes < 1024 * 1024);
        // With one closed segment the creation seal directly precedes the head.
        // Longer histories require exactly one indexed successor-seal lookup.
        let work = (trace.commands - usize::from(closed > 1), trace.source_rows);
        if let Some(baseline) = baseline {
            assert_eq!(work, baseline);
        } else {
            baseline = Some(work);
        }
        assert_eq!(
            &measured[..3],
            &[2, 0, 3],
            "one active scan and creation point"
        );
        assert!(measured[3] < 32 * 1024, "source BSON: {measured:?}");
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_reuse_rolls_only_with_exact_admission_after_gate_closure() {
    let (client, database, store, request, agent) = history("reuse_rollover", 1, 10_000).await;
    let before = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .admit_agent_reusing_profile(request.clone())
        .await
        .is_err());
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_reuse_bindings
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    gate(&store, &agent.id, 0, true).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let operation = PublicationIdentity {
        key: format!(
            "reuse:{}",
            crate::store::agent_admission_records::binding_id(&request).unwrap()
        ),
        fingerprint: publication::request_digest(&request.fingerprint().unwrap()).unwrap(),
        appended_at: request.event_at.clone(),
    };
    if let Err(error) = writer_admission::admit(
        &client.database(&database),
        &agent,
        &operation,
        writer_admission::AdmissionKind::Rollover,
        NOW,
        &mut session,
    )
    .await
    {
        panic!("admission: {error:?}");
    }
    session.commit_transaction().await.unwrap();
    gate(&store, &agent.id, 1, false).await;
    let result = store
        .admit_agent_reusing_profile(request.clone())
        .await
        .unwrap();
    assert_eq!(
        result.creation().agent.conversation.last_seq,
        agent.conversation.last_seq + 1
    );
    assert_eq!(
        store.admit_agent_reusing_profile(request).await.unwrap(),
        result
    );
    let db = client.database(&database);
    assert_eq!(
        db.collection::<Document>("agent_log_segments")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let head = db
        .collection::<Document>("agent_log_heads")
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.get_i64("active_count").unwrap(), 1);
    db.drop().await.unwrap();
}
