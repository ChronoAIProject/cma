use super::*;
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};
use std::sync::{Arc, Mutex};

type Observer = Arc<Mutex<Option<Box<dyn FnMut(&CommandEvent) + Send>>>>;

pub(super) struct Fixture {
    pub(super) admin: Client,
    pub(super) store: MongoSandboxStore,
    pub(super) independent: MongoSandboxStore,
    pub(super) name: String,
    pub(super) app: String,
    commands: Arc<Mutex<Vec<Document>>>,
    pub(super) events: Arc<Mutex<Vec<CommandEvent>>>,
    pub(super) observer: Observer,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a disposable replica set");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app = format!("m4-work-fence-{suffix}");
        let name = format!("m4_work_fence_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&commands);
        let events = Arc::new(Mutex::new(Vec::new()));
        let replies = events.clone();
        let observer: Observer = Arc::default();
        let callback = observer.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event: CommandEvent| {
            replies.lock().unwrap().push(event.clone());
            if let CommandEvent::Failed(failure) = &event {
                if failure.command_name == "createIndexes" {
                    let code = match failure.failure.kind.as_ref() {
                        mongodb::error::ErrorKind::Command(error) => Some(error.code),
                        _ => None,
                    };
                    eprintln!("fixture_index_command_failed code={code:?}");
                }
            }
            if let Some(callback) = callback.lock().unwrap().as_mut() {
                callback(&event);
            }
            if let CommandEvent::Started(event) = event {
                observed.lock().unwrap().push(event.command);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_renewable_work(true);
        let independent = MongoSandboxStore::open(&admin, &name)
            .await
            .unwrap()
            .with_renewable_work(true);
        Self {
            admin,
            store,
            independent,
            name,
            app,
            commands,
            events,
            observer,
        }
    }

    pub(super) async fn fail(&self, command: &str, times: i32, unknown_commit: bool) {
        self.reset_trace();
        let mut data = doc! {"failCommands":[command], "appName":&self.app};
        if unknown_commit {
            data.insert(
                "writeConcernError",
                doc! {
                    "code":50_i32, "codeName":"MaxTimeMSExpired",
                    "errmsg":"ambiguous work-authority publication commit",
                },
            );
        } else {
            data.insert("errorCode", 112_i32);
            data.insert("errorLabels", vec!["TransientTransactionError"]);
        }
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":times}, "data":data,
            })
            .await
            .unwrap();
    }

    pub(super) async fn clear(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off",
            })
            .await
            .expect("disable failpoint before observations and assertions");
    }

    pub(super) async fn drop_database(&self) {
        let sessions = self
            .starts()
            .iter()
            .filter_map(|command| command.get_document("lsid").ok().cloned())
            .collect::<Vec<_>>();
        if !sessions.is_empty() {
            self.admin
                .database("admin")
                .run_command(doc! {"killSessions":sessions})
                .await
                .unwrap();
        }
        self.admin.database(&self.name).drop().await.unwrap();
    }

    pub(super) fn reset_trace(&self) {
        self.commands.lock().unwrap().clear();
        self.events.lock().unwrap().clear();
    }

    pub(super) fn starts(&self) -> Vec<Document> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
            .cloned()
            .collect()
    }

    pub(super) async fn phase(&self) -> (Sandbox, Vec<Agent>, SandboxLifecycleMutation) {
        let (claimed, agents) = claimed_pause_fixture(&self.independent, "work_fence").await;
        self.independent
            .publish_sandbox_checkpoint(checkpoint_mutation(
                &claimed,
                &agents,
                checkpoint_for_agents(&agents, 1),
            ))
            .await
            .unwrap();
        let claimed = self
            .independent
            .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), work_lease_deadline())
            .await
            .unwrap()
            .unwrap();
        let mutation = pause_mutation(&claimed, &agents);
        (claimed, agents, mutation)
    }

    pub(super) async fn observations(
        &self,
        claimed: &Sandbox,
        agents: &[Agent],
    ) -> (Option<Sandbox>, Vec<Agent>, Vec<usize>, usize) {
        let sandbox = self.independent.get_sandbox(&claimed.id).await.unwrap();
        let mut current = Vec::new();
        let mut logs = Vec::new();
        for agent in agents {
            current.push(
                self.independent
                    .get_agent(&agent.id)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            logs.push(
                self.independent
                    .agent_log(&agent.id, 0, 10)
                    .await
                    .unwrap()
                    .len(),
            );
        }
        let events = self
            .independent
            .sandbox_events(&claimed.id, 0, 20)
            .await
            .unwrap()
            .len();
        (sandbox, current, logs, events)
    }
}

pub(super) fn now() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now())
}

fn pause_mutation(claimed: &Sandbox, agents: &[Agent]) -> SandboxLifecycleMutation {
    let mut next = claimed.clone();
    let Some(SandboxLifecycleOperation::Pause { phase, .. }) = next.lifecycle.operation.as_mut()
    else {
        panic!("fixture must be pausing")
    };
    *phase = SandboxPausePhase::TeardownRuntime;
    let lease = claimed.work.lease.as_ref().unwrap();
    next.work
        .advance(
            &lease.holder,
            lease.fence,
            SandboxWorkStep::RunLifecycleOperation,
            MINUTE.into(),
        )
        .unwrap();
    next.revision += 1;
    next.updated_at = MINUTE.into();
    let mut events = Vec::new();
    let agents = agents
        .iter()
        .map(|current| {
            let mut next = current.clone();
            next.transition(AgentStatus::Paused, MINUTE.into(), None)
                .unwrap();
            next.conversation.last_seq += 1;
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: current.status,
                to: AgentStatus::Paused,
            });
            SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent: next,
                logs: vec![AgentLogKind::StatusChanged {
                    from: current.status,
                    to: AgentStatus::Paused,
                }],
            }
        })
        .collect();
    events.push(SandboxEventKind::WorkStepCompleted {
        step: SandboxWorkStep::RunLifecycleOperation,
    });
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(claimed),
        sandbox: next,
        agents,
        agent_scope: SandboxLifecycleAgentScope::AllLive,
        appended_at: MINUTE.into(),
        events,
    }
}
