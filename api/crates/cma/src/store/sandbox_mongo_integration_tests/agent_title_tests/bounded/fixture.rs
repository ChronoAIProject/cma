use super::*;
use crate::store::agent_log_integrity::INTEGRITY_FIELD;
use crate::store::agents::agent_replacement_fields;
use layer2_core::sandbox::{AgentTitleTranscript, FirstMessagePurpose, FirstMessageRecord};
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
    pub async fn new(count: u64, task_end: u64, setup: bool, owner_bytes: usize) -> Self {
        assert!(task_end >= 6 && task_end < count);
        let name = format!("cma_title_prefix_{}", uuid::Uuid::new_v4().simple());
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
        let mut agent = agent_for_worker("agt_title_prefix", NOW);
        agent.status = AgentStatus::Ready;
        agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
        agent.conversation.conversation_id = Some("title-prefix-conversation".into());
        agent.conversation.runtime_generation = Some(1);
        if setup {
            let mut first = FirstMessageRecord::new("setup", "Prepare workspace", NOW).unwrap();
            first.purpose = Some(FirstMessagePurpose::Setup);
            first.fence_dispatch_for(3, 3, 1, NOW.into()).unwrap();
            first.complete(NOW.into()).unwrap();
            agent.conversation.first_message = Some(first);
        }
        store.create_agent(agent.clone()).await.unwrap();
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.creation_worktree_id().clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
            },
        ];
        if setup {
            kinds.extend(completed_turn("Setup instruction", "Setup done", 3));
        }
        let task_start = kinds.len() as u64 + 1;
        kinds.extend([
            AgentLogKind::TurnStarted {
                turn_sequence: task_start,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "a".repeat(owner_bytes),
            },
        ]);
        kinds.resize(
            task_end as usize - 1,
            AgentLogKind::AgentMessage {
                content: "First task answer".into(),
            },
        );
        kinds.push(AgentLogKind::TurnCompleted {
            turn_sequence: task_start,
        });
        let later_start = kinds.len() as u64 + 1;
        kinds.extend([
            AgentLogKind::TurnStarted {
                turn_sequence: later_start,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Later task".into(),
            },
        ]);
        kinds.resize(
            count as usize - 1,
            AgentLogKind::AgentMessage {
                content: "x".repeat(4096),
            },
        );
        kinds.push(AgentLogKind::TurnCompleted {
            turn_sequence: later_start,
        });
        let entries = kinds
            .into_iter()
            .enumerate()
            .map(|(offset, kind)| AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: offset as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .collect::<Vec<_>>();
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
            .update_one(
                doc! { "_id": agent.id.as_str() },
                doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
            )
            .await
            .unwrap();
        let store = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap();
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

    pub fn expected(&self) -> Option<AgentTitleTranscript> {
        AgentTitleTranscript::from_agent_log(&self.agent, &self.entries)
    }

    pub fn assert_snapshot_read_only(&self) {
        let commands = self.commands.lock().unwrap();
        let first = commands
            .iter()
            .find(|command| command.contains_key("find"))
            .unwrap();
        for command in commands.iter() {
            assert!(![
                "update",
                "insert",
                "delete",
                "findAndModify",
                "abortTransaction",
                "commitTransaction"
            ]
            .iter()
            .any(|key| command.contains_key(key)));
            if command.contains_key("find") {
                assert!(command_uses_snapshot_read(command));
                assert_eq!(command.get("lsid"), first.get("lsid"));
            }
        }
    }

    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
