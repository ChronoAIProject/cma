use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn missing_foreign_stale_and_unsupported_projection_refuses_without_repair_or_fallback() {
    for fault in [
        "missing",
        "foreign",
        "stale",
        "unsupported",
        "active-source",
    ] {
        let (_, db, store, agent, entries) = fixture(2, false).await;
        let filter = doc! {"_id":agent.id.as_str()};
        match fault {
            "missing" => {
                store
                    .segment_logs
                    .projections
                    .delete_one(filter.clone())
                    .await
                    .unwrap();
            }
            "active-source" => {
                let result = store
                    .agent_log
                    .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":agent.conversation.last_seq as i64})
                    .await
                    .unwrap();
                assert_eq!(result.deleted_count, 1);
            }
            _ => {
                let field = match fault {
                    "foreign" => "incarnation",
                    "stale" => "through_seq",
                    _ => "format_version",
                };
                let value = if fault == "foreign" {
                    mongodb::bson::Bson::String("foreign".into())
                } else {
                    0_i64.into()
                };
                store
                    .segment_logs
                    .projections
                    .update_one(filter.clone(), doc! {"$set":{field:value}})
                    .bypass_document_validation(true)
                    .await
                    .unwrap();
            }
        }
        let before = store
            .segment_logs
            .projections
            .find_one(filter.clone())
            .await
            .unwrap();
        assert!(
            store.agent_log_transcript_projection(&agent).await.is_err(),
            "{fault}"
        );
        assert!(
            store
                .agent_log_transcript_page(&agent, &first(4))
                .await
                .is_err(),
            "{fault}"
        );
        assert!(
            store.agent_log_usage_projection(&agent).await.is_err(),
            "{fault}"
        );
        let (after, limit) = if fault == "active-source" {
            // An intact sealed page has its own proof. Probe the active row
            // directly to verify that an intersecting page fails closed.
            let sealed = store
                .agent_log_page_projection(&agent, 0, 20)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(sealed, entries[..20]);
            (agent.conversation.last_seq - 1, 1)
        } else {
            (0, 20)
        };
        assert!(
            store
                .agent_log_page_projection(&agent, after, limit)
                .await
                .is_err(),
            "{fault}"
        );
        let mut session = store.client.start_session().snapshot(true).await.unwrap();
        assert!(
            crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut session)
                .await
                .is_err(),
            "{fault}"
        );
        assert_eq!(
            store
                .segment_logs
                .projections
                .find_one(filter)
                .await
                .unwrap(),
            before
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn corrupt_turn_and_original_refuse_but_existing_snapshot_remains_consistent() {
    for fault in ["turn", "source"] {
        let (_, db, store, agent, entries) = fixture(2, false).await;
        let mut old = store.client.start_session().snapshot(true).await.unwrap();
        let original = page(&store, &agent, &mut old, &first(4)).await.unwrap();
        match fault {
            "turn" => {
                store
                    .segment_logs
                    .evidence
                    .update_one(
                        doc! {"kind":"turn"},
                        doc! {"$set":{"record.facts.owner_sequence":1_i64}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                store
                    .agent_log
                    .update_one(
                        doc! {"agent_id":agent.id.as_str(),"seq":4_i64},
                        doc! {"$set":{"record.kind.content":"altered"}},
                    )
                    .await
                    .unwrap();
            }
        }
        assert_eq!(
            page(&store, &agent, &mut old, &first(4)).await.unwrap(),
            original
        );
        assert!(
            store
                .agent_log_transcript_page(&agent, &first(4))
                .await
                .is_err(),
            "{fault}"
        );
        let mut fresh = store.client.start_session().snapshot(true).await.unwrap();
        assert!(
            crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut fresh)
                .await
                .is_err(),
            "{fault}"
        );
        assert_eq!(
            original.unwrap().transcript.turns[0].owner_message,
            expected(&agent, &entries).turns[0].owner_message
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn transcript_page_rejects_invalid_limits_cursors_and_changed_high_water() {
    let (_, db, store, agent, _) = fixture(2, false).await;
    for limit in [0, 33, u16::MAX] {
        assert!(matches!(
            store.agent_log_transcript_page(&agent, &first(limit)).await,
            Err(SandboxStoreError::InvalidPage)
        ));
    }
    for (turn, after, through) in [
        (3, 2, 1),
        (3, 1, agent.conversation.last_seq),
        (99, 98, agent.conversation.last_seq),
        (3, agent.conversation.last_seq, agent.conversation.last_seq),
    ] {
        let request = layer2_core::sandbox::AgentTranscriptPageRequest {
            turn: Some(turn),
            after: Some(after),
            through: Some(through),
            limit: 4,
        };
        assert!(store
            .agent_log_transcript_page(&agent, &request)
            .await
            .is_err());
    }
    db.drop().await.unwrap();
}
