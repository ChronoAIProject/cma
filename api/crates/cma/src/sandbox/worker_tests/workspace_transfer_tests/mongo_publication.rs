use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::{SandboxTransaction, SandboxTransactionStore};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::{options::ClientOptions, Client};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
struct PublicationGate {
    held: AtomicBool,
    entered: Notify,
    released: Mutex<bool>,
    release: Condvar,
}

impl PublicationGate {
    fn observe(&self, event: CommandEvent) {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("update").ok() == Some("workspaces")
                && event.command.get_bool("autocommit").ok() == Some(false)
                && event
                    .command
                    .get_array("updates")
                    .ok()
                    .is_some_and(|updates| {
                        updates
                            .iter()
                            .filter_map(|update| update.as_document())
                            .any(|update| {
                                update.get_array("u").ok().is_some_and(|stages| {
                                    stages.iter().filter_map(|stage| stage.as_document()).any(
                                        |stage| {
                                            stage
                                                .get_document("$set")
                                                .ok()
                                                .and_then(|fields| {
                                                    fields.get_document("status").ok()
                                                })
                                                .and_then(|status| status.get_str("$literal").ok())
                                                == Some("ready")
                                        },
                                    )
                                })
                            })
                    })
                && !self.held.swap(true, Ordering::SeqCst)
            {
                self.entered.notify_one();
                let (released, _) = self
                    .release
                    .wait_timeout_while(
                        self.released.lock().unwrap(),
                        Duration::from_secs(20),
                        |released| !*released,
                    )
                    .unwrap();
                assert!(*released, "publication gate must be released");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn setup_and_prepared_completion_obey_parent_commit_order() {
    for prepared in [false, true] {
        for final_status in [SandboxStatus::Installing, SandboxStatus::Retiring] {
            let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
            let (pools, mut parent, mut workspace, completed, _) = owned(&harness).await;
            let now = SystemTime::now();
            parent.revision = 1;
            workspace.revision = 1;
            workspace.work.ready_at = rfc3339_at(now);
            if !prepared {
                workspace.work.step = WorkspaceWorkStep::Setup;
                workspace
                    .record_resolved_commit(completed.prepare.resolved_commit.clone())
                    .unwrap();
            }
            let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
            let admin = Client::with_uri_str(&uri).await.unwrap();
            let name = format!("cma_worker_ready_{}", uuid::Uuid::new_v4().simple());
            let gate = Arc::new(PublicationGate::default());
            let observe = gate.clone();
            let mut options = ClientOptions::parse(&uri).await.unwrap();
            options.command_event_handler =
                Some(EventHandler::callback(move |event| observe.observe(event)));
            let client = Client::with_options(options).unwrap();
            let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
            let peer = MongoSandboxStore::open(&admin, &name).await.unwrap();
            store.create_sandbox(parent.clone()).await.unwrap();
            store.create_workspace(workspace.clone()).await.unwrap();
            let worker = WorkspaceWorker::new(
                WorkspaceWorkerDependencies {
                    store: store.clone(),
                    adapter: adapter(&harness, pools),
                },
                WorkspaceWorkerConfig::new("publication", "test"),
            );
            let requests_before = harness.backend.requests().await.len();
            let mut pending = if prepared {
                let claimed = store
                    .claim_workspace_work(
                        &workspace.id,
                        "publication",
                        rfc3339_at(now),
                        rfc3339_at(now + Duration::from_secs(60)),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                let worker = worker.clone();
                let original_parent = parent.clone();
                tokio::spawn(async move {
                    worker
                        .complete_prepared(&claimed, &original_parent, completed, now)
                        .await
                })
            } else {
                let worker = worker.clone();
                let id = workspace.id.clone();
                tokio::spawn(async move { worker.run_one(&id, now).await.map(|_| ()) })
            };
            tokio::time::timeout(Duration::from_secs(15), async {
                tokio::select! {
                    () = gate.entered.notified() => (),
                    result = &mut pending => panic!(
                        "completion finished before the Ready write: prepared={prepared}, parent={final_status:?}, result={result:?}"
                    ),
                }
            }).await.expect("worker must reach the Ready publication gate");
            let claimed = peer.get_workspace(&workspace.id).await.unwrap().unwrap();
            assert_eq!(claimed.status, WorkspaceStatus::Preparing);
            assert_eq!(claimed.work.lease.as_ref().unwrap().holder, "publication");
            if final_status != SandboxStatus::Installing {
                let mut retired = parent.clone();
                retired.begin_retirement(rfc3339_at(now)).unwrap();
                peer.commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: None,
                    expected_revision: parent.revision,
                    sandbox: Box::new(retired.clone()),
                    event_at: rfc3339_at(now),
                    event: SandboxEventKind::StatusChanged {
                        from: SandboxStatus::Installing,
                        to: SandboxStatus::Retiring,
                    },
                })
                .await
                .unwrap();
            }
            *gate.released.lock().unwrap() = true;
            gate.release.notify_one();
            let result = pending.await.unwrap();
            let stored = peer.get_workspace(&workspace.id).await.unwrap().unwrap();
            let events = peer.sandbox_events(&parent.id, 0, 20).await.unwrap();
            let ready_count = events
                .iter()
                .filter(|event| matches!(event.kind, SandboxEventKind::WorkspaceReady { .. }))
                .count();
            if final_status == SandboxStatus::Installing {
                result.unwrap();
                assert_eq!(stored.status, WorkspaceStatus::Ready);
                assert_eq!(ready_count, 1);
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(stored, claimed);
                assert_eq!(ready_count, 0);
            }
            assert!(!worker.run_one(&workspace.id, now).await.unwrap());
            let requests = harness.backend.requests().await;
            let calls = &requests[requests_before..];
            assert_eq!(
                calls
                    .iter()
                    .filter(|(_, r)| matches!(r, RequestFrame::WorkspaceSetup(_)))
                    .count(),
                usize::from(!prepared)
            );
            assert!(!calls
                .iter()
                .any(|(_, r)| matches!(r, RequestFrame::PrepareWorkspace(_))));
            assert_eq!(
                peer.get_sandbox(&parent.id).await.unwrap().unwrap().status,
                final_status
            );
            admin.database(&name).drop().await.unwrap();
        }
    }
}
