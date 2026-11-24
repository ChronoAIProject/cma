use super::*;
use layer2_core::sandbox::store::{
    AgentProvenanceStore, AgentStore, ClaimedWork, SandboxWorkerStore, WorkAuthority,
};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

#[derive(Clone)]
struct Fault {
    command: String,
    collection: Option<String>,
    data: Document,
    times: i32,
}

struct AfterAbort {
    collection: String,
    filter: Document,
    update: Document,
}

pub(super) struct Fixture {
    pub admin: mongodb::Client,
    pub independent: MongoSandboxStore,
    pub store: Arc<MongoSandboxStore>,
    pub source: Arc<MongoAgentAccessToolSource>,
    pub state: layer2_core::sandbox::store::AgentProvenanceSnapshot,
    pub open: AgentOpenRequest,
    pub runtime: RuntimeHandle,
    database: String,
    fault: Arc<Mutex<Option<Fault>>>,
    fired: Arc<AtomicBool>,
    commands: Arc<Mutex<Vec<Document>>>,
    commit_replies: Arc<Mutex<Vec<Document>>>,
    after_abort: Arc<Mutex<Option<AfterAbort>>>,
    changed_after_abort: Arc<AtomicBool>,
}

impl Fixture {
    pub async fn new(ready: bool) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("disposable failpoint-enabled replica set required");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let database = format!("cma_access_retry_{suffix}");
        let app = format!("cma-access-retry-{suffix}");
        let admin = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let independent = MongoSandboxStore::open(&admin, &database)
            .await
            .unwrap()
            .with_renewable_work(true);
        let mut plan = crate::store::native_prepared::tests::plan();
        plan.warm.reaches = vec![ServiceId::new("original-service")];
        plan.validate().unwrap();
        let (mut agent, _, _, _) =
            crate::store::native_prepared::tests::open_ready_fixture::hierarchy_with_plan(
                &independent,
                plan,
            )
            .await;
        agent.work.lease.as_mut().unwrap().expires_at = layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        );
        agent.revision += 1;
        independent
            .save_agent(agent.revision - 1, agent.clone())
            .await
            .unwrap();
        if ready {
            super::super::tests::publish(&independent, &agent.id).await;
        }
        let state = independent
            .agent_provenance_snapshot(&agent.id)
            .await
            .unwrap();
        let open = super::super::tests::request(&state);
        let runtime = RuntimeHandle(
            state
                .resource
                .lineage
                .sandbox
                .runtime
                .as_ref()
                .unwrap()
                .runtime_id
                .clone(),
        );
        let fault = Arc::new(Mutex::new(None::<Fault>));
        let pending = fault.clone();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        let commit_replies = Arc::new(Mutex::new(Vec::new()));
        let replies = commit_replies.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let injected = fired.clone();
        let after_abort = Arc::new(Mutex::new(None::<AfterAbort>));
        let pending_change = after_abort.clone();
        let changed_after_abort = Arc::new(AtomicBool::new(false));
        let changed = changed_after_abort.clone();
        let change_database = database.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            match &event {
                CommandEvent::Succeeded(event) if event.command_name == "commitTransaction" => {
                    replies.lock().unwrap().push(event.reply.clone());
                }
                _ => {}
            }
            // This runs after the first apply error and explicit abort, before
            // the next transaction takes its snapshot, even if Mongo reports
            // that the failed transaction had already been aborted.
            if injected.load(Ordering::SeqCst)
                && matches!(&event, CommandEvent::Started(event) if event.command.get_bool("startTransaction").ok() == Some(true))
            {
                let change = pending_change.lock().unwrap().take();
                if let Some(change) = change {
                    let uri = uri.clone();
                    let database = change_database.clone();
                    std::thread::spawn(move || {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap()
                            .block_on(async {
                                let admin = mongodb::Client::with_uri_str(uri).await.unwrap();
                                let result = admin
                                    .database(&database)
                                    .collection::<Document>(&change.collection)
                                    .update_one(change.filter, change.update)
                                    .await
                                    .unwrap();
                                assert_eq!(result.matched_count, 1);
                            });
                    })
                    .join()
                    .unwrap();
                    changed.store(true, Ordering::SeqCst);
                }
            }
            let CommandEvent::Started(event) = event else {
                return;
            };
            observed.lock().unwrap().push(event.command.clone());
            let mut pending = pending.lock().unwrap();
            if !pending.as_ref().is_some_and(|fault| {
                event.command_name == fault.command
                    && fault.collection.as_ref().is_none_or(|name| {
                        event.command.get_str(&fault.command).ok() == Some(name.as_str())
                    })
            }) {
                return;
            }
            let fault = pending.take().unwrap();
            drop(pending);
            let uri = uri.clone();
            let app = app.clone();
            // Block this exact command before send; independent admin setup is
            // complete before the real prepared/authority read reaches Mongo.
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                    let admin = mongodb::Client::with_uri_str(uri).await.unwrap();
                    let mut data = fault.data;
                    data.insert("failCommands", vec![fault.command]);
                    data.insert("appName", app);
                    admin.database("admin").run_command(doc! {
                        "configureFailPoint":"failCommand", "mode":{"times":fault.times}, "data":data,
                    }).await.unwrap();
                });
            }).join().unwrap();
            injected.store(true, Ordering::SeqCst);
        }));
        let client = mongodb::Client::with_options(options).unwrap();
        let store = Arc::new(
            MongoSandboxStore::open(&client, &database)
                .await
                .unwrap()
                .with_renewable_work(true),
        );
        let source = MongoAgentAccessToolSource::new(
            store.clone(),
            "https://control.example.test",
            &super::super::tests::key(9),
        )
        .unwrap();
        Self {
            admin,
            independent,
            store,
            source,
            state,
            open,
            runtime,
            database,
            fault,
            fired,
            commands,
            commit_replies,
            after_abort,
            changed_after_abort,
        }
    }

    pub fn change_after_abort(&self, collection: &str, filter: Document, update: Document) {
        self.changed_after_abort.store(false, Ordering::SeqCst);
        *self.after_abort.lock().unwrap() = Some(AfterAbort {
            collection: collection.into(),
            filter,
            update,
        });
    }

    pub fn changed_after_abort(&self) -> bool {
        self.changed_after_abort.load(Ordering::SeqCst)
    }

    pub fn arm(&self, command: &str, collection: Option<&str>, times: i32, data: Document) {
        self.commands.lock().unwrap().clear();
        self.commit_replies.lock().unwrap().clear();
        self.fired.store(false, Ordering::SeqCst);
        *self.fault.lock().unwrap() = Some(Fault {
            command: command.into(),
            collection: collection.map(str::to_owned),
            data,
            times,
        });
    }

    pub fn abort(&self, command: &str, collection: Option<&str>, times: i32) {
        self.arm(
            command,
            collection,
            times,
            doc! {"errorCode":112_i32,"errorLabels":["TransientTransactionError"]},
        );
    }

    pub async fn clear(&self) -> (bool, Vec<Document>) {
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .expect("disable failpoint before observations and assertions");
        (
            self.fired.load(Ordering::SeqCst),
            self.commands.lock().unwrap().clone(),
        )
    }

    /// failCommand's writeConcernError skips the majority wait. Carry the
    /// commit timestamp into independent majority reads before asserting state.
    pub async fn committed_read_session(&self, commands: &[Document]) -> mongodb::ClientSession {
        let commits = commands
            .iter()
            .filter(|command| command.contains_key("commitTransaction"))
            .collect::<Vec<_>>();
        assert_eq!(
            commits.len(),
            5,
            "bounded original-transaction commit retries"
        );
        assert!(commits.iter().all(|command| {
            command.get("lsid") == commits[0].get("lsid")
                && command.get("txnNumber") == commits[0].get("txnNumber")
        }));
        let at = {
            let replies = self.commit_replies.lock().unwrap();
            assert_eq!(replies.len(), 5, "every commit command executed");
            assert!(replies.iter().all(|reply| {
                reply
                    .get_document("writeConcernError")
                    .unwrap()
                    .get_i32("code")
                    .unwrap()
                    == 50
            }));
            replies
                .iter()
                .map(|reply| reply.get_timestamp("operationTime").unwrap())
                .max()
                .unwrap()
        };
        let mut session = self
            .admin
            .start_session()
            .causal_consistency(true)
            .await
            .unwrap();
        session.advance_operation_time(at);
        session
    }

    pub async fn cleanup(&self) {
        self.admin.database(&self.database).drop().await.unwrap();
    }

    pub fn open_fence(&self) -> u64 {
        self.state
            .resource
            .lineage
            .agent
            .as_ref()
            .unwrap()
            .work
            .lease
            .as_ref()
            .unwrap()
            .fence
    }

    pub fn original_services(&self) -> Vec<ServiceId> {
        self.state
            .resource
            .lineage
            .sandbox
            .credential
            .effective_reaches()
    }

    pub fn requester(&self) -> AgentAccessRequester {
        let agent = self.state.resource.lineage.agent.as_ref().unwrap();
        AgentAccessRequester {
            agent_id: agent.id.clone(),
            owner: agent.owner.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            conversation: agent.conversation.conversation_id.clone().unwrap(),
            runtime: self.state.resource.lineage.sandbox.runtime.clone().unwrap(),
            creation: self.state.creation.clone().unwrap(),
        }
    }

    pub async fn with_work<T>(&self, work: impl std::future::Future<Output = T>) -> T {
        let claimed = self
            .independent
            .claim_sandbox_work(
                &self.state.resource.lineage.sandbox.id,
                "access-parent-worker",
                layer2_core::now_rfc3339(),
                deadline(),
            )
            .await
            .unwrap()
            .unwrap();
        WorkAuthority::new(
            ClaimedWork::Sandbox(Box::new(claimed.clone())),
            tokio::time::Instant::now(),
        )
        .run(
            async { Ok(crate::store::work_publication::scope(&self.store, work).await) },
            || async {
                self.independent
                    .renew_sandbox_authority(&claimed, &layer2_core::now_rfc3339(), &deadline())
                    .await
            },
        )
        .await
        .expect("fixture retains live original parent work authority")
    }

    pub async fn owner_save(
        &self,
        expected: Vec<ServiceId>,
        key: &str,
    ) -> std::result::Result<AgentAccessRecord, AgentAccessError> {
        let requester = self.requester();
        self.store
            .replace_agent_service_access(
                &requester.owner,
                &requester.agent_id,
                vec![ServiceId::new("github")],
                expected,
                &[],
                AgentAuthorizationRevision {
                    replay_principal: None,
                    authority_id: "personal:retry-owner".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                key,
            )
            .await
    }
}

fn deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(15))
}

pub(super) fn assert_starts(commands: &[Document], count: usize) {
    let starts: Vec<_> = commands
        .iter()
        .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
        .collect();
    assert_eq!(
        starts.len(),
        count,
        "retry must begin a fresh bounded transaction"
    );
    for (index, command) in starts.iter().enumerate() {
        for earlier in &starts[..index] {
            assert!(
                command.get_document("lsid").unwrap() != earlier.get_document("lsid").unwrap()
                    || command.get_i64("txnNumber").unwrap()
                        != earlier.get_i64("txnNumber").unwrap()
            );
        }
    }
}
