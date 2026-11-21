use super::*;

// Frozen pre-#1311 storage contract, including its non-null vote requirement.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousRecord {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    owner: String,
    agent_id: String,
    sequence: i64,
    vote: MessageVote,
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn message_feedback_writer_admission_preserves_legacy_and_retained_records() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = client.database(&format!(
        "cma_writer_feedback_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut store = MongoMessageFeedbackStore::open(&database).await.unwrap();
    let owner = SubjectId::new("selected-owner");
    let other = SubjectId::new("other-owner");
    let agent = AgentId("agt_writer_feedback".into());
    store.writer_policy = Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode":"accounts", "accounts":[owner.as_str()], "capabilities":["replay_presentation"]
        }))
        .unwrap(),
    );
    store
        .set(&other, &agent, 1, Some(MessageVote::Down))
        .await
        .unwrap();
    store
        .set(&other, &agent, 1, Some(MessageVote::Up))
        .await
        .unwrap();
    let raw = store
        .records
        .find_one(doc! {"owner":other.as_str()})
        .await
        .unwrap()
        .unwrap();
    let legacy: PreviousRecord = mongodb::bson::deserialize_from_document(raw.clone()).unwrap();
    assert_eq!(legacy.id, identity(&other, &agent, 1).unwrap());
    assert_eq!(
        (
            legacy.document_schema_version,
            legacy.owner.as_str(),
            legacy.agent_id.as_str(),
            legacy.sequence,
            legacy.vote
        ),
        (1, other.as_str(), agent.as_str(), 1, MessageVote::Up)
    );
    assert!(matches!(
        store.set(&other, &agent, 1, None).await,
        Err(AgentReadError::Forbidden)
    ));
    assert_eq!(
        store
            .records
            .find_one(doc! {"owner":other.as_str()})
            .await
            .unwrap()
            .unwrap(),
        raw
    );
    store
        .set(&owner, &agent, 2, Some(MessageVote::Down))
        .await
        .unwrap();
    let request = FeedbackReasonRequest {
        expected_version: 1,
        request_id: "accepted-reasons".into(),
        reasons: vec![layer2_core::sandbox::message_feedback::FeedbackReason::Unclear],
        comment: "Explain the evidence".into(),
    };
    let saved = store
        .set_reasons(&owner, &agent, 2, &request)
        .await
        .unwrap();
    let expanded = store
        .records
        .find_one(doc! {"owner":owner.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(mongodb::bson::deserialize_from_document::<PreviousRecord>(expanded.clone()).is_err());
    let closed = MongoMessageFeedbackStore::open(&database).await.unwrap();
    assert_eq!(closed.get(&owner, &agent, 2).await.unwrap(), saved);
    assert_eq!(
        closed
            .set_reasons(&owner, &agent, 2, &request)
            .await
            .unwrap(),
        saved
    );
    assert_eq!(
        closed.list(&other, &agent, 0, 2, 10).await.unwrap().len(),
        1
    );
    assert!(matches!(
        closed.set(&owner, &agent, 2, Some(MessageVote::Up)).await,
        Err(AgentReadError::Forbidden)
    ));
    let next = FeedbackReasonRequest {
        expected_version: saved.version,
        request_id: "new-reasons".into(),
        ..request
    };
    assert!(matches!(
        closed.set_reasons(&owner, &agent, 2, &next).await,
        Err(FeedbackReasonError::Read(AgentReadError::Forbidden))
    ));
    assert_eq!(
        closed
            .records
            .find_one(doc! {"owner":owner.as_str()})
            .await
            .unwrap()
            .unwrap(),
        expanded
    );
    assert_eq!(closed.get(&other, &agent, 1).await.unwrap().version, 0);
    // An admitted user keeps the normal clear operation and versioned recovery.
    store.set(&owner, &agent, 2, None).await.unwrap();
    assert!(closed.get(&owner, &agent, 2).await.unwrap().vote.is_none());
    database.drop().await.unwrap();
}
