use super::*;
use crate::store::agent_log_segments::{self as segments, mongo_tests::fixtures};

async fn source() -> (
    mongodb::Database,
    MongoSandboxStore,
    Agent,
    Vec<AgentLogEntry>,
) {
    let (client, name) = fixtures::database("s3_suggest").await;
    let db = client.database(&name);
    segments::schema::migrate(&db).await.unwrap();
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.writer_policy = std::sync::Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode":"accounts","accounts":["fixture"],"capabilities":["suggested_followup_admission"]
        }))
        .unwrap(),
    );
    let mut agent = fixtures::agent();
    agent.conversation.conversation_id = Some("native".into());
    agent.conversation.runtime_generation = Some(1);
    let kinds = [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner".into(),
        },
        AgentLogKind::AgentMessage {
            content: "answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ];
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(n, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: n as u64 + 1,
            appended_at: "2026-11-01T00:00:00Z".into(),
            kind,
        })
        .collect();
    let (agent, entries) = fixtures::install(&store, agent, entries, 1, 4).await;
    (db, store, agent, entries)
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn suggestion_ordinary_read_claim_and_completion_never_bypass_refused_proof() {
    for snapshot in [true, false] {
        for empty in [true, false] {
            let (db, store, agent, _) = source().await;
            store
                .segment_logs
                .heads
                .update_one(
                    doc! {"_id":agent.id.as_str()},
                    doc! {"$inc":{"active_bytes":1_i64,"lifetime_bytes":1_i64}},
                )
                .await
                .unwrap();
            let before = store.agent_log.count_documents(doc! {}).await.unwrap();
            let mut session = store
                .client
                .start_session()
                .snapshot(snapshot)
                .await
                .unwrap();
            if !snapshot {
                start_transaction(&mut session).await.unwrap();
            }
            assert!(bounded_log(
                &store,
                &agent,
                if empty { 6 } else { 2 },
                6,
                &mut session,
                snapshot
            )
            .await
            .is_err());
            if !snapshot {
                session.abort_transaction().await.unwrap();
            }
            assert_eq!(
                store.agent_log.count_documents(doc! {}).await.unwrap(),
                before
            );
            assert_eq!(store.suggestions.count_documents(doc! {}).await.unwrap(), 0);
            db.drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn suggestion_enqueue_requires_exact_published_append_and_rejects_unpublished_or_foreign_batch(
) {
    let (db, store, agent, entries) = source().await;
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    // A valid published suffix is readable inside its transaction (no sandbox
    // means no outbox work). It does not grant a general proof bypass.
    enqueue(&store, &agent, &entries[5..], &mut session)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    for fault in ["unpublished", "foreign", "nonconsecutive", "changed-source"] {
        let mut batch = entries[3..].to_vec();
        match fault {
            "unpublished" => batch.last_mut().unwrap().seq += 1,
            "foreign" => batch[0].agent_id = AgentId("foreign".into()),
            "nonconsecutive" => batch[0].seq = 1,
            _ => {
                batch[0].kind = AgentLogKind::OwnerMessage {
                    content: "altered".into(),
                }
            }
        }
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        assert!(
            enqueue(&store, &agent, &batch, &mut session).await.is_err(),
            "{fault}"
        );
        session.abort_transaction().await.unwrap();
    }
    assert_eq!(store.suggestions.count_documents(doc! {}).await.unwrap(), 0);
    db.drop().await.unwrap();
}
