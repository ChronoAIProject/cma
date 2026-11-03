use super::*;
use layer2_core::sandbox::{
    AgentDescriptionSource, AgentMessageAssembly, AgentOperationService, AgentPreferencesUpdate,
    AgentReadService, AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeInputSender,
    AgentRuntimeRegistry, AgentRuntimeTarget, AgentRuntimeUpdate, AgentTitleOutcome,
    AgentTitleService, AgentTitleSource, AgentUpdateRecorder, DurableAgentUpdateRecorder,
    FirstMessagePurpose, FirstMessageRecord, SandboxResourceReadError, SandboxResourceReader,
    SandboxResourceReading,
};

struct TitlePorts;

#[async_trait::async_trait]
impl AgentRuntimeRegistry for TitlePorts {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: layer2_core::RuntimeCapabilities::new([
                layer2_core::RuntimeCapability::Conversation,
            ]),
            input: AgentRuntimeInputSender::spawn(4, |_| async { Ok(()) }),
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for TitlePorts {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        Err(SandboxResourceReadError::Unsupported(
            "No resource measurements in title proof".into(),
        ))
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn task_titles_generation_skips_setup_and_recovers_first_real_task_after_store_reopen() {
    let (client, name) = database("task_titles").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let mut agent = agent_for_worker("agt_task_titles", NOW);
    let mut first = FirstMessageRecord::new("setup", "Prepare the workspace", NOW).unwrap();
    first.purpose = Some(FirstMessagePurpose::Setup);
    first.fence_dispatch_for(1, 1, 1, NOW.into()).unwrap();
    first.complete(NOW.into()).unwrap();
    agent.conversation.first_message = Some(first);
    store.create_agent(agent.clone()).await.unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: Some("setup".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "Prepare the workspace".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Ready".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let service = AgentTitleService::new(store.clone(), "title-worker").unwrap();
    assert_eq!(
        service
            .generate_agent_title(&agent.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert!(service.title(&agent.id).await.unwrap().is_none());
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 5,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "比较 Rust 和 Go 的部署工具".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Here is the comparison".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 5 },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    assert_eq!(
        service
            .generate_agent_title(&agent.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Fallback
    );
    drop(service);
    drop(store);
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let service = AgentTitleService::new(reopened, "restarted-worker").unwrap();
    let title = service.title(&agent.id).await.unwrap().unwrap();
    assert_eq!(title.title.as_str(), "比较 Rust 和 Go 的部署工具");
    assert_eq!(title.source, AgentTitleSource::Fallback);
    assert_eq!(
        service
            .generate_agent_title(&agent.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn task_titles_authorized_reads_restore_task_source_and_manual_name_after_mongo_reopen() {
    let (client, name) = database("task_title_reads").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let parent = sandbox("sbx_task_title");
    store.create_sandbox(parent.clone()).await.unwrap();
    store
        .create_workspace(ready_workspace(&parent, "wks_atomic", "task-title"))
        .await
        .unwrap();
    let (worktree, mut agent) = worktree_and_agent(&parent.id);
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(NOW.into());
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    agent.conversation.conversation_id = Some("task-title-thread".into());
    agent.conversation.runtime_generation = Some(1);
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    let owner = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(store.clone(), Arc::new(TitlePorts));
    operations
        .send_message(&owner, &agent.id, "比较 Rust 和 Go 的部署工具".into())
        .await
        .unwrap();
    drop(operations);
    drop(store);
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let reads = AgentReadService::new(store.clone(), Arc::new(TitlePorts));
    let restored = reads.read(&owner, &agent.id).await.unwrap();
    assert_eq!(
        restored
            .conversation
            .observations
            .metadata
            .description
            .as_deref(),
        Some("比较 Rust 和 Go 的部署工具")
    );
    assert_eq!(
        restored
            .conversation
            .observations
            .metadata
            .description_source,
        Some(AgentDescriptionSource::PromptPreview)
    );
    assert!(
        !restored
            .conversation
            .observations
            .metadata
            .description_is_setup_preview
    );
    let operations = AgentOperationService::new(store.clone(), Arc::new(TitlePorts));
    operations
        .update_preferences(
            &owner,
            &agent.id,
            AgentPreferencesUpdate::Title {
                title: "工具选型".into(),
                expected_revision: 0,
            },
        )
        .await
        .unwrap();
    let mut update = AgentRuntimeUpdate::Metadata(Default::default());
    if let AgentRuntimeUpdate::Metadata(metadata) = &mut update {
        metadata.description = Some("Old provider name".into());
        metadata.description_source = Some(AgentDescriptionSource::ProviderTitle);
    }
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    drop(operations);
    drop(reads);
    drop(store);
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let reads = AgentReadService::new(store, Arc::new(TitlePorts));
    let restored = reads.read(&owner, &agent.id).await.unwrap();
    assert_eq!(
        restored.conversation.preferences.title.as_deref(),
        Some("工具选型")
    );
    assert_eq!(
        restored
            .conversation
            .observations
            .metadata
            .description
            .as_deref(),
        Some("Old provider name")
    );
    let other = Caller::new(
        SubjectId::new("other-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert!(matches!(
        reads.read(&other, &agent.id).await,
        Err(layer2_core::sandbox::AgentReadError::NotFound)
    ));
    client.database(&name).drop().await.unwrap();
}
