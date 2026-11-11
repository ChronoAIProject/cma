use super::*;
use crate::store::agents::agent_replacement_fields;
use mongodb::event::cmap::CmapEvent;

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: MongoSandboxStore,
    pub agent: Agent,
    pub entries: Vec<AgentLogEntry>,
    pub commands: Arc<Mutex<Vec<Document>>>,
    checkouts: Arc<AtomicU64>,
}

impl Fixture {
    pub async fn new(
        count: u64,
        turn: u64,
        next: Option<u64>,
        model: Option<&str>,
        bytes: usize,
    ) -> Self {
        assert!(turn >= 10 && turn + 3 < count);
        let name = format!("cma_response_window_{}", uuid::Uuid::new_v4().simple());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        let checkouts = Arc::new(AtomicU64::new(0));
        let observed = checkouts.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured.lock().unwrap().push(event.command);
            }
        }));
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut agent = agent_for_worker("agt_response_window", NOW);
        agent.status = AgentStatus::Ready;
        agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
        agent.conversation.conversation_id = Some("response-window-conversation".into());
        agent.conversation.runtime_generation = Some(1);
        store.create_agent(agent.clone()).await.unwrap();
        let entries: Vec<_> = (1..=count)
            .map(|seq| {
                let kind = if seq == 1 {
                    AgentLogKind::Created {
                        workspace_id: agent.workspace_id.clone(),
                        worktree_id: agent.creation_worktree_id().clone(),
                    }
                } else if seq == 2 {
                    AgentLogKind::ConversationOpened {
                        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                        runtime_generation: 1,
                    }
                } else if seq == 3 || seq == turn || next == Some(seq) {
                    AgentLogKind::TurnStarted {
                        turn_sequence: seq,
                        origin_key: None,
                        model: if seq == turn {
                            model.map(str::to_owned)
                        } else {
                            Some("earlier-model".into())
                        },
                    }
                } else if seq == 4 || seq == turn + 1 || next == seq.checked_sub(1) {
                    AgentLogKind::OwnerMessage {
                        content: "Question".into(),
                    }
                } else if seq == turn - 1 {
                    AgentLogKind::TurnCompleted { turn_sequence: 3 }
                } else if next == seq.checked_add(1) {
                    AgentLogKind::TurnCompleted {
                        turn_sequence: turn,
                    }
                } else if seq == count {
                    AgentLogKind::TurnCompleted {
                        turn_sequence: next.unwrap_or(turn),
                    }
                } else {
                    AgentLogKind::AgentMessage {
                        content: "x".repeat(if seq == turn + 2 { bytes } else { 4096 }),
                    }
                };
                AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                }
            })
            .collect();
        for chunk in entries.chunks(200) {
            store
                .agent_log
                .insert_many(chunk.iter().map(|entry| log_document(entry).unwrap()))
                .await
                .unwrap();
        }
        agent.conversation.last_seq = count;
        agent.revision += 1;
        let mut fields = agent_replacement_fields(&agent).unwrap();
        fields.insert("last_seq", count as i64);
        store
            .agents
            .update_one(doc! { "_id":agent.id.as_str() }, doc! { "$set":fields })
            .await
            .unwrap();
        let store = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap()
            .with_bounded_response_reads(true);
        Self {
            client,
            name,
            store,
            agent,
            entries,
            commands,
            checkouts,
        }
    }

    pub fn reset(&self) -> u64 {
        self.commands.lock().unwrap().clear();
        self.checkouts.load(Ordering::SeqCst)
    }
    pub fn used(&self, before: u64) -> u64 {
        self.checkouts.load(Ordering::SeqCst) - before
    }
    pub fn assert_read_only(&self) {
        for command in self.commands.lock().unwrap().iter() {
            assert!(![
                "insert",
                "update",
                "delete",
                "findAndModify",
                "abortTransaction",
                "commitTransaction",
                "startTransaction"
            ]
            .iter()
            .any(|key| command.contains_key(key)));
        }
    }
    pub fn assert_snapshot(&self) {
        self.assert_read_only();
        let commands = self.commands.lock().unwrap();
        let first = commands.iter().find(|c| c.contains_key("find")).unwrap();
        for command in commands
            .iter()
            .filter(|c| c.contains_key("find") || c.contains_key("getMore"))
        {
            assert_eq!(command.get("lsid"), first.get("lsid"));
            if command.contains_key("find") {
                assert!(command_uses_snapshot_read(command));
            }
        }
    }
    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
