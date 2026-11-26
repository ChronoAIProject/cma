use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{AgentTitle, AgentTitleStore};
use mongodb::bson::{doc, Bson, Document};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::{options::ClientOptions, Client};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

#[derive(Clone, Copy)]
enum InterveningWrite {
    Title,
    Runtime,
}

struct Intervention {
    agent: AgentId,
    write: InterveningWrite,
}

struct WorkspaceAdvance {
    id: layer2_core::ids::WorkspaceId,
    transaction: usize,
}

struct RecoveryRead {
    workspace_reads_remaining: usize,
    claim_expires_at: SystemTime,
}

#[derive(Default)]
struct Trace {
    workspace_advance: Mutex<Option<WorkspaceAdvance>>,
    publish_ready_workspace: Mutex<Option<Workspace>>,
    deliver_initial_credential: Mutex<Option<layer2_core::SandboxId>>,
    recovery_read: Mutex<Option<RecoveryRead>>,
    expire_recovery_publication: Mutex<Option<SystemTime>>,
    repair_after_abort: Mutex<Option<AgentId>>,
    intervention: Mutex<Option<Intervention>>,
    replace_before_failure: Mutex<Option<(AgentId, layer3_adapter::hosted::InMemoryHostedBackend)>>,
    pending_fault: Mutex<Option<(AgentId, Fault)>>,
    prepared: Mutex<Option<Agent>>,
    bodies: AtomicUsize,
    commits: AtomicUsize,
}

struct Fixture {
    admin: Client,
    app: String,
    database: String,
    trace: Arc<Trace>,
    bootstrap: BootstrapHarness<MongoSandboxStore>,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("a disposable replica set is required");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app = format!("cma-lifecycle-publication-{suffix}");
        let database = format!("cma_lifecycle_publication_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let observer = Arc::new(MongoSandboxStore::open(&admin, &database).await.unwrap());
        let trace = Arc::new(Trace::default());
        let callback = trace.clone();
        let fault_admin = admin.clone();
        let fault_app = app.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Started(event) => {
                if event.command.get_str("find").ok() == Some("workspaces")
                    && !event.command.contains_key("autocommit")
                    && callback.bodies.load(Ordering::SeqCst) != 0
                {
                    let hold = callback.recovery_read.lock().unwrap().take_if(|read| {
                        read.workspace_reads_remaining -= 1;
                        read.workspace_reads_remaining == 0
                    });
                    if let Some(hold) = hold {
                        let remaining = hold
                            .claim_expires_at
                            .duration_since(SystemTime::now())
                            .expect("the recovery read must start with the original lease live");
                        std::thread::sleep(remaining + Duration::from_millis(100));
                    }
                }
                if event.command.get_str("find").ok() == Some("worktrees") {
                    let replacement = callback.replace_before_failure.lock().unwrap().clone();
                    if let Some((agent, backend)) = replacement {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                let sent = backend.requests().await.iter().any(|(_, request)| {
                                    matches!(
                                        request,
                                        layer3_adapter::wire::RequestFrame::AddWorktree(_)
                                    )
                                });
                                if sent
                                    && callback
                                        .replace_before_failure
                                        .lock()
                                        .unwrap()
                                        .take()
                                        .is_some()
                                {
                                    intervene(
                                        &observer,
                                        Intervention {
                                            agent,
                                            write: InterveningWrite::Runtime,
                                        },
                                    )
                                    .await;
                                }
                            });
                        });
                    }
                }
                if event.command.get_bool("startTransaction").ok() == Some(true) {
                    parent_interleaving::advance(&callback, &observer);
                }
                if event.command_name == "commitTransaction" {
                    callback.commits.fetch_add(1, Ordering::SeqCst);
                }
            }
            CommandEvent::Succeeded(event) if event.command_name == "commitTransaction" => {
                let pending = callback.pending_fault.lock().unwrap().take();
                if let Some((id, fault)) = pending {
                    // Admission of the native request committed. Arm only its later result.
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            let admitted = observer.get_agent(&id).await.unwrap().unwrap();
                            assert_eq!(admitted.work.cleanup_debt.len(), 1);
                            *callback.prepared.lock().unwrap() = Some(admitted);
                            callback.bodies.store(0, Ordering::SeqCst);
                            callback.commits.store(0, Ordering::SeqCst);
                            let (mode, data) = fault.options(&fault_app);
                            set_failpoint(&fault_admin, mode, data).await;
                        });
                    });
                }
            }
            CommandEvent::Failed(event)
                if event.command_name == "update"
                    && event
                        .failure
                        .contains_label(mongodb::error::TRANSIENT_TRANSACTION_ERROR) =>
            {
                let repair = callback.repair_after_abort.lock().unwrap().take();
                if let Some(id) = repair {
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current()
                            .block_on(lineage::repair_workspace_commit(&observer, &id));
                    });
                }
                let intervention = callback.intervention.lock().unwrap().take();
                if let Some(intervention) = intervention {
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current()
                            .block_on(intervene(&observer, intervention));
                    });
                }
            }
            _ => {}
        }));
        let client = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
        let bootstrap = BootstrapHarness::with_store(store, None).await;
        Self {
            admin,
            app,
            database,
            trace,
            bootstrap,
        }
    }

    async fn claim(&self, id: &AgentId) -> (Agent, SystemTime) {
        self.claim_for(id, Duration::from_secs(60)).await
    }

    async fn claim_for(&self, id: &AgentId, lease: Duration) -> (Agent, SystemTime) {
        let now = SystemTime::now();
        let agent = self
            .bootstrap
            .harness
            .store
            .claim_agent_work(
                id,
                "bootstrap-worker",
                rfc3339_at(now),
                rfc3339_at(now + lease),
            )
            .await
            .unwrap()
            .unwrap();
        (agent, now)
    }

    async fn fault(&self, mode: Bson, data: Document) {
        self.trace.bodies.store(0, Ordering::SeqCst);
        self.trace.commits.store(0, Ordering::SeqCst);
        set_failpoint(&self.admin, mode, data).await;
    }

    async fn transient(&self, mode: Bson) {
        self.fault(
            mode,
            doc! {"appName": &self.app, "failCommands": ["update"],
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"],
            },
        )
        .await;
    }

    async fn disable(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off",
            })
            .await
            .unwrap();
    }

    async fn cleanup(&self) {
        self.admin.database(&self.database).drop().await.unwrap();
    }
}

async fn intervene(store: &MongoSandboxStore, intervention: Intervention) {
    match intervention.write {
        InterveningWrite::Title => {
            let title = AgentTitle::new("Title saved during the aborted publication").unwrap();
            store
                .set_user_agent_title(&intervention.agent, &title, rfc3339_at(SystemTime::now()))
                .await
                .unwrap();
        }
        InterveningWrite::Runtime => {
            let agent = store.get_agent(&intervention.agent).await.unwrap().unwrap();
            let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
            let expected = sandbox.revision;
            let runtime = sandbox.runtime.as_mut().unwrap();
            runtime.runtime_id = "replacement-runtime".into();
            sandbox.lifecycle.runtime_generation_high_water += 1;
            runtime.provision_generation = Some(sandbox.lifecycle.runtime_generation_high_water);
            sandbox.revision += 1;
            store.save_sandbox(expected, sandbox).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn aborted_wait_publication_rebuilds_only_with_original_parent_authority() {
    for write in [InterveningWrite::Title, InterveningWrite::Runtime] {
        let fixture = Fixture::new().await;
        let h = &fixture.bootstrap.harness;
        let waiting = fixture.bootstrap.admit(None).await;
        fixture.bootstrap.ready().await;
        let (claimed, now) = fixture.claim(&waiting.id).await;
        let requests_before = h.backend.requests().await.len();
        *fixture.trace.intervention.lock().unwrap() = Some(Intervention {
            agent: claimed.id.clone(),
            write,
        });
        fixture
            .transient(Bson::Document(doc! {"times": 1_i32}))
            .await;

        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), now)
            .await;
        fixture.disable().await;
        assert!(fixture.trace.intervention.lock().unwrap().is_none());
        let agent = h.stored(&waiting.id).await;
        let logs = h.store.agent_log(&waiting.id, 0, 100).await.unwrap();
        let logged = logs
            .iter()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::WaitForWorkspace
                    }
                )
            })
            .count();
        assert_eq!(h.backend.requests().await.len(), requests_before);
        match write {
            InterveningWrite::Title => {
                result.unwrap();
                assert_eq!(agent.work.step, AgentWorkStep::AddWorktree);
                assert_eq!(
                    agent.title.as_deref(),
                    Some("Title saved during the aborted publication")
                );
                assert_eq!((logged, agent.work.attempts), (1, 0));
                assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 2);
            }
            InterveningWrite::Runtime => {
                assert!(matches!(result, Err(SandboxStoreError::Conflict)));
                assert_eq!(agent, claimed);
                assert_eq!(logged, 0);
                assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 1);
            }
        }
        fixture.cleanup().await;
    }
}

#[derive(Clone, Copy)]
enum Fault {
    KnownAbort,
    CommitAcknowledgement,
    Exhaustion,
}

async fn set_failpoint(client: &Client, mode: Bson, data: Document) {
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": mode, "data": data,
        })
        .await
        .unwrap();
}

impl Fault {
    fn options(self, app: &str) -> (Bson, Document) {
        match self {
            Self::CommitAcknowledgement => (
                doc! {"times": 5_i32}.into(),
                doc! {
                    "appName": app, "failCommands": ["commitTransaction"],
                    "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired",
                        "errmsg": "injected lost commit acknowledgement"},
                },
            ),
            Self::KnownAbort | Self::Exhaustion => {
                let mode = if matches!(self, Self::KnownAbort) {
                    doc! {"times": 1_i32}.into()
                } else {
                    "alwaysOn".into()
                };
                (
                    mode,
                    doc! {"appName": app, "failCommands": ["update"],
                        "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"],
                    },
                )
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn failure_publication_uses_one_native_result_after_abort_or_unknown_ack() {
    for fault in [
        Fault::KnownAbort,
        Fault::CommitAcknowledgement,
        Fault::Exhaustion,
    ] {
        let fixture = Fixture::new().await;
        let h = &fixture.bootstrap.harness;
        let waiting = fixture.bootstrap.admit(None).await;
        fixture.bootstrap.ready().await;
        assert!(h
            .worker
            .run_one(&waiting.id, SystemTime::now())
            .await
            .unwrap());
        let (claimed, now) = fixture.claim(&waiting.id).await;
        assert_eq!(claimed.work.step, AgentWorkStep::AddWorktree);
        let requests_before = h.backend.requests().await.len();
        h.backend.fail_next(
            WireOperation::AddWorktree,
            "runtime_mutation_outcome_unknown",
            "synthetic native worktree refusal",
        );
        if matches!(fault, Fault::KnownAbort) {
            *fixture.trace.intervention.lock().unwrap() = Some(Intervention {
                agent: claimed.id.clone(),
                write: InterveningWrite::Title,
            });
        }
        *fixture.trace.pending_fault.lock().unwrap() = Some((claimed.id.clone(), fault));
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), now)
            .await;
        fixture.disable().await;
        let prepared = fixture
            .trace
            .prepared
            .lock()
            .unwrap()
            .take()
            .expect("native intent must commit before arming the result publication fault");
        let agent = h.stored(&waiting.id).await;
        let logs = h.store.agent_log(&agent.id, 0, 100).await.unwrap();
        let failures = logs
            .iter()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    AgentLogKind::WorkStepFailed {
                        step: AgentWorkStep::AddWorktree,
                        ..
                    }
                )
            })
            .count();
        let requests = h.backend.requests().await;
        assert_eq!(
            requests
                .iter()
                .skip(requests_before)
                .filter(|(_, request)| {
                    matches!(request, layer3_adapter::wire::RequestFrame::AddWorktree(_))
                })
                .count(),
            1
        );

        match fault {
            Fault::KnownAbort | Fault::CommitAcknowledgement => {
                result.unwrap();
                assert_eq!(agent.status, AgentStatus::Failed);
                assert_eq!(
                    agent.failure.as_ref().unwrap().class,
                    "agent_runtime_rejected"
                );
                assert_eq!((failures, agent.work.attempts), (1, 0));
                if matches!(fault, Fault::KnownAbort) {
                    assert_eq!(
                        agent.title.as_deref(),
                        Some("Title saved during the aborted publication")
                    );
                    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 2);
                } else {
                    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 1);
                    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 5);
                }
            }
            Fault::Exhaustion => {
                assert!(matches!(result, Err(SandboxStoreError::Conflict)));
                assert_eq!(agent, prepared);
                assert_eq!(failures, 0);
                assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 5);
            }
        }
        fixture.cleanup().await;
    }
}

#[path = "lifecycle_publication/native_failure_tests.rs"]
mod native_failure;

#[path = "lifecycle_publication/parent_interleaving.rs"]
mod parent_interleaving;

#[path = "lifecycle_publication/workspace_binding.rs"]
mod workspace_binding;

#[path = "lifecycle_publication/lineage_tests.rs"]
mod lineage;
