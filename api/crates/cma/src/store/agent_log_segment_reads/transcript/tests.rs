use super::*;
use layer2_core::sandbox::{store::AgentLogStore, AgentTitleTranscript};
mod answers;
mod parity;
mod refusal;
mod support;
use support::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn transcript_title_and_usage_keep_fixed_page_bounds_across_history_sizes() {
    for closed in [1, 10, 100, 1000] {
        let (_, db, store, agent, entries) = fixture(closed, false).await;
        if closed == 1000 {
            assert!(entries.len() > 10_128);
            assert!(serde_json::to_vec(&entries).unwrap().len() > 4 * 1024 * 1024);
        }
        db.run_command(doc! {"profile":2,"slowms":0}).await.unwrap();
        let measured = crate::store::mongo_read_telemetry::isolated_test(async {
            let mut session = store.client.start_session().snapshot(true).await.unwrap();
            let request = layer2_core::sandbox::AgentTranscriptPageRequest {
                turn: Some(3),
                after: Some((closed * 16 - 2) as u64),
                through: Some(agent.conversation.last_seq),
                limit: 4,
            };
            let result = page(&store, &agent, &mut session, &request)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.next.unwrap().after, Some((closed * 16 + 2) as u64));
            let title =
                crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut session)
                    .await
                    .unwrap();
            assert_eq!(
                title,
                AgentTitleTranscript::from_agent_log(&agent, &entries)
            );
            let usage = super::super::projected_usage(&store, &agent, &mut session)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(usage.usage.output_tokens, 8);
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert!(measured[2] <= 36, "bounded originals: {measured:?}");
        assert!(measured[3] < 32 * 1024, "bounded BSON: {measured:?}");
        db.run_command(doc! {"profile":0}).await.unwrap();
        use futures_util::TryStreamExt;
        let profile = db
            .collection::<mongodb::bson::Document>("system.profile")
            .find(doc! {"ns":{"$regex":format!("^{}\\.",db.name())}})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert!(
            profile.len() < 100,
            "bounded total commands: {}",
            profile.len()
        );
        for entry in &profile {
            assert!(!entry.get_bool("hasSortStage").unwrap_or(false));
            assert!(!entry
                .get_str("planSummary")
                .unwrap_or("")
                .contains("COLLSCAN"));
            if let Ok(command) = entry.get_document("command") {
                assert!(![
                    "insert",
                    "update",
                    "delete",
                    "commitTransaction",
                    "abortTransaction"
                ]
                .iter()
                .any(|key| command.contains_key(*key)));
            }
        }
        eprintln!(
            "s3_bounds segments={closed} source={measured:?} commands={}",
            profile.len()
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn transcript_long_turn_continues_without_replaying_prefix_and_preserves_serialized_parity() {
    let (_, db, store, agent, entries) = fixture(20, true).await;
    for limit in [1, 20, 32] {
        let result = collect(&store, &agent, limit).await;
        assert_eq!(
            serde_json::to_vec(&result).unwrap(),
            serde_json::to_vec(&expected(&agent, &entries)).unwrap()
        );
    }
    assert!(
        store.agent_log_transcript_projection(&agent).await.is_err(),
        "complete compatibility has a fixed budget"
    );
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    let result = crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut session)
        .await
        .unwrap();
    assert_eq!(
        result,
        AgentTitleTranscript::from_agent_log(&agent, &entries)
    );
    db.drop().await.unwrap();
}
