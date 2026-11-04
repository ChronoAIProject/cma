use super::*;

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn message_feedback_survives_reopen_switches_clears_and_isolates_owner_and_agent() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("cma_feedback_{}", uuid::Uuid::new_v4().simple()));
    let owner = SubjectId::new("feedback-owner");
    let other = SubjectId::new("feedback-other");
    let agent = AgentId("agt_feedback".into());
    let foreign = AgentId("agt_foreign".into());
    let store = concurrency_tests::enabled_store(&database).await;
    store
        .set(&owner, &agent, 4, Some(MessageVote::Up))
        .await
        .unwrap();
    store
        .set(&owner, &agent, 8, Some(MessageVote::Down))
        .await
        .unwrap();
    store
        .set(&other, &agent, 4, Some(MessageVote::Down))
        .await
        .unwrap();
    store
        .set(&owner, &foreign, 4, Some(MessageVote::Down))
        .await
        .unwrap();
    drop(store);
    let store = concurrency_tests::enabled_store(&database).await;
    assert_eq!(
        store.list(&owner, &agent, 0, 8, 10).await.unwrap(),
        vec![
            MessageFeedback {
                sequence: 4,
                vote: MessageVote::Up
            },
            MessageFeedback {
                sequence: 8,
                vote: MessageVote::Down
            },
        ]
    );
    assert_eq!(store.list(&owner, &agent, 4, 8, 1).await.unwrap().len(), 1);
    assert_eq!(store.list(&owner, &agent, 0, 4, 10).await.unwrap().len(), 1);
    store
        .set(&owner, &agent, 4, Some(MessageVote::Down))
        .await
        .unwrap();
    store.set(&owner, &agent, 8, None).await.unwrap();
    store.set(&owner, &agent, 8, None).await.unwrap();
    assert_eq!(
        store.list(&owner, &agent, 0, 8, 10).await.unwrap(),
        vec![MessageFeedback {
            sequence: 4,
            vote: MessageVote::Down
        }]
    );
    assert_eq!(store.list(&other, &agent, 0, 8, 10).await.unwrap().len(), 1);
    assert_eq!(
        store.list(&owner, &foreign, 0, 8, 10).await.unwrap().len(),
        1
    );
    database.drop().await.unwrap();
}
#[path = "message_feedback_mongo_tests.rs"]
mod concurrency_tests;

#[path = "message_feedback_writer_tests.rs"]
mod writer_tests;
