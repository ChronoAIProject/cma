use super::*;

pub(super) async fn source(
    store: &MongoSandboxStore,
    closed: i64,
    dense: bool,
) -> (Agent, Vec<AgentLogEntry>) {
    let mut agent = agent();
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    let mut kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: Some("original-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "fixture".into(),
        },
    ];
    for _ in kinds.len()..(closed * 16 + 5) as usize {
        kinds.push(if dense {
            AgentLogKind::AgentMessage {
                content: "answer".into(),
            }
        } else {
            AgentLogKind::ModeChanged {
                mode: "x".repeat(512),
            }
        });
    }
    kinds.extend([
        AgentLogKind::AgentMessage {
            content: "final answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::Usage {
            input_tokens: 7,
            output_tokens: 11,
            context_used: 18,
            context_window: None,
        },
    ]);
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: i as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let (agent, entries) = super::super::fixtures::install(store, agent, entries, closed, 16).await;
    (agent, entries)
}

pub(super) async fn certify(client: &Client, db: &mongodb::Database, agent: &Agent, units: i64) {
    for _ in 0..units {
        assert_eq!(
            audit::run_unit(client, db, agent.id.as_str(), "response-reader", false)
                .await
                .unwrap(),
            "running"
        );
    }
    assert_eq!(
        audit::run_unit(client, db, agent.id.as_str(), "response-reader", false)
            .await
            .unwrap(),
        "complete"
    );
    assert_eq!(
        audit::install(client, db, agent.id.as_str(), "response-reader")
            .await
            .unwrap(),
        "installed"
    );
}

#[derive(Default)]
pub(super) struct Trace {
    pub enabled: std::sync::atomic::AtomicBool,
    pub checkouts: std::sync::atomic::AtomicU64,
    pub commands: std::sync::atomic::AtomicU64,
    pub bytes: std::sync::atomic::AtomicU64,
    pub writes: std::sync::atomic::AtomicU64,
    pub output_finds: std::sync::atomic::AtomicU64,
    pub evidence_get_mores: std::sync::atomic::AtomicU64,
}
impl Trace {
    pub fn reset(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        for counter in [
            &self.checkouts,
            &self.commands,
            &self.bytes,
            &self.writes,
            &self.output_finds,
            &self.evidence_get_mores,
        ] {
            counter.store(0, SeqCst);
        }
        self.enabled.store(true, SeqCst);
    }
    pub fn finish(&self) -> [u64; 4] {
        use std::sync::atomic::Ordering::SeqCst;
        self.enabled.store(false, SeqCst);
        [&self.checkouts, &self.commands, &self.bytes, &self.writes].map(|v| v.load(SeqCst))
    }

    pub fn output_commands(&self) -> [u64; 2] {
        use std::sync::atomic::Ordering::SeqCst;
        [
            self.output_finds.load(SeqCst),
            self.evidence_get_mores.load(SeqCst),
        ]
    }
}

pub(super) async fn measured_client() -> (Client, std::sync::Arc<Trace>) {
    use mongodb::{
        event::{cmap::CmapEvent, command::CommandEvent, EventHandler},
        options::ClientOptions,
    };
    use std::sync::{atomic::Ordering::SeqCst, Arc};
    let trace = Arc::new(Trace::default());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let seen = trace.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if seen.enabled.load(SeqCst) && matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            seen.checkouts.fetch_add(1, SeqCst);
        }
    }));
    let seen = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if !seen.enabled.load(SeqCst) {
            return;
        }
        match event {
            CommandEvent::Started(e) => {
                seen.commands.fetch_add(1, SeqCst);
                if e.command_name == "find"
                    && e.command.get_str("find").ok() == Some(EVIDENCE)
                    && e.command.get_str("hint").ok() == Some("agent_log_output_change_seek")
                {
                    seen.output_finds.fetch_add(1, SeqCst);
                }
                if e.command_name == "getMore"
                    && e.command.get_str("collection").ok() == Some(EVIDENCE)
                {
                    seen.evidence_get_mores.fetch_add(1, SeqCst);
                }
                if matches!(
                    e.command_name.as_str(),
                    "insert"
                        | "update"
                        | "delete"
                        | "findAndModify"
                        | "commitTransaction"
                        | "abortTransaction"
                ) {
                    seen.writes.fetch_add(1, SeqCst);
                }
            }
            CommandEvent::Succeeded(e) => {
                seen.bytes.fetch_add(
                    bson::serialize_to_vec(&e.reply).unwrap().len() as u64,
                    SeqCst,
                );
            }
            _ => {}
        }
    }));
    (Client::with_options(options).unwrap(), trace)
}
