use super::*;
use layer2_core::sandbox::{
    store::AgentStore, AgentTitle, AgentTitleClaimRequest, AgentTitleSource, AgentTitleStore,
    SetAgentTitleOutcome,
};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_title_claim_and_publication_remain_bounded_and_fence_automatic_work() {
    for closed in [1, 10, 100, 1000] {
        let (_, db, store, agent, entries) = fixture(closed, false).await;
        segments::mongo_tests::fixtures::certify_output(&store, &agent, &entries).await;
        let request = AgentTitleClaimRequest::new(agent.id.clone(), "generator");
        let measured = crate::store::mongo_read_telemetry::isolated_test(async {
            let claim = store
                .acquire_agent_title_claim(&request, std::time::SystemTime::now())
                .await
                .unwrap()
                .unwrap();
            let title = AgentTitle::new("Chosen title").unwrap();
            assert_eq!(
                store
                    .set_user_agent_title(&agent.id, &title, NOW.into())
                    .await
                    .unwrap(),
                SetAgentTitleOutcome::Set
            );
            assert!(!store
                .complete_agent_title(
                    &claim,
                    &AgentTitle::new("Late automatic title").unwrap(),
                    AgentTitleSource::Generated,
                    NOW.into()
                )
                .await
                .unwrap());
            assert_eq!(
                store
                    .set_user_agent_title(&agent.id, &title, NOW.into())
                    .await
                    .unwrap(),
                SetAgentTitleOutcome::AlreadySet
            );
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert!(measured[2] <= 90, "bounded originals: {measured:?}");
        assert!(measured[3] < 128 * 1024, "bounded BSON: {measured:?}");
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(
            current.conversation.last_seq,
            agent.conversation.last_seq + 1
        );
        assert_eq!(current.title.as_deref(), Some("Chosen title"));
        assert_eq!(
            store
                .agent_log_entry(&agent.id, current.conversation.last_seq)
                .await
                .unwrap()
                .unwrap()
                .kind,
            AgentLogKind::TitleSet {
                title: "Chosen title".into(),
                source: "user".into()
            }
        );
        let row = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
        assert!(!row.contains_key("agent_log_integrity_v1"));
        assert_eq!(
            store
                .segment_logs
                .publications
                .count_documents(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap(),
            1
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_automatic_title_completes_exact_claim_and_refuses_corrupt_source() {
    for corrupt in [false, true] {
        let (_, db, store, agent, entries) = fixture(3, false).await;
        segments::mongo_tests::fixtures::certify_output(&store, &agent, &entries).await;
        let claim = store
            .acquire_agent_title_claim(
                &AgentTitleClaimRequest::new(agent.id.clone(), "generator"),
                std::time::SystemTime::now(),
            )
            .await
            .unwrap()
            .unwrap();
        if corrupt {
            store
                .segment_logs
                .projections
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        let before = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap();
        let result = store
            .complete_agent_title(
                &claim,
                &AgentTitle::new("Generated title").unwrap(),
                AgentTitleSource::Generated,
                NOW.into(),
            )
            .await;
        if corrupt {
            assert!(result.is_err());
            assert_eq!(
                store
                    .agents
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap(),
                before
            );
            assert_eq!(
                store
                    .segment_logs
                    .publications
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
        } else {
            assert!(result.unwrap());
            assert!(store
                .complete_agent_title(
                    &claim,
                    &AgentTitle::new("Generated title").unwrap(),
                    AgentTitleSource::Generated,
                    NOW.into()
                )
                .await
                .unwrap());
            let titled = store.get_agent(&agent.id).await.unwrap().unwrap();
            let original = store
                .agent_log_entry(&agent.id, titled.conversation.last_seq)
                .await
                .unwrap()
                .unwrap();
            let receipt = segments::publication::Publication::read(
                store
                    .segment_logs
                    .publications
                    .find_one(doc! {"agent_id":agent.id.as_str()})
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            let later = AgentLogKind::ModeChanged {
                mode: "later".into(),
            };
            let identity = layer2_core::sandbox::store::AgentLogPublicationIdentity {
                key: "after-title".into(),
                fingerprint: segments::publication::request_digest(&later)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
                appended_at: NOW.into(),
            };
            store
                .append_agent_log_keyed(&agent.id, &identity, later)
                .await
                .unwrap();
            let operation = segments::publication::PublicationIdentity {
                key: receipt.publication_key.clone(),
                fingerprint: receipt.fingerprint.clone(),
                appended_at: receipt.appended_at.clone(),
            };
            let mut session = store.client.start_session().snapshot(true).await.unwrap();
            assert!(matches!(
                segments::writer_reconcile::reconcile(
                    &store,
                    &operation,
                    &titled,
                    &[original],
                    &receipt.result_digest,
                    &mut session,
                )
                .await
                .unwrap(),
                segments::writer_reconcile::Outcome::Recorded(actual) if *actual == receipt
            ));
        }
        db.drop().await.unwrap();
    }
}
