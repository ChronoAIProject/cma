use super::*;
use crate::store::agent_log_segments::{self as segments, publication, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

#[tokio::test]
#[ignore = "requires the disposable replica-set runner"]
async fn open_cursor_survives_real_adoption_without_resetting_history() {
    let f = Fixture::semantic(401).await;
    segments::schema::migrate(&f.client.database(&f.name))
        .await
        .unwrap();
    let reads = f.reads(false);
    let authorization = reads
        .authorize_event_stream(&context(f.agent.owner.clone()), None, &f.agent.id)
        .await
        .unwrap();
    let mut cursor = AgentHistoryCursor::default();
    let first = reads
        .event_stream_page(&authorization, &mut cursor, None)
        .await
        .unwrap();
    assert_eq!(first.history.last().unwrap().seq, 200);
    let original = f.store.agent_log_prefix(&f.agent.id, 401).await.unwrap();
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &f.client.database(&f.name),
        gate::GateName::Segments,
        0,
        gate::Policy {
            enabled: true,
            agents: vec![f.agent.id.to_string()],
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
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let identity = AgentLogPublicationIdentity {
        key: "cursor-adoption".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    };
    f.store
        .append_agent_log_keyed(&f.agent.id, &identity, kind)
        .await
        .unwrap();
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert_eq!(
        f.store.agent_log_prefix(&f.agent.id, 401).await.unwrap(),
        original
    );
    let mut seen = Vec::new();
    while cursor.sequence() < 402 {
        let page = reads
            .event_stream_page(&authorization, &mut cursor, None)
            .await
            .unwrap();
        seen.extend(page.history.into_iter().map(|entry| entry.seq));
    }
    assert_eq!(seen, (201..=402).collect::<Vec<_>>());
    assert!(reads
        .event_stream_page(&authorization, &mut cursor, None)
        .await
        .unwrap()
        .history
        .is_empty());
    f.store
        .agents
        .update_one(
            doc! {"_id":f.agent.id.as_str()},
            doc! {"$set":{"agent_log_incarnation":"foreign-incarnation"}},
        )
        .await
        .unwrap();
    assert!(reads
        .event_stream_page(&authorization, &mut cursor, None)
        .await
        .is_err());
    assert_eq!(cursor.sequence(), 402);
    f.client.database(&f.name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable replica-set runner"]
async fn cursor_crosses_tiny_segment_boundaries_and_refuses_changed_original() {
    use crate::store::agent_log_segments::mongo_tests::fixtures;
    let f = Fixture::semantic(410).await;
    segments::schema::migrate(&f.client.database(&f.name))
        .await
        .unwrap();
    let entries = f.store.agent_log_prefix(&f.agent.id, 410).await.unwrap();
    f.store
        .agents
        .delete_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap();
    f.store
        .agent_log
        .delete_many(doc! {"agent_id":f.agent.id.as_str()})
        .await
        .unwrap();
    let (agent, entries) = fixtures::install(&f.store, f.agent.clone(), entries, 200, 2).await;
    fixtures::certify(&f.store, &agent, &entries).await;
    let reads = f.reads(false);
    let authorization = reads
        .authorize_event_stream(&context(agent.owner.clone()), None, &agent.id)
        .await
        .unwrap();
    let mut cursor = AgentHistoryCursor::default();
    let mut actual = Vec::new();
    while cursor.sequence() < 410 {
        let page = reads
            .event_stream_page(&authorization, &mut cursor, None)
            .await
            .unwrap();
        actual.extend(page.history.into_iter().map(|entry| entry.seq));
    }
    assert_eq!(actual, (1..=410).collect::<Vec<_>>());
    let mut retry = AgentHistoryCursor::default();
    reads
        .event_stream_page(&authorization, &mut retry, None)
        .await
        .unwrap();
    f.store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":201_i64},
            doc! {"$set":{"record.appended_at":"2026-09-17T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert!(reads
        .event_stream_page(&authorization, &mut retry, None)
        .await
        .is_err());
    assert_eq!(retry.sequence(), 200);
    f.client.database(&f.name).drop().await.unwrap();
}
