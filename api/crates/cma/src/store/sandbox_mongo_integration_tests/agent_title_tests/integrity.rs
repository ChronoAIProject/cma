use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn corrupt_transcript_and_full_log_capacity_fail_without_partial_writes() {
    let (client, name) = database("agent_titles_integrity").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let corrupt_id = create_eligible_agent(&store, "agt_title_gap").await;
    store
        .agent_log
        .delete_one(doc! { "agent_id": corrupt_id.as_str(), "seq": 2_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_title_transcript(&corrupt_id).await,
        Err(AgentTitleStoreError::SequenceConflict)
    );
    let filter = doc! { "_id": corrupt_id.as_str() };
    assert!(!store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap()
        .contains_key("agent_log_integrity_quarantine_v1"));
    assert_eq!(
        store
            .acquire_agent_title_claim(
                &AgentTitleClaimRequest::new(corrupt_id.clone(), "holder"),
                claim_time(),
            )
            .await,
        Err(AgentTitleStoreError::SequenceConflict)
    );
    assert!(
        store
            .agents
            .find_one(filter)
            .await
            .unwrap()
            .unwrap()
            .contains_key("agent_log_integrity_quarantine_v1"),
        "the explicit writer audit owns quarantine"
    );
    assert_eq!(
        store
            .acquire_agent_title_claim(
                &AgentTitleClaimRequest::new(corrupt_id.clone(), "holder"),
                claim_time()
            )
            .await,
        Err(AgentTitleStoreError::Corrupt)
    );
    assert!(store
        .agents
        .find_one(doc! { "_id": corrupt_id.as_str(), "title_attempted": false })
        .await
        .unwrap()
        .is_some());

    let full_id = create_title_agent(&store, "agt_title_capacity").await;
    let logs = (1..=NORMAL_AGENT_LOG_ENTRIES)
        .map(|seq| AgentLogEntry {
            agent_id: full_id.clone(),
            seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::Deleted,
        })
        .map(|entry| log_document(&entry).unwrap())
        .collect::<Vec<_>>();
    store.agent_log.insert_many(logs).await.unwrap();
    let mut agent = store.get_agent(&full_id).await.unwrap().unwrap();
    agent.conversation.last_seq = NORMAL_AGENT_LOG_ENTRIES;
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert("last_seq", i64::try_from(NORMAL_AGENT_LOG_ENTRIES).unwrap());
    store
        .agents
        .update_one(doc! { "_id": full_id.as_str() }, doc! { "$set": fields })
        .await
        .unwrap();
    assert_eq!(
        store
            .set_user_agent_title(
                &full_id,
                &AgentTitle::new("No capacity").unwrap(),
                MINUTE.into(),
            )
            .await,
        Err(AgentTitleStoreError::AgentLogCapacityExceeded)
    );
    assert_eq!(store.agent_title(&full_id).await.unwrap(), None);
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": full_id.as_str() })
            .await
            .unwrap(),
        NORMAL_AGENT_LOG_ENTRIES
    );
    client.database(&name).drop().await.unwrap();
}
