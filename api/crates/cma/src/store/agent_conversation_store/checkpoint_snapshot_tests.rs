use super::*;

#[test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
fn metadata_only_command_counts_match_real_quiet_and_text_transactions() {
    use layer2_core::sandbox::{AgentUpdateRecorder, DurableAgentUpdateRecorder};
    use layer3_adapter::{ContentBlock, ContentChunk, ConversationUpdate, SessionUpdate};
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    metrics::with_local_recorder(&recorder, || {
        runtime.block_on(async {
            let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
            let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
            assert!(crate::store::mongo_diagnostics::install_configured(
                &mut options,
                "general",
                Some("false")
            )
            .is_none());
            assert!(options.command_event_handler.is_none());
            let client = mongodb::Client::with_options(options).unwrap();
            let name = format!("cma_1807_metadata_{}", uuid::Uuid::new_v4().simple());
            let store = MongoSandboxStore::open(&client, &name).await.unwrap();
            let current = working(&store, "sbx_metadata_counter").await;
            let recorder = DurableAgentUpdateRecorder::new(std::sync::Arc::new(store));
            let mut assembly = AgentMessageAssembly::default();
            let text = ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::from("private synthetic prefix")),
            ));
            let (ack, _) = tokio::sync::mpsc::channel(1);
            let quiet = ConversationUpdate::RuntimeOutput {
                receipt: layer3_adapter::RuntimeOutputReceipt::new(
                    layer3_adapter::RuntimeOutputSource {
                        stream_id: uuid::Uuid::new_v4().to_string(),
                        cursor: 1,
                        native_conversation_id: current
                            .conversation
                            .conversation_id
                            .clone()
                            .unwrap(),
                        payload_sha256: "a".repeat(64),
                    },
                    ack,
                ),
                update: None,
            };
            for update in [text, quiet] {
                crate::telemetry::agent_output::observe(recorder.record_update(
                    &current.id,
                    1,
                    &mut assembly,
                    update,
                    NOW.into(),
                ))
                .await
                .unwrap();
            }
            client.database(&name).drop().await.unwrap();
        })
    });
    let text = handle.render();
    for category in ["durable_assembly", "quiet_receipt"] {
        let sum = text
            .lines()
            .find(|line| {
                line.starts_with("cma_agent_output_mongo_commands_sum{")
                    && line.contains(&format!("category=\"{category}\""))
            })
            .unwrap();
        assert!(sum.ends_with(" 4"), "{sum}");
    }
    assert!(!text.contains("private"));
    assert!(!text.contains("sbx_metadata_counter"));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn atomic_snapshot_decodes_both_fields_and_rejects_corrupt_checkpoint() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_snapshot").await;
    let snapshot = store
        .agent_message_snapshot(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.agent, current);
    assert!(snapshot.checkpoint.is_none());
    let next = partial(&current, "durable prefix");
    store
        .commit_agent_message_update(current.clone(), None, next.clone(), None)
        .await
        .unwrap();
    let snapshot = store
        .agent_message_snapshot(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.agent, current);
    assert_eq!(snapshot.checkpoint, Some(next));
    assert!(store
        .agent_message_snapshot(&AgentId("absent".into()))
        .await
        .unwrap()
        .is_none());
    store
        .agents
        .update_one(
            doc! {"_id":current.id.as_str()},
            doc! {"$set":{"agent_message_checkpoint.version":99}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.agent_message_snapshot(&current.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn reused_checkpoint_row_cannot_bypass_a_history_seal_written_in_the_transaction() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_snapshot_seal").await;
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":current.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":current.id.as_str()},
            doc! {"$set":{"channel_history_seal":{"sealed":true}}},
        )
        .session(&mut session)
        .await
        .unwrap();
    let result = checkpoint::write_from_row(
        &store,
        &current,
        &row,
        Some(&partial(&current, "refused")),
        &mut session,
    )
    .await;
    assert!(matches!(
        result,
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    ));
    session.abort_transaction().await.unwrap();
    assert!(store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}
