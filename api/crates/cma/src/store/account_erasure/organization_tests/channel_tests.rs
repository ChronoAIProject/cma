use super::*;

async fn channel_fixture(
    db: &Database,
) -> (crate::store::sandbox_mongo::MongoSandboxStore, String) {
    use layer2_core::profiles::{AgentProfileSnapshot, WorkspaceProfileSnapshot};
    use layer2_core::sandbox::{channel_usage::*, *};
    let binding = binding();
    let mut parent = crate::store::sandbox_mongo_integration_tests::sandbox("sbx_org");
    parent.owner = SubjectId::new("organization");
    let environment = binding.profile.environment.as_ref().unwrap();
    let workspace = Workspace {
        id: WorkspaceId("wsp_org".into()),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: layer2_core::WorkspaceProfileId("wkp_org".into()),
            revision: 1,
            digest: content_digest(&environment.workspace).unwrap(),
            content: environment.workspace.clone(),
        },
        name: "organization".into(),
        path: "/home/cma/organization".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    let worktree = Worktree {
        id: WorktreeId("wtr_org".into()),
        workspace_id: workspace.id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/organization/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "a".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(AgentId("agt_org".into())),
        revision: 1,
    };
    let agent = Agent {
        id: AgentId("agt_org".into()),
        worktree_id: worktree.id.clone(),
        workspace_id: workspace.id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        creation_key: "organization-agent".into(),
        creation_fingerprint: "a".repeat(64),
        profile: AgentProfileSnapshot {
            id: layer2_core::AgentProfileId("agp_org".into()),
            revision: 1,
            digest: content_digest(&binding.profile.agent).unwrap(),
            content: binding.profile.agent.clone(),
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            observations: Default::default(),
            settings: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    let creation = provenance();
    creation.validate_against(&agent, &parent).unwrap();
    put(
        db,
        "sandboxes",
        crate::store::sandboxes::sandbox_document(&parent, 0).unwrap(),
    )
    .await;
    put(
        db,
        "workspaces",
        crate::store::workspaces::workspace_document(&workspace).unwrap(),
    )
    .await;
    put(
        db,
        "worktrees",
        crate::store::worktrees::worktree_document(&worktree).unwrap(),
    )
    .await;
    put(
        db,
        "agents",
        crate::store::agents::agent_document(&agent).unwrap(),
    )
    .await;
    put(
        db,
        "resource_policies",
        records::policy_document(&policy()).unwrap(),
    )
    .await;
    put(
        db,
        "agent_creation_provenance",
        crate::store::agent_provenance::document(&creation).unwrap(),
    )
    .await;
    let before = AgentChannelUsageState::empty(creation.clone());
    let change = ChannelUsageChange::Attach {
        binding: ChannelBindingReference {
            binding_id: "historical-binding".into(),
            bridge_id: "historical-bridge".into(),
            partition_id: "historical-partition".into(),
            generation: 1,
            previous_binding_id: None,
        },
    };
    let reason = ChannelBindingChangeReason::Attachment;
    let mut after = before.clone();
    after.revision = 1;
    after.last_changed_at = Some(NOW.into());
    after.active_bindings.push(ActiveChannelBinding {
        reference: change.introduced_binding().unwrap().clone(),
        attached_at: NOW.into(),
        first_used_at: None,
        last_used_at: None,
    });
    let input = ChannelUsageMutation {
        creation: creation.clone(),
        policy_fence: ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 1,
        },
        expected_revision: 0,
        key_sha256: "d".repeat(64),
        actor: actor("erased"),
        change: change.clone(),
        reason,
        at: NOW.into(),
    };
    let event:ChannelUsageEvent=serde_json::from_value(serde_json::json!({"operation_id":input.operation_id(),"fingerprint":input.fingerprint(),"actor":input.actor,"at":NOW,"detached_at":null,"reason":reason,"change":change,"predecessor":null,"before":before,"after":after})).unwrap();
    event.validate().unwrap();
    let digest = event.digest();
    put(db,"channel_usage_events",doc! {"_id":&event.operation_id,"document_schema_version":1_i64,"agent_id":"agt_org","revision":1_i64,"binding_id":"historical-binding","digest":&digest,"record":bson::serialize_to_bson(&event).unwrap()}).await;
    let before = event.after.clone();
    let mut after = before.clone();
    after.revision = 2;
    after.first_used_at = Some(NOW.into());
    after.last_used_at = Some(NOW.into());
    after.active_bindings[0].first_used_at = Some(NOW.into());
    after.active_bindings[0].last_used_at = Some(NOW.into());
    let input = ChannelUsageMutation {
        creation: creation.clone(),
        policy_fence: ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 1,
        },
        expected_revision: 1,
        key_sha256: "e".repeat(64),
        actor: actor("survivor"),
        change: ChannelUsageChange::Use {
            binding_id: "historical-binding".into(),
        },
        reason: ChannelBindingChangeReason::AcceptedCommand,
        at: NOW.into(),
    };
    let second:ChannelUsageEvent=serde_json::from_value(serde_json::json!({"operation_id":input.operation_id(),"fingerprint":input.fingerprint(),"actor":input.actor,"at":NOW,"detached_at":null,"reason":input.reason,"change":input.change,"predecessor":null,"before":before,"after":after})).unwrap();
    second.validate().unwrap();
    put(db,"channel_usage_events",doc! {"_id":&second.operation_id,"document_schema_version":1_i64,"agent_id":"agt_org","revision":2_i64,"digest":second.digest(),"record":bson::serialize_to_bson(&second).unwrap()}).await;
    let stored = StoredAgentChannelUsage {
        state: second.after.clone(),
        latest_event_digest: second.digest(),
    };
    put(db,"agent_channel_usage",doc! {"_id":"agt_org","document_schema_version":1_i64,"revision":2_i64,"active":true,"record":bson::serialize_to_bson(&stored).unwrap()}).await;
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    (store, digest)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn channel_identity_erasure_keeps_current_snapshot_and_history_readable_in_every_phase() {
    use layer2_core::sandbox::store::{AgentChannelUsageStore, PageRequest};
    for order in [
        [
            "channel_usage_events",
            "agent_channel_usage",
            "agent_creation_provenance",
        ],
        [
            "agent_creation_provenance",
            "agent_channel_usage",
            "channel_usage_events",
        ],
    ] {
        let db = database().await;
        let (store, digest) = channel_fixture(&db).await;
        let id = AgentId("agt_org".into());
        store.agent_channel_usage_snapshot(&id).await.unwrap();
        for name in order {
            let originals: Vec<Document> = db
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"revision":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            for original in originals {
                assert_eq!(
                    redact(&db, name, &original, &subjects()).await.unwrap(),
                    Redaction::Updated,
                    "{name}"
                );
                store.agent_channel_usage_snapshot(&id).await.unwrap();
                let (_, first) = store
                    .agent_channel_usage_history(&id, &PageRequest::new(1, None).unwrap())
                    .await
                    .unwrap();
                store
                    .agent_channel_usage_history(&id, &PageRequest::new(1, first.next).unwrap())
                    .await
                    .unwrap();
            }
        }
        let (snapshot, history) = store
            .agent_channel_usage_history(&id, &PageRequest::new(10, None).unwrap())
            .await
            .unwrap();
        assert_eq!(
            snapshot.state.creation.created_by,
            Some(actor("deleted-account"))
        );
        assert_eq!(history.items[0].actor, actor("deleted-account"));
        assert_eq!(history.items[1].actor, actor("survivor"));
        assert_eq!(history.items[0].digest(), digest);
        let mut mutated = history.items[0].clone();
        mutated.after.creation.creation_fingerprint = "b".repeat(64);
        assert!(mutated.validate().is_err());
        assert_ne!(mutated.digest(), digest);
        let event_row = db
            .collection::<Document>("channel_usage_events")
            .find_one(doc! {"revision":1_i64})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event_row.get_str("digest").unwrap(), digest);
        db.collection::<Document>("channel_usage_events")
            .update_one(
                doc! {"_id":event_row.get("_id").unwrap().clone()},
                doc! {"$set":{"redacted_digest":"a".repeat(64)}},
            )
            .await
            .unwrap();
        assert!(store
            .agent_channel_usage_history(&id, &PageRequest::new(10, None).unwrap())
            .await
            .is_err());
        db.collection::<Document>("channel_usage_events")
            .replace_one(
                doc! {"_id":event_row.get("_id").unwrap().clone()},
                event_row.clone(),
            )
            .await
            .unwrap();
        db.collection::<Document>("channel_usage_events")
            .update_one(
                doc! {"_id":event_row.get("_id").unwrap().clone()},
                doc! {"$unset":{"mutation_version":""}},
            )
            .await
            .unwrap();
        assert!(store
            .agent_channel_usage_history(&id, &PageRequest::new(10, None).unwrap())
            .await
            .is_err());
        db.drop().await.unwrap();
    }
}
