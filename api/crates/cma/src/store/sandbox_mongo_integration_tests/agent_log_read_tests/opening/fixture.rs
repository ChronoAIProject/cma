use super::*;

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: MongoSandboxStore,
    pub agent: Agent,
    pub entries: Vec<AgentLogEntry>,
    pub commands: Arc<Mutex<Vec<Document>>>,
}

impl Fixture {
    pub async fn new(messages: usize, gaps: usize, summary: bool) -> Self {
        Self::with_payload(messages, gaps, summary, 128).await
    }

    pub async fn with_payload(messages: usize, gaps: usize, summary: bool, width: usize) -> Self {
        let name = format!("cma_opening_{}", uuid::Uuid::new_v4().simple());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                observed.lock().unwrap().push(event.command);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let parent = sandbox("sbx_opening");
        store.create_sandbox(parent.clone()).await.unwrap();
        let (_, mut agent) = worktree_and_agent(&parent.id);
        agent.status = AgentStatus::Ready;
        agent.conversation.conversation_id = Some("opening".into());
        agent.conversation.runtime_generation = Some(1);
        store.create_agent(agent.clone()).await.unwrap();
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.creation_worktree_id().clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "opening".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 3,
                metadata: [("mode".into(), "review".into())].into(),
            },
        ];
        kinds.extend((0..messages).map(|index| AgentLogKind::AgentMessage {
            content: format!("answer {index} {}", "x".repeat(width)),
        }));
        kinds.extend((0..gaps).map(|_| AgentLogKind::ConversationGap { frames: 1 }));
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        if summary {
            kinds.push(AgentLogKind::UsageUpdated {
                input_tokens: 123,
                output_tokens: 45,
                context_used: 8,
                context_window: Some(200),
                turn_sequence: Some(3),
                turn_input_tokens: 23,
                turn_output_tokens: 5,
            });
        }
        let entries: Vec<_> = kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: index as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .collect();
        for chunk in entries.chunks(200) {
            store
                .agent_log
                .insert_many(chunk.iter().map(|row| log_document(row).unwrap()))
                .await
                .unwrap();
        }
        agent.conversation.last_seq = entries.len() as u64;
        agent.revision += 1;
        let mut fields = crate::store::agents::agent_replacement_fields(&agent).unwrap();
        fields.insert("last_seq", entries.len() as i64);
        store
            .agents
            .update_one(
                doc! {"_id":agent.id.as_str()},
                doc! {"$set":fields,"$unset":{INTEGRITY_FIELD:""}},
            )
            .await
            .unwrap();
        let store = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap()
            .with_bounded_transcript_reads(true)
            .with_bounded_usage_reads(true);
        let fixture = Self {
            client,
            name,
            store,
            agent,
            entries,
            commands,
        };
        fixture.install_index().await;
        fixture.commands.lock().unwrap().clear();
        fixture
    }

    pub async fn append_turn(&mut self, active: bool) {
        let queued = self.entries.len() as u64 + 1;
        let turn = queued + 2;
        let mut kinds = vec![
            AgentLogKind::OwnerMessage {
                content: "queued question".into(),
            },
            AgentLogKind::ModeChanged {
                mode: "idle".into(),
            },
            AgentLogKind::TurnStarted {
                turn_sequence: turn,
                origin_key: Some(format!("queued-message:{queued}")),
                model: Some("model".into()),
            },
            AgentLogKind::AgentMessage {
                content: "partial answer".into(),
            },
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: turn + 2,
                operation: layer2_core::sandbox::AgentOperationKind::Steer,
                state: layer2_core::sandbox::AgentOperationDelivery::DispatchFenced,
            },
            AgentLogKind::OwnerMessage {
                content: "steering".into(),
            },
            AgentLogKind::AgentMessage {
                content: "next answer".into(),
            },
        ];
        if !active {
            kinds.push(AgentLogKind::TurnInterrupted {
                turn_sequence: turn,
            });
        }
        let rows: Vec<_> = kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| AgentLogEntry {
                agent_id: self.agent.id.clone(),
                seq: queued + index as u64,
                appended_at: NOW.into(),
                kind,
            })
            .collect();
        self.store
            .agent_log
            .insert_many(rows.iter().map(|row| log_document(row).unwrap()))
            .await
            .unwrap();
        self.entries.extend(rows);
        self.agent.conversation.active_turn = active.then_some(turn);
        self.agent.conversation.last_seq = self.entries.len() as u64;
        self.agent.revision += 1;
        let mut fields = crate::store::agents::agent_replacement_fields(&self.agent).unwrap();
        fields.insert("last_seq", self.entries.len() as i64);
        self.store
            .agents
            .update_one(
                doc! {"_id":self.agent.id.as_str()},
                doc! {"$set":fields,"$unset":{INTEGRITY_FIELD:""}},
            )
            .await
            .unwrap();
        self.store = self
            .store
            .clone()
            .prepare_bounded_admission(std::slice::from_ref(&self.agent.id))
            .await
            .unwrap();
        self.commands.lock().unwrap().clear();
    }

    pub async fn install_index(&self) {
        let db = self.client.database(&self.name);
        let pending = crate::store::migrations::opening_index::inspect(&db)
            .await
            .unwrap();
        for (collection, index) in pending {
            db.run_command(doc! {"createIndexes":collection,"indexes":[index]})
                .await
                .unwrap();
        }
    }

    pub fn expected(&self) -> layer2_core::sandbox::AgentTranscript {
        let mut fold = layer2_core::sandbox::AgentTranscriptFold::default();
        for entry in &self.entries {
            fold.push(&self.agent, entry.clone()).unwrap();
        }
        fold.finish(&self.agent).unwrap()
    }

    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
