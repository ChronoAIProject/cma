use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn aborted_owner_save_does_not_adopt_a_replacement_conversation() {
    let f = Fixture::new(true).await;
    f.change_after_abort(
        "agents",
        doc! {"_id":f.requester().agent_id.as_str()},
        doc! {"$set":{"record.conversation.conversation_id":"replacement-native"}},
    );
    f.abort("insert", Some("agent_access_requests"), 1);
    let result = f
        .owner_save(f.original_services(), "original-identity")
        .await;
    let (fired, commands) = f.clear().await;
    let changed = f.changed_after_abort();
    let count = f
        .independent
        .agent_access_requests
        .count_documents(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(
        fired && changed,
        "replacement must occur after the exact aborted publication"
    );
    assert_starts(&commands, 2);
    assert_eq!(result, Err(AgentAccessError::Conflict));
    assert_eq!(
        count, 0,
        "retry must not issue consent for a new conversation"
    );
}
