use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_log_prefix_uses_one_complete_snapshot_scan_and_retains_integrity_checks() {
    let (admin, name) = database("agent_log_prefix").await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = Arc::new(Mutex::new(Vec::<Document>::new()));
    let observed = Arc::clone(&trace);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                observed.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_prefix");
    store.create_sandbox(parent.clone()).await.unwrap();
    let (_, agent) = worktree_and_agent(&parent.id);
    store.create_agent(agent.clone()).await.unwrap();

    let entries: Vec<_> = (1..=405)
        .map(|seq| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::ModeChanged {
                mode: "code".into(),
            },
        })
        .collect();
    store
        .agent_log
        .insert_many(entries.iter().map(|entry| log_document(entry).unwrap()))
        .await
        .unwrap();
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": { "last_seq": 405_i64, "record.conversation.last_seq": 405_i64 } },
        )
        .await
        .unwrap();

    trace.lock().unwrap().clear();
    assert_eq!(
        store.agent_log_prefix(&agent.id, 405).await.unwrap(),
        entries
    );
    let commands = trace.lock().unwrap().clone();
    assert_eq!(
        commands.len(),
        2,
        "one aggregate read and one complete log scan"
    );
    assert!(command_uses_snapshot_read(&commands[0]));
    assert_eq!(
        commands[0].get_document("lsid").unwrap(),
        commands[1].get_document("lsid").unwrap()
    );
    assert_eq!(commands[1].get_str("find").unwrap(), AGENT_LOG);
    assert_eq!(
        store.agent_log_prefix(&agent.id, 200).await.unwrap(),
        entries[..200]
    );
    assert_eq!(
        store.agent_log(&agent.id, 400, 10).await.unwrap(),
        entries[400..]
    );
    assert!(store
        .agent_log_prefix(&agent.id, 0)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.agent_log_prefix(&agent.id, 406).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.agent_log_prefix(&AgentId("missing".into()), 1).await,
        Err(SandboxStoreError::SequenceConflict)
    );

    let last = entries.last().unwrap();
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 405_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_log_prefix(&agent.id, 200).await,
        Err(SandboxStoreError::SequenceConflict),
        "a gap beyond the requested prefix must fail closed"
    );
    store
        .agent_log
        .insert_one(log_document(last).unwrap())
        .await
        .unwrap();

    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 405_i64 },
            doc! { "$set": { "record.agent_id": "agt_foreign" } },
        )
        .await
        .unwrap();
    assert!(
        matches!(
            store.agent_log_prefix(&agent.id, 200).await,
            Err(SandboxStoreError::Corrupt(_))
        ),
        "foreign identity beyond the prefix must fail closed"
    );
    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 405_i64 },
            doc! { "$set": { "record.agent_id": agent.id.as_str() } },
        )
        .await
        .unwrap();

    let orphan = AgentLogEntry {
        seq: 406,
        ..last.clone()
    };
    store
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.agent_log_prefix(&agent.id, 200).await,
        Err(SandboxStoreError::SequenceConflict),
        "uncommitted tail beyond the prefix must fail closed"
    );
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 406_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_log_prefix(&agent.id, 405).await.unwrap(),
        entries
    );
    admin.database(&name).drop().await.unwrap();
}

mod bounded_window_tests;
