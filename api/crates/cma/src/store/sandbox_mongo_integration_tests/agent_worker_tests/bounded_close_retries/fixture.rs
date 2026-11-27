use super::*;
use mongodb::event::cmap::CmapEvent;
use mongodb::event::command::CommandEvent;

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: MongoSandboxStore,
    pub agent: Agent,
    pub checkouts: Arc<AtomicU64>,
    pub commands: Arc<AtomicU64>,
    pub attempts: Arc<AtomicU64>,
    pub updates: Arc<AtomicU64>,
    pub complete_reads: Arc<AtomicU64>,
    pub ambiguous_commits: Arc<AtomicU64>,
    pub app: String,
}

impl Fixture {
    pub async fn new(history: u64) -> Self {
        let name = format!("cma_close_retry_{}", uuid::Uuid::new_v4().simple());
        let app = name.clone();
        let checkouts = Arc::new(AtomicU64::new(0));
        let captured = checkouts.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(app.clone());
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
                captured.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let complete_reads = Arc::new(AtomicU64::new(0));
        let ambiguous_commits = Arc::new(AtomicU64::new(0));
        let commands = Arc::new(AtomicU64::new(0));
        let attempts = Arc::new(AtomicU64::new(0));
        let updates = Arc::new(AtomicU64::new(0));
        let update_count = updates.clone();
        let command_count = commands.clone();
        let attempt_count = attempts.clone();
        let complete = complete_reads.clone();
        let ambiguous = ambiguous_commits.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = &event {
                command_count.fetch_add(1, Ordering::SeqCst);
                if event.command_name == "update" {
                    update_count.fetch_add(1, Ordering::SeqCst);
                }
                if event.command.get_bool("startTransaction").unwrap_or(false) {
                    attempt_count.fetch_add(1, Ordering::SeqCst);
                }
            }
            match event {
                CommandEvent::Started(event)
                    if event.command.get_str("find").ok() == Some(AGENT_LOG)
                        && event.command.get_document("filter").is_ok_and(|filter| {
                            filter.len() == 1 && filter.contains_key("agent_id")
                        })
                        && event.command.get_document("sort").ok() == Some(&doc! { "seq": 1 }) =>
                {
                    complete.fetch_add(1, Ordering::SeqCst);
                }
                CommandEvent::Succeeded(event)
                    if event.command_name == "commitTransaction"
                        && event
                            .reply
                            .get_document("writeConcernError")
                            .is_ok_and(|error| error.get_i32("code").ok() == Some(50)) =>
                {
                    ambiguous.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut agent = ready_conversation_fixture(&store, "bounded_close").await;
        store.agent_log.delete_many(doc! {}).await.unwrap();
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.creation_worktree_id().clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "fixture task".into(),
            },
        ];
        kinds.resize(
            history as usize - 2,
            AgentLogKind::AgentMessage {
                content: "x".repeat(4096),
            },
        );
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        for (batch, kinds) in kinds.chunks(200).enumerate() {
            let rows = kinds.iter().enumerate().map(|(offset, kind)| {
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: (batch * 200 + offset + 1) as u64,
                    appended_at: NOW.into(),
                    kind: kind.clone(),
                })
                .unwrap()
            });
            store.agent_log.insert_many(rows).await.unwrap();
        }
        agent.conversation.last_seq = history - 1;
        agent.work.schedule(AgentWorkStep::None, NOW.into());
        agent.revision += 1;
        let mut fields = agent_replacement_fields(&agent).unwrap();
        fields.insert("last_seq", (history - 1) as i64);
        store
            .agents
            .update_one(
                doc! { "_id": agent.id.as_str() },
                doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
            )
            .await
            .unwrap();
        store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason: AgentStopReason::Owner,
                requested_at: MINUTE.into(),
            })
            .await
            .unwrap();
        // FenceInteraction publication checks the lease against the MongoDB clock.
        let claim_time = std::time::SystemTime::now();
        let fenced = store
            .claim_agent_work(
                &agent.id,
                "worker",
                layer2_core::rfc3339_at(claim_time),
                layer2_core::rfc3339_at(claim_time + std::time::Duration::from_secs(30)),
            )
            .await
            .unwrap()
            .unwrap();
        store.commit_agent_work(advance(&fenced)).await.unwrap();
        let store = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap()
            .with_bounded_close_retries(true);
        let agent = claim(&store, &agent.id, MINUTE).await;
        assert_eq!(agent.conversation.last_seq, history);
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert!(parse_integrity(&row).unwrap().unwrap().semantic.valid);
        Self {
            client,
            name,
            store,
            agent,
            checkouts,
            commands,
            attempts,
            updates,
            complete_reads,
            ambiguous_commits,
            app,
        }
    }

    pub async fn row(&self) -> Document {
        self.store
            .agents
            .find_one(doc! { "_id": self.agent.id.as_str() })
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn reset(&self, row: Document) {
        self.store.agent_log.delete_many(doc! { "agent_id": self.agent.id.as_str(), "seq": { "$gt": self.agent.conversation.last_seq as i64 } }).await.unwrap();
        self.store
            .agents
            .replace_one(doc! { "_id": self.agent.id.as_str() }, row)
            .await
            .unwrap();
    }

    pub async fn change(&self, update: Document) {
        self.store
            .agents
            .update_one(doc! { "_id": self.agent.id.as_str() }, update)
            .await
            .unwrap();
    }

    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

pub(super) async fn claim(store: &MongoSandboxStore, id: &AgentId, at: &str) -> Agent {
    store
        .claim_agent_work(id, "worker", at.into(), "2026-09-05T00:10:00Z".into())
        .await
        .unwrap()
        .unwrap()
}

pub(super) fn retry(agent: &Agent) -> AgentWorkMutation {
    let fence = agent.work.lease.as_ref().unwrap().fence;
    let mut next = agent.clone();
    next.work
        .retry("worker", fence, TWO_MINUTES.into())
        .unwrap();
    next.updated_at = MINUTE.into();
    next.revision += 1;
    next.conversation.last_seq += 1;
    AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: "worker".into(),
        fence,
        agent: next,
        worktree: None,
        appended_at: MINUTE.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::CloseProcess,
            failure: TypedFailure {
                class: "runtime_unavailable".into(),
                detail: "retry".into(),
            },
        }],
    }
}

fn advance(agent: &Agent) -> AgentWorkMutation {
    let mut mutation = retry(agent);
    mutation.agent = agent.clone();
    mutation
        .agent
        .work
        .advance(
            "worker",
            mutation.fence,
            AgentWorkStep::CloseProcess,
            MINUTE.into(),
        )
        .unwrap();
    mutation.agent.revision += 1;
    mutation.agent.updated_at = MINUTE.into();
    mutation.agent.conversation.last_seq += 1;
    mutation.agent_logs = vec![AgentLogKind::WorkStepCompleted {
        step: agent.work.step,
    }];
    mutation
}
