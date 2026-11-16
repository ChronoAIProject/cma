//! Actual keyed and segmented-control roots retain original inputs after aborts.
use super::*;
use crate::store::agent_log_segments::publication;
use layer2_core::sandbox::{store::*, *};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

async fn authority(f: &Fixture) -> Sandbox {
    let mut parent = lifecycle_caller::parent(f).await;
    parent
        .work
        .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into());
    parent.revision += 1;
    f.store
        .save_sandbox(parent.revision - 1, parent.clone())
        .await
        .unwrap();
    f.store
        .create_worktree(Worktree {
            id: f.agent.worktree_id.clone(),
            workspace_id: f.agent.workspace_id.clone(),
            sandbox_id: f.agent.sandbox_id.clone(),
            owner: f.agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/receipt/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(f.agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
    f.store
        .claim_sandbox_work(&parent.id, "boundary-worker", now(), deadline())
        .await
        .unwrap()
        .unwrap()
}

fn now() -> String {
    layer2_core::now_rfc3339()
}
fn deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(300))
}

async fn observed(f: &mut Fixture) -> (Client, String, Arc<Mutex<Vec<Document>>>) {
    let admin = f.client.clone();
    let app = format!("m4-boundary-{}", uuid::Uuid::new_v4().simple());
    let trace = Arc::new(Mutex::new(Vec::new()));
    let seen = trace.clone();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            seen.lock().unwrap().push(event.command);
        }
    }));
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap()
        .with_renewable_work(true);
    (admin, app, trace)
}

async fn fault(admin: &Client, app: &str, unknown: bool) {
    let mut labels = vec!["TransientTransactionError"];
    if unknown {
        labels.push("UnknownTransactionCommitResult");
    }
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1},
        "data":{"appName":app,"failCommands":["update"],"errorCode":112_i32,"errorLabels":labels},
    }).await.unwrap();
}
async fn clear(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}
fn identity(kind: &AgentLogKind) -> AgentLogPublicationIdentity {
    AgentLogPublicationIdentity {
        key: "fixture".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn keyed_and_segmented_control_retry_aborted_start_with_original_authority() {
    for control in [false, true] {
        let mut f = fixture(2).await;
        f.store = f.store.clone().with_renewable_work(true);
        let claimed = authority(&f).await;
        let independent = f.store.clone();
        let (admin, app, trace) = observed(&mut f).await;
        trace.lock().unwrap().clear();
        let result = WorkAuthority::new(
            ClaimedWork::Sandbox(Box::new(claimed.clone())),
            Instant::now(),
        )
        .run(
            async {
                fault(&admin, &app, false).await;
                crate::store::work_publication::scope(&f.store, async {
                    if control {
                        f.store
                            .schedule_agent_stop(AgentStopRequest {
                                target: AgentControlTarget::from(&f.agent),
                                reason: AgentStopReason::Owner,
                                requested_at: NOW.into(),
                            })
                            .await
                            .map(|result| matches!(result, AgentControlResult::Scheduled(_)))
                            .map_err(|e| SandboxStoreError::Unavailable(format!("{e:?}")))
                    } else {
                        let kind = AgentLogKind::ConversationGap { frames: 1 };
                        f.store
                            .append_agent_log_keyed(&f.agent.id, &identity(&kind), kind)
                            .await
                            .map(|entry| entry.seq == 3)
                            .map_err(|e| SandboxStoreError::Unavailable(format!("{e:?}")))
                    }
                })
                .await
            },
            || async {
                independent
                    .renew_sandbox_authority(&claimed, &now(), &deadline())
                    .await
            },
        )
        .await;
        clear(&admin).await;
        let starts = trace
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c.get_bool("startTransaction"), Ok(true)))
            .cloned()
            .collect::<Vec<_>>();
        let row = f
            .store
            .agents
            .find_one(doc! {"_id":f.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let publications = collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap();
        f.destroy().await;
        assert_eq!(
            result,
            Ok(true),
            "aborted transaction-start fence must retry original writer"
        );
        assert_eq!(starts.len(), 2);
        assert!(
            starts[0].get("lsid") != starts[1].get("lsid")
                || starts[0].get("txnNumber") != starts[1].get("txnNumber")
        );
        assert_eq!(publications, 1);
        assert!(!row.contains_key("keyed_publication_attempt_v1"));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn keyed_uncertain_precommit_error_retains_original_claim_without_replay() {
    let mut f = fixture(2).await;
    let admin = f.client.clone();
    let app = format!("m4-keyed-unknown-{}", uuid::Uuid::new_v4().simple());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let control = admin.clone();
    options.command_event_handler = Some(EventHandler::async_callback(move |event| {
        let control = control.clone();
        let app = app.clone();
        Box::pin(async move {
            if let CommandEvent::Started(event) = event {
                // Durable claim acquisition is outside the transaction. Inject
                // only when its consume write starts the publication transaction.
                if matches!(event.command.get_str("update"), Ok("agents"))
                    && matches!(event.command.get_bool("startTransaction"), Ok(true))
                {
                    fault(&control, &app, true).await;
                }
            }
        })
    }));
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let operation = identity(&kind);
    let result = f
        .store
        .append_agent_log_keyed(&f.agent.id, &operation, kind.clone())
        .await;
    clear(&admin).await;
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let second = f
        .store
        .append_agent_log_keyed(&f.agent.id, &operation, kind)
        .await;
    clear(&admin).await;
    let count = f.store.agent_log.count_documents(doc! {}).await.unwrap();
    f.destroy().await;
    assert!(result.is_err());
    assert!(second.is_err());
    assert_eq!(count, 2);
    assert!(
        row.contains_key("keyed_publication_attempt_v1"),
        "uncertain precommit error released original claim"
    );
}
