use super::*;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    execution_tests::fixture_command_execution, record, work_tests::live_request,
};
use layer2_core::sandbox::agent_io::{
    AgentCommandStore, AgentCommandWorker, AgentResponseStatus, CommandFailureCode,
};
use layer2_core::sandbox::store::{
    owner_restart_ready, AgentReopenGuard, AgentRestartDispatch, AgentWorkPublication,
    AgentWorkPublicationLineage, AgentWorkPublicationParents, OwnerRestartOutputFence,
    SandboxMutationGuard,
};
use layer2_core::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    AgentOpenRequest, AgentProcessAction, ConversationMetadata, ConversationUpdate,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn owner_restart_ready_reconciles_ambiguous_commit_and_keeps_history() {
    for segmented in [false, true] {
        Box::pin(publish_ready(segmented)).await;
    }
}

async fn publish_ready(segmented: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!(
        "cma_restart_ready_{}_{}",
        std::process::id(),
        DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    );
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let ambiguous_commits = Arc::new(AtomicU64::new(0));
    let observed = ambiguous_commits.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Succeeded(event) = event {
            if event.command_name == "commitTransaction"
                && event.reply.get_document("writeConcernError").is_ok()
            {
                observed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    crate::store::migrations::prepare_bindings_for_test(&client.database(&name)).await;
    crate::store::MongoOperationStore::open_for_test(&client, &name)
        .await
        .unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let predecessor = ready_conversation_fixture(&store, "restart-publication").await;
    let parent = store
        .get_sandbox(&predecessor.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let old_command = record(
        store
            .reserve_command(live_request(&predecessor, &parent, "before-stop"))
            .await
            .unwrap(),
    );
    let (mut stopped, source) = stop_fixture(&store, predecessor).await;
    if segmented {
        cancellation::adopt(&client, &name, &store, &stopped).await;
        stopped = store.get_agent(&stopped.id).await.unwrap().unwrap();
    }
    let history = store.agent_log(&stopped.id, 0, 20).await.unwrap();
    let mut original = request(&stopped, &source, "publish-ready", [1; 32]);
    let now = SystemTime::now();
    let at = layer2_core::rfc3339_at(now);
    original.requested_at.clone_from(&at);
    let accepted = store
        .schedule_agent_restart(original.clone())
        .await
        .unwrap();
    let starting = store.get_agent(&stopped.id).await.unwrap().unwrap();
    assert_eq!(
        starting.work.pre_restart_command_order,
        Some(old_command.receipt.agent_order)
    );
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let old_checkpoint = store.agent_message_checkpoint(&starting.id).await.unwrap();
    assert_predecessor_output_refused(&store, &recorder, &starting, &at).await;
    let claimed = store
        .claim_agent_work(
            &stopped.id,
            "restart-worker",
            at.clone(),
            layer2_core::rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let work = store.agent_restart_work(&claimed).await.unwrap();
    let sandbox = store
        .get_sandbox(&claimed.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let mut open = AgentOpenRequest::new(
        AgentProcessAction::Open.operation_id(claimed.id.as_str(), accepted.next_incarnation),
        crate::sandbox::runtime_options(&claimed, &sandbox, &worktree),
        Some(source.conversation_id.clone()),
    );
    open.restart_runtime = Some(source.runtime);
    let armed = store
        .prepare_agent_restart_dispatch(AgentRestartDispatch {
            claimed,
            work,
            open: open.clone(),
            dispatched_at: at.clone(),
        })
        .await
        .unwrap();
    let lease = armed.work.lease.as_ref().unwrap();
    let guard = AgentReopenGuard {
        sandbox: SandboxMutationGuard::capture(&sandbox),
        agent_fence: armed.work.next_fence,
        native_conversation_id: source.conversation_id,
        runtime_generation: 2,
        owner_restart: Some(OwnerRestartOutputFence {
            agent_id: armed.id.clone(),
            restart_operation: accepted.operation_id.clone(),
            open_operation: open.operation_id.clone(),
            open_request_sha256: open.request_sha256.clone(),
            lease: lease.clone(),
        }),
    };
    let observation = recorder
        .record_reopen_update(
            &armed.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            metadata("/home/cma/successor"),
            at.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(
        observation.appended.as_slice(),
        [AgentLogEntry {
            kind: AgentLogKind::RuntimeMetadataChanged { .. },
            ..
        }]
    ));
    let updated = store.get_agent(&armed.id).await.unwrap().unwrap();
    let successor_checkpoint = store.agent_message_checkpoint(&armed.id).await.unwrap();
    assert_ne!(successor_checkpoint, old_checkpoint);
    assert_eq!(successor_checkpoint.as_ref().unwrap().runtime_generation, 2);
    let proposal = owner_restart_ready(&updated, &lease.holder, lease.fence, at).unwrap();
    let parents = AgentWorkPublicationParents {
        sandbox_revision: sandbox.revision,
        workspace_revision: workspace.revision,
        worktree_revision: worktree.revision,
        lineage: AgentWorkPublicationLineage::Authorized,
    };
    let admin = client.database("admin");
    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": {"times": 5_i32},
            "data": {
                "failCommands": ["commitTransaction"], "appName": &name,
                "writeConcernError": {
                    "code": 50_i32, "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous Agent restart publication",
                },
            },
        })
        .await
        .unwrap();
    let expected = proposal.agent.clone();
    let publication = store.try_publish_agent_work(proposal, parents).await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert_eq!(ambiguous_commits.load(Ordering::Relaxed), 5);
    assert_eq!(publication.unwrap(), AgentWorkPublication::Published);
    let ready = store.get_agent(&stopped.id).await.unwrap().unwrap();
    assert_eq!(ready, expected);
    assert_eq!(
        ready.conversation.conversation_id,
        stopped.conversation.conversation_id
    );
    assert_eq!(ready.conversation.runtime_generation, Some(2));
    assert_eq!(ready.worktree_id, stopped.worktree_id);
    assert!(ready.work.cleanup_debt.is_empty());
    let row = store
        .agents
        .find_one(doc! {"_id": ready.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let retained: AgentOpenRequest = mongodb::bson::deserialize_from_document(
        row.get_document("control_restart_connection_v1")
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(retained, open);
    assert!(!row.contains_key("control_restart_active_v1"));
    assert!(!row.contains_key("control_restart_dispatch_v1"));
    let after = store.agent_log(&ready.id, 0, 20).await.unwrap();
    assert_eq!(&after[..history.len()], history);
    assert_eq!(after.len(), history.len() + 3);
    original.target = AgentControlTarget::from(&ready);
    original.observed_process = None;
    assert_eq!(
        store.schedule_agent_restart(original).await.unwrap(),
        accepted
    );
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    let (execution, sends) = fixture_command_execution(&store, &ready);
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "command-worker".into()).unwrap();
    let old_id = &old_command.receipt.envelope.command_id;
    worker.run_one(old_id).await.unwrap();
    let refused = store.get_command(old_id).await.unwrap().unwrap();
    assert_eq!(refused.receipt, old_command.receipt);
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    assert_eq!(
        refused.work.failure.as_ref().map(|failure| failure.code),
        Some(CommandFailureCode::AgentUnavailable)
    );
    assert!(refused.work.dispatch.is_none());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    let new_command = record(
        store
            .reserve_command(live_request(&ready, &parent, "after-restart"))
            .await
            .unwrap(),
    );
    let new_id = &new_command.receipt.envelope.command_id;
    worker.run_one(new_id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert!(store
        .get_command(new_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .dispatch
        .is_some());
    client.database(&name).drop().await.unwrap();
}

async fn assert_predecessor_output_refused(
    store: &MongoSandboxStore,
    recorder: &DurableAgentUpdateRecorder,
    starting: &Agent,
    at: &str,
) {
    let checkpoint = store.agent_message_checkpoint(&starting.id).await.unwrap();
    let history = store.agent_log(&starting.id, 0, 20).await.unwrap();
    for update in [
        metadata("/home/cma/delayed"),
        ConversationUpdate::Metadata(ConversationMetadata::default()),
    ] {
        assert!(matches!(
            recorder
                .record_output(
                    &starting.id,
                    1,
                    &mut AgentMessageAssembly::default(),
                    update,
                    at.into(),
                )
                .await,
            Err(AgentConversationError::StaleGeneration)
        ));
    }
    assert_eq!(
        store.agent_message_checkpoint(&starting.id).await.unwrap(),
        checkpoint
    );
    assert_eq!(store.agent_log(&starting.id, 0, 20).await.unwrap(), history);
    assert_eq!(
        store.get_agent(&starting.id).await.unwrap().as_ref(),
        Some(starting)
    );
}

fn metadata(path: &str) -> ConversationUpdate {
    ConversationUpdate::Metadata(ConversationMetadata {
        working_directory: Some(path.into()),
        ..Default::default()
    })
}
