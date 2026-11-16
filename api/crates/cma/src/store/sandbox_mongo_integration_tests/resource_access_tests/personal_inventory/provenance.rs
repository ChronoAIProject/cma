use super::*;
use axum::http::StatusCode;
use layer2_core::sandbox::{
    channel_usage::*,
    store::{AgentChannelUsageStore, AgentProvenanceStore},
};
use public::request;

const PATH: &str = "/api/v1/agents/provenance";

async fn ready(store: &MongoSandboxStore, index: usize, gap: bool) {
    let id = AgentId(format!("agt_list_{index:04}"));
    let mut agent = store.get_agent(&id).await.unwrap().unwrap();
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(NOW.into());
    agent.conversation.conversation_id = Some(format!("conversation-{index}"));
    agent.conversation.runtime_generation = Some(1);
    agent.work = layer2_core::sandbox::AgentWorkState::new(
        layer2_core::sandbox::AgentWorkStep::None,
        NOW.into(),
    );
    agent.conversation.last_seq = 4;
    agent.validate_conversation().unwrap();
    store
        .agents
        .replace_one(
            doc! {"_id":id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    for (seq, kind) in [
        (
            1,
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        ),
        (
            2,
            AgentLogKind::TurnStarted {
                turn_sequence: 2,
                model: None,
                origin_key: None,
            },
        ),
        (
            3,
            if gap {
                AgentLogKind::ConversationGap { frames: 1 }
            } else {
                AgentLogKind::AgentMessage {
                    content: "recorded result".into(),
                }
            },
        ),
        (4, AgentLogKind::TurnCompleted { turn_sequence: 2 }),
    ] {
        store
            .agent_log
            .insert_one(
                log_document(&AgentLogEntry {
                    agent_id: id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_provenance_preserves_status_filters_lookahead_and_rollback() {
    let f = fixture::Fixture::new(201, false).await;
    for i in 0..201 {
        ready(&f.store, i, i % 2 == 0).await;
    }
    let old = f.app(false, false);
    let new = f.app(true, false);
    for suffix in [
        "?limit=20",
        "?limit=200",
        "?limit=20&cursor=agt_list_0019",
        "?task_status=completed&limit=20",
        "?task_status=failed&limit=20",
        "?channel_active=true&limit=20",
        "?channel_history=true&limit=20",
        "?cursor=zzz",
        "?sandbox=sbx_list_0000&limit=200",
        "?workspace=wks_list_0000&limit=200",
        "?sandbox=sbx_list_0000&workspace=wks_list_0000&limit=200",
        "?sandbox=sbx_list_0000&status=failed&limit=200",
        "?sandbox=missing",
        "?workspace=missing",
    ] {
        let path = format!("{PATH}{suffix}");
        let expected = request(&old, &path).await;
        if !suffix.contains("missing") {
            assert_eq!(expected.0, StatusCode::OK, "{suffix}: {expected:?}");
        }
        if suffix.ends_with("limit=200") && !suffix.contains("status=failed") {
            assert_eq!(expected.1["items"].as_array().unwrap().len(), 200);
        }
        if suffix == "?limit=20" {
            assert_eq!(expected.1["items"][0]["task_status"], "failed");
            assert_eq!(expected.1["items"][1]["task_status"], "completed");
        }
        f.meter.take();
        assert_eq!(request(&new, &path).await, expected, "{suffix}");
        let stats = f.meter.take();
        println!("AGENT_PROVENANCE_FILTER path={path} stats={stats:?}");
        assert!(
            stats.count() <= 10,
            "complete filtered-page budget: {stats:?}"
        );
    }
    // Empty filtered pages still advance through authorized scanned rows.
    let empty = request(&new, &format!("{PATH}?channel_active=true&limit=20")).await;
    assert_eq!(empty.1["items"].as_array().unwrap().len(), 0);
    assert_eq!(empty.1["next"], "agt_list_0199");
    // Both old and new writers keep the same authoritative source format.
    for store in [&f.legacy, &f.store, &f.legacy, &f.store] {
        let id = AgentId("agt_list_0000".into());
        let mut agent = store.get_agent(&id).await.unwrap().unwrap();
        agent.revision += 1;
        agent.updated_at = format!("2026-09-05T00:{:02}:00Z", agent.revision);
        store.save_agent(agent.revision - 1, agent).await.unwrap();
        assert_eq!(request(&new, PATH).await, request(&old, PATH).await);
    }
    // Provenance scans authorize and validate hidden lookahead, unlike the
    // ordinary compatibility list's historical returned-row-only fence.
    f.store
        .worktrees
        .delete_one(doc! {"_id":"wkt_list_0020"})
        .await
        .unwrap();
    let path = format!("{PATH}?limit=20");
    let expected = request(&old, &path).await;
    assert!(!expected.0.is_success());
    assert_eq!(request(&new, &path).await, expected);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_provenance_requires_complete_creation_usage_and_task_evidence() {
    let f = fixture::Fixture::new(2, true).await;
    ready(&f.store, 0, true).await;
    let id = AgentId("agt_list_0000".into());
    let snapshot = f.store.agent_provenance_snapshot(&id).await.unwrap();
    let creation = snapshot.creation_provenance(&id).unwrap();
    f.store
        .agent_creation_provenance
        .insert_one(crate::store::agent_provenance::document(&creation).unwrap())
        .await
        .unwrap();
    let before = f.store.agent_channel_usage_snapshot(&id).await.unwrap();
    f.store
        .record_agent_channel_usage(ChannelUsageMutation {
            policy_fence: before.provenance.resource.fence(),
            creation: before.state.creation,
            expected_revision: 0,
            key_sha256: layer2_core::profiles::content_digest(&"list-attach").unwrap(),
            actor: ResourceActor::Human {
                subject: identity::owner(0),
            },
            change: ChannelUsageChange::Attach {
                binding: ChannelBindingReference {
                    binding_id: "list-binding".into(),
                    bridge_id: "list-bridge".into(),
                    partition_id: "list-partition".into(),
                    generation: 1,
                    previous_binding_id: None,
                },
            },
            reason: ChannelBindingChangeReason::Attachment,
            at: MINUTE.into(),
        })
        .await
        .unwrap();
    let old = f.app(false, false);
    let new = f.app(true, false);
    for suffix in [
        "",
        "?channel_active=true",
        "?channel_history=true",
        "?channel_active=false",
    ] {
        let path = format!("{PATH}{suffix}");
        let expected = request(&old, &path).await;
        assert_eq!(expected.0, StatusCode::OK, "{expected:?}");
        assert_eq!(request(&new, &path).await, expected);
    }
    for (collection, filter, update) in [
        (
            &f.store.agent_creation_provenance,
            doc! {},
            doc! {"$set":{"record.agent_id":"foreign"}},
        ),
        (
            &f.store.agent_creation_provenance,
            doc! {},
            doc! {"$set":{"document_schema_version":999_i64}},
        ),
        (
            &f.store.agent_log,
            doc! {"seq":1_i64},
            doc! {"$set":{"record.agent_id":"foreign"}},
        ),
        (
            &f.store.agent_log,
            doc! {"seq":3_i64},
            doc! {"$set":{"record.agent_id":"foreign"}},
        ),
        (
            &f.store.agent_log,
            doc! {"seq":4_i64},
            doc! {"$set":{"record.agent_id":"foreign"}},
        ),
        (
            &f.store.agent_channel_usage,
            doc! {},
            doc! {"$set":{"record.latest_event_digest":"stale"}},
        ),
        (
            &f.store.channel_usage_events,
            doc! {},
            doc! {"$set":{"record.actor.subject":"forged"}},
        ),
    ] {
        let original = collection.find_one(filter.clone()).await.unwrap().unwrap();
        collection.update_one(filter.clone(), update).await.unwrap();
        let expected = request(&old, PATH).await;
        assert!(!expected.0.is_success(), "{}", collection.name());
        assert_eq!(request(&new, PATH).await, expected, "{}", collection.name());
        collection.replace_one(filter, original).await.unwrap();
    }
    for collection in [&f.store.agent_channel_usage, &f.store.channel_usage_events] {
        let original = collection.find_one(doc! {}).await.unwrap().unwrap();
        collection.delete_one(doc! {}).await.unwrap();
        let expected = request(&old, PATH).await;
        assert!(!expected.0.is_success());
        assert_eq!(request(&new, PATH).await, expected);
        collection.insert_one(original).await.unwrap();
    }
    f.store
        .agent_commands
        .insert_one(doc! {"_id":"bad-command","agent_id":id.as_str(),"status":"queued"})
        .await
        .unwrap();
    let expected = request(&old, PATH).await;
    assert!(!expected.0.is_success());
    assert_eq!(request(&new, PATH).await, expected);
    f.close().await;
}
