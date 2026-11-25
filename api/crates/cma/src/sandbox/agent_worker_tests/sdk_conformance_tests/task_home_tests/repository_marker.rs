use super::repository_connection_tests::RepositoryRefusal;
use super::*;
use crate::sandbox::worker::SandboxWorker;
use mongodb::bson::{doc, Bson, Document};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::{options::ClientOptions, Client, Database};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_archive_refusal_before_allocation_marker_retires_original_sandbox() {
    run_marker_refusal(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_marker_recovers_a_timed_out_initial_renewal_without_another_create() {
    run_marker_refusal(true).await;
}

async fn run_marker_refusal(delay_initial_renewal: bool) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("cma::agent_lifecycle=info")
        .with_test_writer()
        .try_init();
    super::environment::initial_task_environment(
        Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
        Some(RepositoryRefusal {
            code: "repository_archive_unavailable",
            sibling: false,
            before_marker: true,
            delay_initial_renewal,
        }),
        false,
    )
    .await;
}

#[derive(Default)]
pub(super) struct MarkerPause {
    entered: tokio::sync::Notify,
    resume: (Mutex<bool>, Condvar),
    completed: tokio::sync::Notify,
    request: Mutex<Option<i32>>,
    delay_initial_renewal: bool,
    renewal_delayed: AtomicBool,
}

impl MarkerPause {
    pub(super) fn new(delay_initial_renewal: bool) -> Arc<Self> {
        Arc::new(Self {
            delay_initial_renewal,
            ..Self::default()
        })
    }

    pub(super) async fn client(self: &Arc<Self>, uri: &str) -> Client {
        let mut options = ClientOptions::parse(uri).await.unwrap();
        let marker = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            match event {
                CommandEvent::Started(event)
                    if marker.delay_initial_renewal
                        && is_lease_renewal(&event.command)
                        && !marker.renewal_delayed.swap(true, Ordering::SeqCst) =>
                {
                    // Cross the worker's five-second renewal deadline before
                    // any provisioning work can run. The late Mongo write may
                    // still renew the original token; discovery must wait for it.
                    tokio::task::block_in_place(|| std::thread::sleep(Duration::from_secs(6)));
                }
                CommandEvent::Started(event) if is_allocation_marker(&event.command) => {
                    *marker.request.lock().unwrap() = Some(event.request_id);
                    marker.entered.notify_one();
                    // The driver's asynchronous command handlers are detached.
                    // Hold this command before send while the other runtime thread drives failure.
                    tokio::task::block_in_place(|| {
                        let (resume, wake) = &marker.resume;
                        let (released, _) = wake
                            .wait_timeout_while(
                                resume.lock().unwrap(),
                                Duration::from_secs(60),
                                |released| !*released,
                            )
                            .unwrap();
                        assert!(*released, "resume the original allocation marker write");
                    });
                }
                CommandEvent::Succeeded(event)
                    if marker.request.lock().unwrap().as_ref() == Some(&event.request_id) =>
                {
                    assert_eq!(
                        event.reply.get_i32("n").unwrap(),
                        0,
                        "the old marker write must lose"
                    );
                    marker.completed.notify_one();
                }
                _ => {}
            }
        }));
        Client::with_options(options).unwrap()
    }

    pub(super) async fn fail_before_marker(
        &self,
        database: &Database,
        operation_id: &str,
        harness: &Harness<MongoSandboxStore>,
        worker: &SandboxWorker,
        code: &str,
        retry: &super::store_retry::StoreRetry,
    ) -> SandboxId {
        tokio::time::timeout(Duration::from_secs(15), self.entered.notified())
            .await
            .expect("pause the lifecycle worker after its actual allocation");
        let operations = database.collection::<Document>("agent_lifecycle_operations");
        let query = doc! {"_id": operation_id};
        let before = operations.find_one(query.clone()).await.unwrap().unwrap();
        if self.delay_initial_renewal {
            assert!(self.renewal_delayed.load(Ordering::SeqCst));
            assert_eq!(before.get_i64("attempts").unwrap(), 5);
        }
        assert_eq!(before.get("sandbox_id"), Some(&Bson::Null));
        assert_eq!(before.get("agent_id"), Some(&Bson::Null));

        let owner = SubjectId::new("owner");
        let sandboxes = harness
            .store
            .list_sandboxes(&owner, &PageRequest::default())
            .await
            .unwrap();
        let origin = layer2_core::sandbox::SandboxOrigin::Lifecycle {
            operation_id: operation_id.to_owned(),
        };
        let id = sandboxes
            .items
            .into_iter()
            .find(|sandbox| sandbox.origin == origin)
            .expect("find the original Sandbox before its operation marker")
            .id;
        let status = tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let current = harness.store.get_sandbox(&id).await.unwrap().unwrap();
                if matches!(
                    current.status,
                    SandboxStatus::Failed | SandboxStatus::Retiring
                ) {
                    break current.status;
                }
                if matches!(
                    current.work.step,
                    SandboxWorkStep::CreateProfileWorkspace(_)
                ) {
                    harness.backend.fail_next(
                        WireOperation::PrepareWorkspace,
                        code,
                        "fixture refusal",
                    );
                }
                retry.sandbox(harness, worker, &id).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;

        *self.resume.0.lock().unwrap() = true;
        self.resume.1.notify_one();
        tokio::time::timeout(Duration::from_secs(5), self.completed.notified())
            .await
            .unwrap();
        assert_eq!(status.unwrap(), SandboxStatus::Retiring);
        assert!(harness
            .store
            .get_sandbox(&id)
            .await
            .unwrap()
            .unwrap()
            .runtime
            .is_some());
        let after = operations.find_one(query).await.unwrap().unwrap();
        assert_eq!(after.get_str("status").ok(), Some("failed"));
        assert_eq!(after.get_str("sandbox_id").ok(), Some(id.as_str()));
        assert_eq!(after.get("agent_id"), Some(&Bson::Null));
        assert!(after.get_str("error").unwrap().starts_with(code));
        id
    }
}

fn is_allocation_marker(command: &Document) -> bool {
    command.get_str("update").ok() == Some("agent_lifecycle_operations")
        && command.get_array("updates").is_ok_and(|updates| {
            updates.iter().any(|update| {
                update
                    .as_document()
                    .and_then(|update| update.get_document("u").ok())
                    .and_then(|update| update.get_document("$set").ok())
                    .is_some_and(|fields| {
                        fields.get_str("sandbox_id").is_ok() && !fields.contains_key("status")
                    })
            })
        })
}

fn is_lease_renewal(command: &Document) -> bool {
    command.get_str("update").ok() == Some("agent_lifecycle_operations")
        && command.get_array("updates").is_ok_and(|updates| {
            updates.iter().any(|update| {
                update
                    .as_document()
                    .and_then(|update| update.get_array("u").ok())
                    .is_some_and(|pipeline| {
                        pipeline.iter().any(|stage| {
                            stage
                                .as_document()
                                .and_then(|stage| stage.get_document("$set").ok())
                                .is_some_and(|fields| {
                                    fields.contains_key("lease_expires_at")
                                        && !fields.contains_key("status")
                                })
                        })
                    })
            })
        })
}
