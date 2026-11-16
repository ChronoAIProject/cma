use super::*;
use layer2_core::sandbox::{
    AgentTitleEffectIntent, AgentTitleGenerationError, AgentTitleGenerationResponse,
    AgentTitleOutcome, AgentTitleService, PreparedAgentTitle,
};

struct Prepared;
#[async_trait::async_trait]
impl PreparedAgentTitle for Prepared {
    fn request_digest(&self) -> &str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
    fn strategy(&self) -> &str {
        "segmented-title-fixture"
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        panic!("stored result replay cannot dispatch")
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn title_custody_segmented_result_replays_on_second_client_with_flag_off() {
    let (_, db, store, agent, entries) = fixture(3, false).await;
    segments::mongo_tests::fixtures::certify_output(&store, &agent, &entries).await;
    let store = store.with_title_effect_custody(true);
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(agent.id.clone(), "origin"),
            std::time::SystemTime::now(),
        )
        .await
        .unwrap()
        .unwrap();
    let transcript = store
        .agent_title_transcript(&agent.id)
        .await
        .unwrap()
        .unwrap();
    let intent = AgentTitleEffectIntent::new(&claim, &transcript, &Prepared).unwrap();
    let title = AgentTitle::new("Retained segmented result").unwrap();
    assert!(store
        .begin_agent_title_effect(&claim, &intent)
        .await
        .unwrap());
    assert!(store
        .accept_agent_title_effect(&claim, &intent, &title)
        .await
        .unwrap());
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {
                "title_lease_acquired_at": mongodb::bson::DateTime::from_millis(1000),
                "title_lease_expires_at": mongodb::bson::DateTime::from_millis(46000),
            }},
        )
        .await
        .unwrap();
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let peer = std::sync::Arc::new(MongoSandboxStore::open(&client, db.name()).await.unwrap());
    let service = AgentTitleService::new(peer.clone(), "successor-off").unwrap();
    assert_eq!(
        service
            .generate_agent_title(&agent.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(
        peer.agent_title(&agent.id).await.unwrap().unwrap().title,
        title
    );
    assert_eq!(
        peer.get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        agent.conversation.last_seq + 1
    );
    assert_eq!(
        store
            .segment_logs
            .publications
            .count_documents(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap(),
        1
    );
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert_eq!(row.get_i32("title_effect_floor").unwrap(), 2);
    assert_eq!(
        peer.agent_title_effect(&agent.id).await.unwrap().accepted(),
        Some(&title)
    );
    db.drop().await.unwrap();
}
