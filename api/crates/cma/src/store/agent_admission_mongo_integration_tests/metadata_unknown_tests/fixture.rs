use super::*;
use layer2_core::sandbox::Agent;
use mongodb::Collection;

#[derive(Clone, Copy, Debug)]
pub(in crate::store) enum Operation {
    Channel,
    Provenance,
    Reuse,
    Snapshot,
    History,
}

#[derive(Debug)]
pub(in crate::store) enum Outcome {
    Channel(Box<ChannelUsageEvent>),
    Provenance(ProvenanceBackfillOutcome),
    Reuse(Box<AgentCreation>),
    Read,
}

pub(in crate::store) struct Fixture {
    pub(in crate::store) store: MongoSandboxStore,
    pub(in crate::store) independent: MongoSandboxStore,
    admin: Client,
    database: String,
    app: String,
    commands: Arc<Mutex<Vec<(String, Document)>>>,
    observations: Arc<Mutex<Vec<Document>>>,
    parent: Sandbox,
    pub(in crate::store) original: AgentCreation,
    pub(in crate::store) provenance: AgentCreationProvenance,
    pub(in crate::store) channel: ChannelUsageMutation,
    pub(in crate::store) reuse: AgentAdmission,
}

impl Fixture {
    pub(in crate::store) async fn new(operation: Operation) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a disposable replica set");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app = format!("m4-metadata-{suffix}");
        let database = format!("m4_metadata_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let receipts = observations.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let Some(row) = observations::observe(&event) {
                receipts.lock().unwrap().push(row);
            }
            if let CommandEvent::Started(event) = event {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &database)
            .await
            .unwrap()
            .with_renewable_work(true);
        let independent = MongoSandboxStore::open(&admin, &database)
            .await
            .unwrap()
            .with_renewable_work(true);
        let parent = sandbox(&suffix);
        independent.create_sandbox(parent.clone()).await.unwrap();
        let workspace = workspace(&parent, &suffix);
        independent
            .create_workspace(workspace.clone())
            .await
            .unwrap();
        let mut request = admission(&workspace, &suffix);
        request.creation_key = format!("manual:{}", request.agent_id);
        let original = independent
            .admit_agent(request.clone())
            .await
            .unwrap()
            .creation()
            .clone();
        let snapshot = independent
            .agent_channel_usage_snapshot(&original.agent.id)
            .await
            .unwrap();
        let mut provenance = snapshot.state.creation.clone();
        let channel = ChannelUsageMutation {
            policy_fence: snapshot.provenance.resource.fence(),
            creation: snapshot.state.creation,
            expected_revision: snapshot.state.revision,
            key_sha256: layer2_core::profiles::content_digest(&"channel-exact-key").unwrap(),
            actor: ResourceActor::Human {
                subject: SubjectId::new("owner"),
            },
            change: ChannelUsageChange::Attach {
                binding: ChannelBindingReference {
                    binding_id: format!("binding-{suffix}"),
                    bridge_id: "bridge-one".into(),
                    partition_id: "partition-one".into(),
                    generation: 1,
                    previous_binding_id: None,
                },
            },
            reason: ChannelBindingChangeReason::Attachment,
            at: "2026-09-05T00:01:00Z".into(),
        };
        let mut reuse = request;
        reuse.agent_id = AgentId::generate();
        reuse.worktree_id = WorktreeId::generate();
        reuse.creation_key = format!("trigger-reuse-{suffix}");
        reuse.origin = AgentOrigin::Trigger {
            trigger_id: "trigger".into(),
            launch_id: suffix,
        };
        reuse.first_message = Some(
            FirstMessageRecord::new(reuse.creation_key.clone(), "Exact retained prompt", NOW)
                .unwrap(),
        );
        if matches!(operation, Operation::Provenance) {
            independent
                .agent_creation_provenance
                .delete_one(doc! {"_id":original.agent.id.as_str()})
                .await
                .unwrap();
            provenance = independent
                .agent_provenance_snapshot(&original.agent.id)
                .await
                .unwrap()
                .creation_provenance(&original.agent.id)
                .unwrap();
        }
        Self {
            store,
            independent,
            admin,
            database,
            app,
            commands,
            observations,
            parent,
            original,
            provenance,
            channel,
            reuse,
        }
    }

    pub(in crate::store) async fn claim(&self) -> Sandbox {
        self.independent
            .claim_sandbox_work(&self.parent.id, "metadata-worker", now(), deadline())
            .await
            .unwrap()
            .expect("fixture Sandbox work must be claimable")
    }

    pub(in crate::store) async fn run(
        &self,
        operation: Operation,
    ) -> Result<Outcome, SandboxStoreError> {
        match operation {
            Operation::Channel => self
                .store
                .record_agent_channel_usage(self.channel.clone())
                .await
                .map(|event| Outcome::Channel(Box::new(event))),
            Operation::Provenance => self
                .store
                .backfill_agent_provenance(&self.original.agent.id)
                .await
                .map(Outcome::Provenance),
            Operation::Reuse => self
                .store
                .admit_agent_reusing_profile(self.reuse.clone())
                .await
                .map(|result| Outcome::Reuse(Box::new(result.creation().clone()))),
            Operation::Snapshot => self
                .store
                .agent_channel_usage_snapshot(&self.original.agent.id)
                .await
                .map(|_| Outcome::Read),
            Operation::History => self
                .store
                .agent_channel_usage_history(
                    &self.original.agent.id,
                    &PageRequest::new(10, None).unwrap(),
                )
                .await
                .map(|_| Outcome::Read),
        }
    }

    pub(in crate::store) async fn fail(
        &self,
        command: &str,
        times: i32,
        committed: Option<bool>,
        dual_label: bool,
    ) {
        self.commands.lock().unwrap().clear();
        self.observations.lock().unwrap().clear();
        let mut data = doc! {"failCommands":[command], "appName":&self.app};
        match committed {
            Some(true) => {
                data.insert("writeConcernError", doc! {"code":50_i32, "codeName":"MaxTimeMSExpired", "errmsg":"lost metadata commit acknowledgement"});
            }
            Some(false) => {
                data.insert("errorCode", 50_i32);
                data.insert("errorLabels", vec!["UnknownTransactionCommitResult"]);
            }
            None if dual_label => {
                data.insert("errorCode", 11000_i32);
                // DuplicateKey requires extra info; otherwise the server emits
                // a parser error and never exercises duplicate classification.
                data.insert(
                    "errorExtraInfo",
                    doc! {
                        "keyPattern":{"_id":1_i32},"keyValue":{"_id":"synthetic-duplicate"}
                    },
                );
                data.insert("keyPattern", doc! {"_id":1_i32});
                data.insert("keyValue", doc! {"_id":"synthetic-duplicate"});
                data.insert(
                    "errorLabels",
                    vec![
                        "UnknownTransactionCommitResult",
                        "TransientTransactionError",
                    ],
                );
            }
            None => {
                data.insert("errorCode", 112_i32);
                data.insert("errorLabels", vec!["TransientTransactionError"]);
            }
        }
        self.admin
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand", "mode":{"times":times}, "data":data},
            )
            .await
            .unwrap();
    }

    pub(in crate::store) async fn clear(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .expect("disable failpoint before observations and assertions");
    }

    pub(super) async fn clear_and_settle_commit(&self) {
        self.clear().await;
        crate::store::test_visibility::majority_barrier(&self.admin, &self.database).await;
    }

    pub(super) async fn fail_insert(&self, code: i32, labels: &[&str]) {
        self.reset_observations();
        let mut data = doc! {
            "appName":&self.app, "failCommands":["insert"],
            "errorCode":code, "errorLabels":labels.to_vec(),
        };
        if code == 11000 {
            data.insert(
                "errorExtraInfo",
                doc! {
                    "keyPattern":{"_id":1_i32},"keyValue":{"_id":"synthetic-duplicate"}
                },
            );
            data.insert("keyPattern", doc! {"_id":1_i32});
            data.insert("keyValue", doc! {"_id":"synthetic-duplicate"});
        }
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1_i32}, "data":data,
            })
            .await
            .unwrap();
    }

    pub(in crate::store) fn commands(&self) -> Vec<(String, Document)> {
        self.commands.lock().unwrap().clone()
    }

    pub(in crate::store) fn observations(&self) -> Vec<Document> {
        self.observations.lock().unwrap().clone()
    }

    // Check durable facts independently of the method's result, so a failed
    // reconciliation cannot hide whether the commit failpoint applied writes.
    pub(in crate::store) fn assert_durable_receipt(
        &self,
        operation: Operation,
        state: &[Vec<Document>],
    ) {
        match operation {
            Operation::Channel => {
                assert_eq!(state[5].len(), 1, "committed channel receipt absent");
                assert_eq!(state[6].len(), 1, "committed channel summary absent");
                let event: ChannelUsageEvent =
                    crate::store::sandbox_mongo::read_record(&state[5][0]).unwrap();
                assert_eq!(self.channel.replay(event.clone()).unwrap(), event);
                assert_eq!(event.at, self.channel.at);
                let summary: StoredAgentChannelUsage =
                    crate::store::sandbox_mongo::read_record(&state[6][0]).unwrap();
                summary.validate_latest(&event).unwrap();
            }
            Operation::Provenance => {
                assert_eq!(state[4].len(), 1, "committed provenance receipt absent");
                assert_eq!(
                    crate::store::agent_provenance::read(&state[4][0]).unwrap(),
                    self.provenance
                );
            }
            Operation::Reuse => {
                assert_eq!(state[7].len(), 1, "committed reuse receipt absent");
                assert_eq!(
                    state[7][0].get_str("fingerprint").unwrap(),
                    self.reuse.fingerprint().unwrap()
                );
            }
            _ => panic!("expected a metadata write"),
        }
    }

    pub(in crate::store) async fn state(&self) -> Vec<Vec<Document>> {
        use futures_util::TryStreamExt;
        let store = &self.independent;
        let rows: [&Collection<Document>; 8] = [
            &store.sandboxes,
            &store.workspaces,
            &store.agents,
            &store.agent_log,
            &store.agent_creation_provenance,
            &store.channel_usage_events,
            &store.agent_channel_usage,
            &store.agent_reuse_bindings,
        ];
        let mut result = Vec::new();
        for collection in rows {
            result.push(
                collection
                    .find(doc! {})
                    .sort(doc! {"_id":1})
                    .await
                    .unwrap()
                    .try_collect()
                    .await
                    .unwrap(),
            );
        }
        result
    }

    pub(in crate::store) async fn drop_database(&self) {
        // Refused commits can leave a server transaction open after the driver
        // marks its session committed. Clean only the fixture's exact sessions,
        // after the method and durable-state observations have completed.
        let sessions = self
            .commands()
            .into_iter()
            .filter_map(|(_, command)| {
                command
                    .get_bool("startTransaction")
                    .ok()
                    .filter(|start| *start)?;
                command.get_document("lsid").ok().cloned()
            })
            .collect::<Vec<_>>();
        if !sessions.is_empty() {
            self.admin
                .database("admin")
                .run_command(doc! {"killSessions":sessions})
                .await
                .expect("release only this fixture's transactions");
        }
        self.admin.database(&self.database).drop().await.unwrap();
    }

    pub(in crate::store) fn reset_observations(&self) {
        self.commands.lock().unwrap().clear();
        self.observations.lock().unwrap().clear();
    }

    pub(in crate::store) async fn snapshot_time(&self) -> mongodb::bson::Timestamp {
        let mut session = self.independent.client.start_session().await.unwrap();
        self.independent
            .agents
            .find_one(doc! {"_id":self.original.agent.id.as_str()})
            .session(&mut session)
            .await
            .unwrap();
        session
            .operation_time()
            .expect("a real read supplies an operation time")
    }

    pub(in crate::store) fn assert_committed(
        &self,
        operation: Operation,
        outcome: Outcome,
        state: Vec<Vec<Document>>,
    ) {
        match (operation, outcome) {
            (Operation::Channel, Outcome::Channel(event)) => {
                let event = *event;
                assert_eq!(self.channel.replay(event.clone()).unwrap(), event);
                assert_eq!(event.at, self.channel.at);
                assert_eq!(state[5].len(), 1);
                assert_eq!(state[6].len(), 1);
                let retained: ChannelUsageEvent =
                    crate::store::sandbox_mongo::read_record(&state[5][0]).unwrap();
                assert_eq!(retained, event);
            }
            (Operation::Provenance, Outcome::Provenance(outcome)) => {
                assert!(matches!(
                    outcome,
                    ProvenanceBackfillOutcome::Recorded | ProvenanceBackfillOutcome::Retained
                ));
                assert_eq!(state[4].len(), 1);
                assert_eq!(
                    crate::store::agent_provenance::read(&state[4][0]).unwrap(),
                    self.provenance
                );
            }
            (Operation::Reuse, Outcome::Reuse(creation)) => {
                let mut expected = self.original.clone();
                self.reuse
                    .initialize_reused_agent(&mut expected.agent, false)
                    .unwrap();
                assert_eq!(*creation, expected);
                assert_eq!(state[2].len(), 1);
                let agent: Agent = crate::store::sandbox_mongo::read_record(&state[2][0]).unwrap();
                assert_eq!(agent, expected.agent);
                assert_eq!(state[3].len(), 2);
                assert_eq!(state[7].len(), 1);
                assert_eq!(
                    state[7][0].get_str("fingerprint").unwrap(),
                    self.reuse.fingerprint().unwrap()
                );
            }
            (_, outcome) => panic!("unexpected {operation:?} outcome {outcome:?}"),
        }
    }
}

pub(in crate::store) fn now() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now())
}
pub(in crate::store) fn deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(300))
}
