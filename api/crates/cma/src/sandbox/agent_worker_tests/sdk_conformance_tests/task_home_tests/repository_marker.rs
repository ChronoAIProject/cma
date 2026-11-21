use super::repository_connection_tests::RepositoryRefusal;
use super::*;
use crate::sandbox::worker::SandboxWorker;
use mongodb::bson::{doc, Bson, Document};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::{options::ClientOptions, Client, Database};
use std::sync::{Condvar, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_archive_refusal_before_allocation_marker_retires_original_sandbox() {
    super::environment::initial_task_environment(
        Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
        Some(RepositoryRefusal {
            code: "repository_archive_unavailable",
            sibling: false,
            before_marker: true,
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
}

impl MarkerPause {
    pub(super) async fn client(self: &Arc<Self>, uri: &str) -> Client {
        let mut options = ClientOptions::parse(uri).await.unwrap();
        let marker = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            match event {
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
    ) -> SandboxId {
        tokio::time::timeout(Duration::from_secs(15), self.entered.notified())
            .await
            .expect("pause the lifecycle worker after its actual allocation");
        let operations = database.collection::<Document>("agent_lifecycle_operations");
        let query = doc! {"_id": operation_id};
        let before = operations.find_one(query.clone()).await.unwrap().unwrap();
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
        let retry = super::store_retry::StoreRetry::default();
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
                retry.should_retry(worker.run_one(&id, SystemTime::now()).await);
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
