use super::*;

#[derive(Default)]
struct Reads {
    queries: usize,
    source_rows: usize,
    reply_bytes: usize,
    largest_reply: usize,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn sealed_page_work_is_bounded_at_one_ten_hundred_and_thousand_segments() {
    for closed in [1, 10, 100, 1000] {
        let (admin, name) = database("segment_bounds").await;
        segments::migrate(&admin.database(&name)).await.unwrap();
        let trace = Arc::new(Mutex::new(Reads::default()));
        let observed = trace.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let CommandEvent::Succeeded(event) = event else {
                return;
            };
            if !matches!(event.command_name.as_str(), "find" | "getMore") {
                return;
            }
            let bytes = bson::serialize_to_vec(&event.reply).unwrap().len();
            let mut reads = observed.lock().unwrap();
            reads.queries += 1;
            reads.reply_bytes += bytes;
            reads.largest_reply = reads.largest_reply.max(bytes);
            if let Ok(cursor) = event.reply.get_document("cursor") {
                if cursor
                    .get_str("ns")
                    .is_ok_and(|ns| ns.ends_with(".agent_log"))
                {
                    reads.source_rows += cursor
                        .get_array("firstBatch")
                        .or_else(|_| cursor.get_array("nextBatch"))
                        .map_or(0, Vec::len);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = segmented(&store, closed).await;
        *trace.lock().unwrap() = Reads::default();
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            store.agent_log_range(&agent.id, 0, 2, 2),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page, entries[..2]);
        {
            let reads = trace.lock().unwrap();
            assert_eq!(
                reads.source_rows, 2,
                "only requested sealed rows are decoded"
            );
            assert!(
                reads.queries <= 10,
                "page query count grew with retained segments"
            );
            assert!(
                reads.reply_bytes < 32 * 1024,
                "page bytes grew with retained segments"
            );
            assert!(
                reads.largest_reply < 16 * 1024,
                "reader materialized a lifetime result"
            );
        }
        *trace.lock().unwrap() = Reads::default();
        let mut with_predecessor = Vec::new();
        store
            .visit_agent_log_page_with_predecessor(
                &agent.id,
                agent.conversation.last_seq - 1,
                agent.conversation.last_seq,
                1,
                &mut |entry| {
                    with_predecessor.push(entry);
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(with_predecessor, entries[entries.len() - 2..]);
        assert_eq!(
            trace.lock().unwrap().source_rows,
            2,
            "one active-segment scan supplies both the page and boundary evidence"
        );
        *trace.lock().unwrap() = Reads::default();
        let current = store
            .agent_log_current_state(&agent, 0, 100)
            .await
            .unwrap()
            .unwrap();
        assert!(current.context.is_empty());
        assert!(current.approvals.items.is_empty());
        {
            let reads = trace.lock().unwrap();
            assert!(reads.queries <= 20);
            assert!(reads.source_rows <= 2);
            assert!(reads.reply_bytes < 64 * 1024);
        }
        for limit in [20_u16, 100, 200] {
            if entries.len() < usize::from(limit) {
                continue;
            }
            *trace.lock().unwrap() = Reads::default();
            let started = std::time::Instant::now();
            let page = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                store.agent_log_range(&agent.id, 0, u64::from(limit), limit),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(page, entries[..usize::from(limit)]);
            let reads = trace.lock().unwrap();
            assert_eq!(reads.source_rows, usize::from(limit));
            assert!(reads.queries <= usize::from(limit) * 4 + 10);
            assert!(reads.reply_bytes < 1024 * 1024);
            assert!(reads.largest_reply < 128 * 1024);
            eprintln!("segment_fixture closed={closed} rows={limit} queries={} bytes={} largest_reply={} elapsed_ms={}",
                reads.queries, reads.reply_bytes, reads.largest_reply, started.elapsed().as_millis());
        }
        admin.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn near_capacity_active_tail_uses_bounded_batches_with_authority_deadline_headroom() {
    for large in [false, true] {
        let (admin, name) = database("segment_active_bounds").await;
        segments::migrate(&admin.database(&name)).await.unwrap();
        let trace = Arc::new(Mutex::new(Reads::default()));
        let observed = trace.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let CommandEvent::Succeeded(event) = event else {
                return;
            };
            if !matches!(event.command_name.as_str(), "find" | "getMore") {
                return;
            }
            let bytes = bson::serialize_to_vec(&event.reply).unwrap().len();
            let mut reading = observed.lock().unwrap();
            reading.queries += 1;
            reading.reply_bytes += bytes;
            reading.largest_reply = reading.largest_reply.max(bytes);
            if let Ok(cursor) = event.reply.get_document("cursor") {
                if cursor
                    .get_str("ns")
                    .is_ok_and(|ns| ns.ends_with(".agent_log"))
                {
                    reading.source_rows += cursor
                        .get_array("firstBatch")
                        .or_else(|_| cursor.get_array("nextBatch"))
                        .map_or(0, Vec::len);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
        agent.conversation.conversation_id = Some("native-fixture".into());
        agent.conversation.runtime_generation = Some(1);
        agent.conversation.active_turn = Some(3);
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "native-fixture".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "fixture".into(),
            },
        ];
        for _ in 0..if large { 63 } else { 9996 } {
            kinds.push(AgentLogKind::AgentMessage {
                content: "x".repeat(if large { 1024 * 1024 } else { 1 }),
            });
        }
        let entries = kinds
            .into_iter()
            .enumerate()
            .map(|(offset, kind)| AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: offset as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .collect();
        let (agent, source) = install(&store, agent, entries, 0).await;
        *trace.lock().unwrap() = Reads::default();
        let started = std::time::Instant::now();
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            store.agent_log_range(
                &agent.id,
                agent.conversation.last_seq - 1,
                agent.conversation.last_seq,
                1,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page.as_slice(), &source[source.len() - 1..]);
        {
            let reads = trace.lock().unwrap();
            assert_eq!(reads.source_rows, source.len());
            assert!(
                reads.queries < 200,
                "active validation must batch its source rows"
            );
            assert!(reads.reply_bytes < 66 * 1024 * 1024);
            assert!(reads.largest_reply < 16 * 1024 * 1024 + 1024);
            eprintln!("active_fixture large={large} rows={} queries={} bytes={} largest_reply={} elapsed_ms={}",
                reads.source_rows, reads.queries, reads.reply_bytes, reads.largest_reply, started.elapsed().as_millis());
        }
        admin.database(&name).drop().await.unwrap();
    }
}
