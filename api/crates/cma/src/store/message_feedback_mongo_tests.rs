use super::*;
use layer2_core::sandbox::message_feedback::FeedbackReason;
use std::sync::Arc;

async fn database() -> Database {
    let client = mongodb::Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("isolated Mongo URI"),
    )
    .await
    .unwrap();
    client.database(&format!("cma_ticket10_{}", uuid::Uuid::new_v4().simple()))
}
fn reasons(version: u64) -> FeedbackReasonRequest {
    FeedbackReasonRequest {
        expected_version: version,
        request_id: format!("reasons-{version}"),
        reasons: vec![FeedbackReason::Inaccurate, FeedbackReason::Other],
        comment: "Check the source 🦀".into(),
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn message_feedback_mongo_concurrent_reasons_and_legacy_up_or_null_clear_atomically() {
    let database = database().await;
    let store = Arc::new(MongoMessageFeedbackStore::open(&database).await.unwrap());
    let owner = SubjectId::new("owner");
    let agent = AgentId("agt_ticket10".into());
    for target in [Some(MessageVote::Up), None] {
        for sequence in 1..=24 {
            store
                .set(&owner, &agent, sequence, Some(MessageVote::Down))
                .await
                .unwrap();
            let before = store.get(&owner, &agent, sequence).await.unwrap();
            let request = reasons(before.version);
            let gate = tokio::sync::Barrier::new(2);
            let (reason_result, vote_result) = tokio::join!(
                async {
                    gate.wait().await;
                    store.set_reasons(&owner, &agent, sequence, &request).await
                },
                async {
                    gate.wait().await;
                    store.set(&owner, &agent, sequence, target).await
                }
            );
            vote_result.unwrap();
            assert!(
                reason_result.is_ok()
                    || matches!(reason_result, Err(FeedbackReasonError::Conflict))
            );
            let reopened = MongoMessageFeedbackStore::open(&database).await.unwrap();
            let current = reopened.get(&owner, &agent, sequence).await.unwrap();
            assert_eq!(current.vote, target);
            assert!(current.reasons.is_empty());
            assert_eq!(current.comment, "");
            assert!(matches!(
                reopened
                    .set_reasons(&owner, &agent, sequence, &request)
                    .await,
                Err(FeedbackReasonError::Conflict)
            ));
            assert_eq!(
                reopened.get(&owner, &agent, sequence).await.unwrap(),
                current
            );
            reopened
                .set(&owner, &agent, sequence, Some(MessageVote::Down))
                .await
                .unwrap();
            assert!(reopened
                .get(&owner, &agent, sequence)
                .await
                .unwrap()
                .reasons
                .is_empty());
            assert!(matches!(
                reopened
                    .set_reasons(&owner, &agent, sequence, &request)
                    .await,
                Err(FeedbackReasonError::Conflict)
            ));
        }
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn message_feedback_mongo_concurrent_exact_retries_are_one_durable_version() {
    let database = database().await;
    let store = Arc::new(MongoMessageFeedbackStore::open(&database).await.unwrap());
    let owner = SubjectId::new("owner");
    let agent = AgentId("agt_ticket10".into());
    let votes = (0..12).map(|_| store.set(&owner, &agent, 5, Some(MessageVote::Down)));
    for result in futures_util::future::join_all(votes).await {
        result.unwrap();
    }
    let request = reasons(1);
    let writes = (0..12).map(|_| store.set_reasons(&owner, &agent, 5, &request));
    let results = futures_util::future::join_all(writes).await;
    let reopened = MongoMessageFeedbackStore::open(&database).await.unwrap();
    let current = reopened.get(&owner, &agent, 5).await.unwrap();
    assert_eq!(current.version, 2);
    assert_eq!(
        current.reasons,
        vec![FeedbackReason::Inaccurate, FeedbackReason::Other]
    );
    for result in results {
        assert_eq!(result.unwrap(), current);
    }
    assert_eq!(
        reopened
            .list(&owner, &agent, 0, 5, 200)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        reopened
            .get(&SubjectId::new("other"), &agent, 5)
            .await
            .unwrap(),
        FeedbackDetails::default()
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn message_feedback_mongo_legacy_vote_reads_empty_and_migrates_without_aba() {
    let database = database().await;
    let store = MongoMessageFeedbackStore::open(&database).await.unwrap();
    let owner = SubjectId::new("owner");
    let agent = AgentId("agt_ticket10".into());
    database
        .collection::<mongodb::bson::Document>(COLLECTION)
        .insert_one(doc! {
            "_id": identity(&owner, &agent, 5).unwrap(), "document_schema_version": 1,
            "owner": "owner", "agent_id": "agt_ticket10", "sequence": 5, "vote": "down"
        })
        .await
        .unwrap();
    let legacy = store.get(&owner, &agent, 5).await.unwrap();
    assert_eq!(legacy.vote, Some(MessageVote::Down));
    assert_eq!(legacy.version, 0);
    assert!(legacy.reasons.is_empty());
    let saved = store
        .set_reasons(&owner, &agent, 5, &reasons(0))
        .await
        .unwrap();
    assert_eq!(saved.version, 1);
    store.set(&owner, &agent, 5, None).await.unwrap();
    let reopened = MongoMessageFeedbackStore::open(&database).await.unwrap();
    assert!(reopened
        .list(&owner, &agent, 0, 5, 200)
        .await
        .unwrap()
        .is_empty());
    reopened
        .set(&owner, &agent, 5, Some(MessageVote::Down))
        .await
        .unwrap();
    let current = reopened.get(&owner, &agent, 5).await.unwrap();
    assert_eq!(current.version, 3);
    assert_eq!(current.comment, "");
    assert!(current.reasons.is_empty());
    assert!(matches!(
        reopened.set_reasons(&owner, &agent, 5, &reasons(0)).await,
        Err(FeedbackReasonError::Conflict)
    ));
    database.drop().await.unwrap();
}
