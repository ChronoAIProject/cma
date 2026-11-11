use super::*;
use segments::audit;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_recaptures_changed_tail_without_restarting_sealed_prefix() {
    let (client, name) = database("segment_audit_movement").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 2).await;
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "capture", false)
            .await
            .unwrap(),
        "running"
    );
    // This later commit changes native identity. Only the unscanned active
    // tail is recaptured; already checked sealed segments remain complete.
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
    let source_work = crate::store::mongo_read_telemetry::isolated_test(async {
        for expected in ["running", "complete"] {
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "capture", false)
                    .await
                    .unwrap(),
                expected
            );
        }
        crate::store::mongo_read_telemetry::snapshot("agent_log")
    })
    .await;
    assert_eq!(&source_work[..3], &[2, 0, 5]);
    let progress = store
        .segment_logs
        .audits
        .find_one(doc! {"agent_id":agent.id.as_str(),"audit_id":"capture"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.get_i64("through_seq").unwrap(), 7);
    assert_eq!(
        progress
            .get_document("completed")
            .unwrap()
            .get_i64("checked_entries")
            .unwrap(),
        7
    );
    assert_eq!(progress.get_i64("fence").unwrap(), 3);
    assert_eq!(progress.get_i64("head_revision").unwrap(), 2);
    let semantic: layer2_core::sandbox::AgentLogProjectionState = bson::deserialize_from_document(
        progress
            .get_document("completed")
            .unwrap()
            .get_document("semantic")
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(
        semantic.latest_conversation,
        Some(("fixture-native".into(), 1))
    );
    assert_eq!(
        audit::install(&client, &db, agent.id.as_str(), "capture")
            .await
            .unwrap(),
        "installed"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_restart_checks_a_newly_sealed_tail_before_certifying_its_successor() {
    for damage in [false, true] {
        let (client, name) = database("segment_audit_rollover").await;
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, _) = segmented(&store, 1).await;
        assert_eq!(
            audit::run_unit(&client, &db, agent.id.as_str(), "rollover", false)
                .await
                .unwrap(),
            "running"
        );
        let filter = doc! {"_id":agent.id.as_str()};
        let mut head = Head::read(
            store
                .segment_logs
                .heads
                .find_one(filter.clone())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let raw: Vec<RawDocumentBuf> = store
            .agent_log
            .clone_with_type()
            .find(doc! {"agent_id":agent.id.as_str(),"seq":{"$gte":head.active_start_seq}})
            .sort(doc! {"seq":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let mut seal = Seal {
            id: Seal::identity(agent.id.as_str(), &head.incarnation, head.active_ordinal),
            format_version: 1,
            agent_id: agent.id.0.clone(),
            incarnation: head.incarnation.clone(),
            ordinal: head.active_ordinal,
            start_seq: head.active_start_seq,
            end_seq: head.last_seq,
            entry_count: head.active_count,
            encoded_bytes: head.active_bytes,
            previous_seal: head.previous_seal.clone(),
            prefix_count: head.lifetime_count,
            prefix_bytes: head.lifetime_bytes,
            leaf_digests: digest::binary(
                raw.iter()
                    .enumerate()
                    .flat_map(|(i, row)| {
                        digest::leaf(
                            agent.id.as_str(),
                            &head.incarnation,
                            head.active_start_seq + i as i64,
                            row.as_bytes(),
                        )
                        .bytes
                    })
                    .collect::<Vec<_>>(),
            ),
            seal_digest: digest::binary(vec![]),
            digest_version: digest::VERSION.into(),
            sealing_operation: "fixture-rollover".into(),
            source_format: "segmented_v1".into(),
            storage_kind: "mongo".into(),
        };
        seal.seal_digest = seal.digest();
        seal.validate().unwrap();
        store
            .segment_logs
            .seals
            .insert_one(bson::serialize_to_document(&seal).unwrap())
            .await
            .unwrap();
        head.revision += 1;
        head.active_ordinal += 1;
        head.active_start_seq = head.last_seq + 1;
        head.active_count = 0;
        head.active_bytes = 0;
        head.previous_seal = Some(seal.seal_digest);
        head.validate().unwrap();
        store
            .segment_logs
            .heads
            .replace_one(filter.clone(), bson::serialize_to_document(&head).unwrap())
            .await
            .unwrap();
        store
            .agents
            .update_one(filter, doc! {"$inc":{"agent_log_head_revision":1_i64}})
            .await
            .unwrap();
        if damage {
            store
                .agent_log
                .update_one(
                    doc! {"agent_id":agent.id.as_str(),"seq":3_i64},
                    doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
                )
                .await
                .unwrap();
        }
        let source_work = crate::store::mongo_read_telemetry::isolated_test(async {
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "rollover", false)
                    .await
                    .unwrap(),
                if damage { "corrupt" } else { "running" }
            );
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert_eq!(source_work[2], if damage { 1 } else { 2 });
        if damage {
            assert!(audit::install(&client, &db, agent.id.as_str(), "rollover")
                .await
                .is_err());
            assert!(store.agent_log_range(&agent.id, 0, 1, 1).await.is_err());
        }
        if !damage {
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "rollover", false)
                    .await
                    .unwrap(),
                "complete"
            );
            let progress = store
                .segment_logs
                .audits
                .find_one(doc! {"agent_id":agent.id.as_str(),"audit_id":"rollover"})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(progress.get_i64("fence").unwrap(), 3);
            assert_eq!(
                progress
                    .get_document("completed")
                    .unwrap()
                    .get_i64("next_ordinal")
                    .unwrap(),
                3
            );
            assert_eq!(
                progress
                    .get_document("completed")
                    .unwrap()
                    .get_i64("checked_entries")
                    .unwrap(),
                4
            );
            assert_eq!(progress.get_i64("head_revision").unwrap(), 2);
            assert_eq!(
                audit::install(&client, &db, agent.id.as_str(), "rollover")
                    .await
                    .unwrap(),
                "installed"
            );
        }
        db.drop().await.unwrap();
    }
}
