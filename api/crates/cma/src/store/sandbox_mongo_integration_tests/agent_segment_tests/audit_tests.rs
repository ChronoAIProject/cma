use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_resumes_one_segment_at_a_time_and_certifies_only_its_capture() {
    let (client, name) = database("segment_audit").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    for expected in ["running", "running", "running", "complete", "complete"] {
        assert_eq!(
            segments::audit_unit(&client, &db, agent.id.as_str(), "fixture-audit", false)
                .await
                .unwrap(),
            expected
        );
    }
    let audit = db
        .collection::<Document>(segments::AUDITS)
        .find_one(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.get_i64("through_seq").unwrap(), 8);
    assert_eq!(audit.get_i64("checked_entries").unwrap(), 8);
    assert_eq!(audit.get_i64("next_ordinal").unwrap(), 4);
    assert_eq!(audit.get_i64("fence").unwrap(), 4);
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "cancelled-audit", true)
            .await
            .unwrap(),
        "cancelled"
    );
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "cancelled-audit", false)
            .await
            .unwrap(),
        "cancelled"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_quarantines_corruption_outside_an_earlier_successful_page() {
    let (client, name) = database("segment_quarantine").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":5_i64},
            doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    // The bounded read contract certifies touched rows; a full scrub detects
    // independently damaged bytes in a different retained physical segment.
    assert!(store.agent_log_range(&agent.id, 0, 1, 1).await.is_ok());
    for expected in ["running", "running", "corrupt"] {
        assert_eq!(
            segments::audit_unit(&client, &db, agent.id.as_str(), "damaged-audit", false)
                .await
                .unwrap(),
            expected
        );
    }
    assert!(store.agent_log_range(&agent.id, 0, 1, 1).await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_validates_captured_aggregate_when_live_conversation_advances() {
    let (client, name) = database("segment_audit_movement").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 2).await;
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "capture", false)
            .await
            .unwrap(),
        "running"
    );
    // This later commit changes native identity. The audit must continue to
    // certify the original aggregate and prefix, never today's aggregate.
    let event = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 7,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationOpened {
            conversation_id: "fixture-native".into(),
            runtime_generation: 1,
        },
    };
    let row = log_document(&event).unwrap();
    let bytes = bson::serialize_to_vec(&row).unwrap().len() as i64;
    store.agent_log.insert_one(row).await.unwrap();
    store.segment_logs.heads.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$inc":{"revision":1_i64,"last_seq":1_i64,"active_count":1_i64,"active_bytes":bytes,"lifetime_count":1_i64,"lifetime_bytes":bytes,"projection_through":1_i64}}).await.unwrap();
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {
        "$inc":{"last_seq":1_i64,"record.conversation.last_seq":1_i64,"agent_log_head_revision":1_i64},
        "$set":{"record.conversation.conversation_id":"fixture-native","record.conversation.runtime_generation":1_i64},
    }).await.unwrap();
    for expected in ["running", "complete"] {
        assert_eq!(
            segments::audit_unit(&client, &db, agent.id.as_str(), "capture", false)
                .await
                .unwrap(),
            expected
        );
    }
    let progress = store
        .segment_logs
        .audits
        .find_one(doc! {"agent_id":agent.id.as_str(),"audit_id":"capture"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.get_i64("through_seq").unwrap(), 6);
    assert_eq!(progress.get_i64("checked_entries").unwrap(), 6);
    assert_eq!(
        progress
            .get_document("semantic")
            .unwrap()
            .get("latest_conversation"),
        Some(&Bson::Null)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_refuses_held_fence_and_detects_extra_uncommitted_tail() {
    let (client, name) = database("segment_audit_fence").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 1).await;
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "capture", false)
            .await
            .unwrap(),
        "running"
    );
    let filter = doc! {"agent_id":agent.id.as_str(),"audit_id":"capture"};
    store
        .segment_logs
        .audits
        .update_one(
            filter.clone(),
            doc! {"$set":{"expires_at_ms":bson::DateTime::now().timestamp_millis()+60_000}},
        )
        .await
        .unwrap();
    assert!(matches!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "capture", false).await,
        Err(SandboxStoreError::Conflict)
    ));
    store
        .segment_logs
        .audits
        .update_one(filter, doc! {"$set":{"expires_at_ms":0_i64}})
        .await
        .unwrap();
    store
        .agent_log
        .insert_one(
            log_document(&AgentLogEntry {
                seq: 5,
                ..entries[3].clone()
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "capture", false)
            .await
            .unwrap(),
        "corrupt"
    );
    assert!(store.agent_log_range(&agent.id, 0, 1, 1).await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_crash_after_claim_resumes_with_a_new_fence_and_rejects_old_holder() {
    let (client, name) = database("segment_audit_crash").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 1).await;
    let claim = segments::claim_for_test(&client, &db, agent.id.as_str(), "crashed")
        .await
        .unwrap();
    assert_eq!(claim.get_i64("checked_entries").unwrap(), 0);
    assert!(matches!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "crashed", false).await,
        Err(SandboxStoreError::Conflict)
    ));
    let filter = doc! {"agent_id":agent.id.as_str(),"audit_id":"crashed"};
    store
        .segment_logs
        .audits
        .update_one(filter.clone(), doc! {"$set":{"expires_at_ms":0_i64}})
        .await
        .unwrap();
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "crashed", false)
            .await
            .unwrap(),
        "running"
    );
    let advanced = store
        .segment_logs
        .audits
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(advanced.get_i64("fence").unwrap(), 2);
    assert_eq!(advanced.get_i64("checked_entries").unwrap(), 2);
    assert!(matches!(
        segments::finish_for_test(&client, &db, claim).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store
            .segment_logs
            .audits
            .find_one(filter)
            .await
            .unwrap()
            .unwrap(),
        advanced
    );
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "crashed", false)
            .await
            .unwrap(),
        "complete"
    );
    db.drop().await.unwrap();
}
