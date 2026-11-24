use super::*;
use layer2_core::sandbox::store::{
    owner_restart_ready, AgentRestartDispatch, AgentWorkPublication, AgentWorkPublicationLineage,
    AgentWorkPublicationParents,
};
use layer3_adapter::{AgentOpenRequest, AgentProcessAction};

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
    crate::store::MongoOperationStore::open_for_test(&client, &name)
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (mut stopped, source) = stopped_agent(&store).await;
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
    let proposal = owner_restart_ready(&armed, &lease.holder, lease.fence, at).unwrap();
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
    assert_eq!(after.len(), history.len() + 2);
    original.target = AgentControlTarget::from(&ready);
    original.observed_process = None;
    assert_eq!(
        store.schedule_agent_restart(original).await.unwrap(),
        accepted
    );
    assert_eq!(store.get_agent(&ready.id).await.unwrap(), Some(ready));
    client.database(&name).drop().await.unwrap();
}
