use super::*;
use layer2_core::sandbox::AgentTaskStatus;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_status_uses_certified_original_sources_for_completed_and_incomplete_turns() {
    for gap in ["none", "during", "after"] {
        let (client, name) = database("segment_task_status").await;
        segments::migrate(&client.database(&name)).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
        agent.status = AgentStatus::Ready;
        agent.conversation.conversation_id = Some("native-fixture".into());
        agent.conversation.runtime_generation = Some(1);
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
            AgentLogKind::AgentMessage {
                content: "fixture answer".into(),
            },
        ];
        if gap == "during" {
            kinds.push(AgentLogKind::ConversationGap { frames: 2 });
        }
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        if gap == "after" {
            kinds.push(AgentLogKind::ConversationGap { frames: 2 });
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
        let (agent, source) = install(&store, agent, entries, 2).await;
        for version in [Bson::Int32(2), Bson::Int64(2)] {
            store
                .agents
                .update_one(
                    doc! {"_id":agent.id.as_str()},
                    doc! {"$set":{"document_schema_version":version}},
                )
                .await
                .unwrap();
            let mut session = client.start_session().snapshot(true).await.unwrap();
            let statuses =
                crate::store::agent_task_status::read_many(&store, [&agent], &mut session)
                    .await
                    .unwrap();
            assert_eq!(
                statuses[&agent.id],
                AgentTaskStatus::from_log(&agent, &source, false)
            );
            assert_eq!(
                statuses[&agent.id] == AgentTaskStatus::Completed,
                gap == "none"
            );
        }
        let filter = doc! {"_id":agent.id.as_str()};
        let original = store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        for update in [
            doc! {"$set":{"document_schema_version":3_i64}},
            doc! {"$set":{"document_schema_version":1_i64}},
            doc! {"$unset":{"agent_log_incarnation":""}},
        ] {
            store
                .agents
                .update_one(filter.clone(), update)
                .await
                .unwrap();
            let mut session = client.start_session().snapshot(true).await.unwrap();
            assert!(matches!(
                crate::store::agent_task_status::read_many(&store, [&agent], &mut session).await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            store
                .agents
                .replace_one(filter.clone(), original.clone())
                .await
                .unwrap();
        }
        store
            .agent_log
            .update_one(
                doc! {"agent_id":agent.id.as_str(),"seq":source.last().unwrap().seq as i64},
                doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
            )
            .await
            .unwrap();
        // Active rows have no immutable seal yet: change their length too so
        // the bounded active accounting detects the damaged source snapshot.
        store
            .agent_log
            .update_one(
                doc! {"agent_id":agent.id.as_str(),"seq":source.last().unwrap().seq as i64},
                doc! {"$set":{"unexpected":"damaged source"}},
            )
            .await
            .unwrap();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        assert!(
            crate::store::agent_task_status::read_many(&store, [&agent], &mut session)
                .await
                .is_err()
        );
        client.database(&name).drop().await.unwrap();
    }
}
