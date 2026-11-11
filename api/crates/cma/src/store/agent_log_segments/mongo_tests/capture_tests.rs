use super::*;
use segments::capture::{capture, points, CaptureMode, CapturedFormat};

pub(super) async fn legacy(
    store: &MongoSandboxStore,
    kinds: Vec<AgentLogKind>,
) -> (Agent, Vec<AgentLogEntry>) {
    let mut agent = agent();
    let mut events = vec![AgentLogKind::Created {
        workspace_id: agent.workspace_id.clone(),
        worktree_id: agent.worktree_id.clone(),
    }];
    events.extend(kinds);
    let entries = events
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect::<Vec<_>>();
    agent.conversation.last_seq = entries.len() as u64;
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    store
        .agent_log
        .insert_many(entries.iter().map(|e| log_document(e).unwrap()))
        .await
        .unwrap();
    (agent, entries)
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn legacy_capture_folds_once_and_points_retain_original_snapshot() {
    let (client, name) = database("capture_snapshot").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = legacy(
        &store,
        vec![AgentLogKind::ModeChanged {
            mode: "code".into(),
        }],
    )
    .await;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .unwrap();
    assert_eq!(captured.agent(), &agent);
    assert_eq!(captured.format(), CapturedFormat::Legacy);
    assert_eq!(captured.through(), 2);
    assert_eq!(captured.incarnation(), digest::incarnation(&agent));
    assert_eq!(captured.latest_entry(), entries.last());
    assert_eq!(captured.projection().through_seq, 2);
    assert_eq!(captured.max_opened_generation(), 0);
    assert_eq!(captured.max_retired_generation(), 0);
    assert_eq!(captured.active_head().active_ordinal, 0);
    assert_eq!(captured.active_head().active_count, 2);
    let raw = store
        .agent_log
        .clone_with_type::<RawDocumentBuf>()
        .find(doc! {})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(
        captured.active_head().active_bytes,
        raw.iter().map(|r| r.as_bytes().len() as i64).sum::<i64>()
    );
    let expected_hashes = raw
        .iter()
        .enumerate()
        .flat_map(|(i, r)| {
            digest::leaf(
                agent.id.as_str(),
                captured.incarnation(),
                i as i64 + 1,
                r.as_bytes(),
            )
            .bytes
        })
        .collect::<Vec<_>>();
    assert_eq!(captured.active_leaf_digests().bytes, expected_hashes);
    store
        .agent_log
        .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":2_i64})
        .await
        .unwrap();
    let observed = crate::store::mongo_read_telemetry::isolated_test(async {
        assert_eq!(
            points(
                &store,
                &captured,
                &[1, 2],
                &mut session,
                CaptureMode::Snapshot
            )
            .await
            .unwrap(),
            entries
        );
        crate::store::mongo_read_telemetry::snapshot("agent_log")
    })
    .await;
    assert_eq!(&observed[..3], &[1, 0, 2]);
    let mut fresh = client.start_session().snapshot(true).await.unwrap();
    assert!(capture(&store, &agent, &mut fresh, CaptureMode::Snapshot)
        .await
        .is_err());
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert!(!client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|s| COLLECTIONS.contains(&s.as_str())));
    drop(session);
    drop(fresh);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn captured_points_reject_wrong_context_and_invalid_requests_before_source_io() {
    let (client, name) = database("capture_context").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = legacy(&store, vec![]).await;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .unwrap();
    let mut other = client.start_session().snapshot(true).await.unwrap();
    crate::store::mongo_read_telemetry::isolated_test(async {
        assert!(
            points(&store, &captured, &[1], &mut other, CaptureMode::Snapshot)
                .await
                .is_err()
        );
        assert!(points(
            &store,
            &captured,
            &[1],
            &mut session,
            CaptureMode::Transaction
        )
        .await
        .is_err());
        for seqs in [
            vec![],
            vec![0],
            vec![2],
            vec![1, 1],
            vec![2, 1],
            vec![1; 33],
        ] {
            assert!(points(
                &store,
                &captured,
                &seqs,
                &mut session,
                CaptureMode::Snapshot
            )
            .await
            .is_err());
        }
        assert_eq!(
            crate::store::mongo_read_telemetry::snapshot("agent_log"),
            [0; 4]
        );
    })
    .await;
    assert!(
        capture(&store, &agent, &mut session, CaptureMode::Transaction)
            .await
            .is_err()
    );
    let mut changed = agent.clone();
    changed.revision += 1;
    assert!(
        capture(&store, &changed, &mut session, CaptureMode::Snapshot)
            .await
            .is_err()
    );
    let mut ordinary = client.start_session().await.unwrap();
    assert!(
        capture(&store, &agent, &mut ordinary, CaptureMode::Snapshot)
            .await
            .is_err()
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    drop(session);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn capture_refuses_quarantine_and_invalid_semantics_without_writes() {
    let (client, name) = database("capture_corruption").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = legacy(
        &store,
        vec![AgentLogKind::ModeChanged {
            mode: "code".into(),
        }],
    )
    .await;
    let original = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":{"agent_log_integrity_quarantine_v1":{"format":1_i64}}},
        )
        .await
        .unwrap();
    let before = store.agents.find_one(doc! {}).await.unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .is_err());
    assert_eq!(store.agents.find_one(doc! {}).await.unwrap(), before);
    drop(session);
    store
        .agents
        .replace_one(doc! {"_id":agent.id.as_str()}, original)
        .await
        .unwrap();
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":1_i64},
            doc! {"$set":{"record.kind.workspace_id":"wks_other_fixture"}},
        )
        .await
        .unwrap();
    let before = store
        .agent_log
        .find(doc! {})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .is_err());
    assert_eq!(
        store
            .agent_log
            .find(doc! {})
            .sort(doc! {"seq":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        before
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    drop(session);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn captured_point_byte_limit_refuses_oversize_without_invalidating_capture() {
    let (client, name) = database("capture_budget").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = legacy(
        &store,
        vec![AgentLogKind::ModeChanged {
            mode: "x".repeat(4 * 1024 * 1024),
        }],
    )
    .await;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .unwrap();
    assert!(matches!(
        points(&store, &captured, &[2], &mut session, CaptureMode::Snapshot).await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Unavailable(_)
            )
        )
    ));
    assert_eq!(
        points(&store, &captured, &[1], &mut session, CaptureMode::Snapshot)
            .await
            .unwrap(),
        entries[..1]
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    drop(session);
    client.database(&name).drop().await.unwrap();
}
