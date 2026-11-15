use super::contracts::proposal;
use super::*;
use crate::store::agent_log_segments::{schema, writer_gate};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_segmented_publication_keeps_head_and_exact_receipt() {
    let mut f = Fixture::new(500).await;
    let db = f.client.database(&f.name);
    schema::migrate(&db).await.unwrap();
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    writer_gate::replace(
        &db,
        writer_gate::GateName::Segments,
        0,
        writer_gate::Policy {
            enabled: true,
            agents: vec![f.agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    f.store
        .append_agent_log_keyed(
            &f.agent.id,
            &AgentLogPublicationIdentity {
                key: "retry-adoption".into(),
                fingerprint: [7; 32],
                appended_at: MINUTE.into(),
            },
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
    f.agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    f.store.commit_agent_work(retry(&f.agent)).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, TWO_MINUTES).await;
    assert_eq!(f.row().await.get_i64("document_schema_version").unwrap(), 2);
    let marker = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    let retry = proposal(&f.agent, &marker, TWO_MINUTES, "2026-09-05T00:03:00Z");
    f.store
        .commit_agent_work_repeated_failure(retry.clone(), None)
        .await
        .unwrap();
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(retry.mutation.agent.clone())
    );
    assert_eq!(
        f.store
            .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
            .await
            .unwrap(),
        Some(marker)
    );
    assert!(f
        .store
        .commit_agent_work_repeated_failure(retry, None)
        .await
        .is_err());
    f.cleanup().await;
}
