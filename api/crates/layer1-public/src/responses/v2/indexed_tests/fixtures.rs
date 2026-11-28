use super::*;

pub(super) const AT: &str = "2026-09-05T00:00:00Z";

#[derive(Default)]
pub(super) struct Counts {
    pub probes: AtomicUsize,
    pub points: AtomicUsize,
    pub opens: AtomicUsize,
    pub source_pages: AtomicUsize,
    pub source_rows: AtomicUsize,
    pub fail: AtomicBool,
    pub stall: AtomicBool,
    pub fail_points: AtomicBool,
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}
#[derive(Clone)]
pub(super) struct Data {
    pub metadata: AgentOutputMetadata,
    pub rows: BTreeMap<u64, AgentLogEntry>,
    pub changes: Vec<AgentOutputChange>,
    pub tools: BTreeMap<String, AgentToolOutputState>,
}
pub(super) struct Source {
    pub data: Arc<Data>,
    pub counts: Arc<Counts>,
}
struct Reader {
    metadata: AgentOutputMetadata,
    data: Arc<Data>,
    counts: Arc<Counts>,
}

#[async_trait]
impl AgentResponseLogStore for Source {
    async fn agent_output_available(&self, _: &Agent) -> Result<bool, SandboxStoreError> {
        Ok(true)
    }
    async fn agent_output_reader(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<Box<dyn AgentOutputReader + '_>>, SandboxStoreError> {
        self.counts.opens.fetch_add(1, Ordering::SeqCst);
        if self.counts.fail.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Corrupt(
                "missing certified evidence".into(),
            ));
        }
        let mut metadata = self.data.metadata.clone();
        if turn.is_some() {
            assert_eq!(metadata.agent_id, agent.id);
        }
        if turn.is_none() {
            metadata.turn = None;
            metadata.index = None;
            metadata.agent_id = agent.id.clone();
            metadata.through = agent.conversation.last_seq;
            metadata.created_at = agent.created_at.clone();
            metadata.initial_model = agent.profile.content.model.clone();
            metadata.runtime_generation = agent.conversation.runtime_generation;
            metadata.active_turn = agent.conversation.active_turn;
        }
        Ok(Some(Box::new(Reader {
            metadata,
            data: self.data.clone(),
            counts: self.counts.clone(),
        })))
    }
    async fn agent_response_log_window(
        &self,
        _: &Agent,
        _: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        panic!("segmented source must never select the lifetime fallback")
    }
}
#[async_trait]
impl AgentOutputReader for Reader {
    fn metadata(&self) -> &AgentOutputMetadata {
        &self.metadata
    }
    async fn changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<AgentOutputChange>, SandboxStoreError> {
        assert!((1..=8).contains(&limit));
        self.counts.probes.fetch_add(1, Ordering::SeqCst);
        let from = usize::try_from(from).map_err(|_| SandboxStoreError::InvalidPage)?;
        if from > self.data.changes.len() {
            return Err(SandboxStoreError::InvalidPage);
        }
        Ok(self
            .data
            .changes
            .iter()
            .skip(from)
            .take(usize::from(limit))
            .cloned()
            .collect())
    }
    async fn tool(&mut self, call: &str) -> Result<AgentToolOutputState, SandboxStoreError> {
        self.data
            .tools
            .get(call)
            .cloned()
            .ok_or(SandboxStoreError::NotFound)
    }
    async fn points(&mut self, seqs: &[u64]) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        assert!(seqs.len() <= 32);
        self.counts.points.fetch_add(seqs.len(), Ordering::SeqCst);
        if self.counts.stall.load(Ordering::SeqCst) {
            self.counts.entered.notify_one();
            self.counts.release.notified().await;
        }
        if self.counts.fail_points.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Corrupt("private output fixture".into()));
        }
        seqs.iter()
            .map(|s| {
                self.data
                    .rows
                    .get(s)
                    .cloned()
                    .ok_or(SandboxStoreError::NotFound)
            })
            .collect()
    }
    async fn source_page(
        &mut self,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        assert!((1..=32).contains(&limit));
        self.counts.source_pages.fetch_add(1, Ordering::SeqCst);
        let rows: Vec<_> = self
            .data
            .rows
            .range(after + 1..=self.metadata.index.as_ref().unwrap().through_sequence)
            .take(usize::from(limit))
            .map(|(_, row)| row.clone())
            .collect();
        self.counts
            .source_rows
            .fetch_add(rows.len(), Ordering::SeqCst);
        Ok(rows)
    }
}

pub(super) struct IndexedFixture {
    pub state: crate::AgentConversationState,
    pub agent: Agent,
    pub reads: Arc<AgentReadService>,
    pub access: Arc<ResourceAccessService>,
    pub store: Arc<InMemorySandboxStore>,
    pub source: Arc<Source>,
    pub reading: AgentResponseReading,
    pub baseline: AgentEventStreamSnapshot,
}

pub(super) fn start() -> AgentLogKind {
    AgentLogKind::TurnStarted {
        turn_sequence: 3,
        origin_key: None,
        model: Some("execution-model".into()),
    }
}
pub(super) fn beginning() -> Vec<AgentLogKind> {
    vec![
        start(),
        AgentLogKind::OwnerMessage {
            content: "question".into(),
        },
    ]
}
pub(super) fn message(text: &str) -> AgentLogKind {
    AgentLogKind::AgentMessage {
        content: text.into(),
    }
}
pub(super) fn tool(call: &str, status: &str) -> AgentLogKind {
    AgentLogKind::ToolCallStarted {
        call_id: call.into(),
        title: "tool".into(),
        kind: "execute".into(),
        status: status.into(),
    }
}
pub(super) fn output(call: &str, text: &str) -> AgentLogKind {
    AgentLogKind::ToolOutputReported {
        call_id: call.into(),
        output: layer2_core::sandbox::RecordedToolOutput {
            text: text.into(),
            truncated: false,
        },
    }
}
pub(super) fn update(call: &str, status: &str) -> AgentLogKind {
    AgentLogKind::ToolCallUpdated {
        call_id: call.into(),
        title: None,
        status: status.into(),
        output: None,
    }
}

pub(super) async fn make(kinds: Vec<AgentLogKind>) -> IndexedFixture {
    make_turn(kinds, 3).await
}

pub(super) async fn make_turn(mut kinds: Vec<AgentLogKind>, target: u64) -> IndexedFixture {
    let (mut state, store) = crate::rest::agents_tests::conversation_fixture().await;
    let mut agent = crate::rest::agents_tests::seeded_agent(
        "agt_indexed",
        AgentStatus::Ready,
        kinds.len() as u64 + 2,
        None,
    );
    agent.profile.content.model = Some("creation-model".into());
    kinds.insert(
        0,
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
    );
    kinds.insert(
        1,
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    );
    let rows: Vec<_> = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: i as u64 + 1,
            appended_at: AT.into(),
            kind,
        })
        .collect();
    let mut fold = AgentLogProjectionState::default();
    let mut facts = None;
    let mut index = None;
    let mut changes = vec![];
    let mut tools = BTreeMap::<String, AgentToolOutputState>::new();
    let mut inside = false;
    for row in &rows {
        for change in fold.observe(&agent, row, None).unwrap() {
            if let AgentLogEvidenceChange::Turn(turn) = change {
                if turn.turn_sequence == target {
                    facts = Some(*turn);
                }
            }
        }
        if matches!(row.kind, AgentLogKind::TurnStarted { .. }) {
            inside = row.seq == target;
            if inside {
                index = Some(AgentTurnOutputIndex::start(row).unwrap());
            }
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(index) = &mut index {
            let call = match &row.kind {
                AgentLogKind::ToolCallStarted { call_id, .. }
                | AgentLogKind::ToolCallUpdated { call_id, .. }
                | AgentLogKind::ToolOutputReported { call_id, .. } => Some(call_id),
                _ => None,
            };
            let lookup = match call {
                None => AgentOutputTool::None,
                Some(call) => tools
                    .get(call)
                    .map_or(AgentOutputTool::Absent, AgentOutputTool::Stored),
            };
            let delta = index.observe(row, lookup).unwrap();
            if let Some(change) = delta.change {
                changes.push(change);
            }
            if let Some(tool) = delta.tool {
                tools.insert(tool.call_id.clone(), tool);
            }
        }
    }
    agent.conversation.active_turn = fold.active.as_ref().map(|t| t.turn_sequence);
    agent.status = if agent.conversation.active_turn.is_some() {
        AgentStatus::Working
    } else {
        AgentStatus::Ready
    };
    fold.validate_aggregate(&agent).unwrap();
    let facts = facts.unwrap();
    let metadata = AgentOutputMetadata {
        agent_id: agent.id.clone(),
        through: agent.conversation.last_seq,
        created_at: agent.created_at.clone(),
        runtime_generation: agent.conversation.runtime_generation,
        active_turn: agent.conversation.active_turn,
        initial_model: agent.profile.content.model.clone(),
        index,
        turn: Some(AgentOutputTurn {
            legacy_model_known: facts.legacy_model_known,
            usage_sequence: facts.response_usage_sequence,
            terminal_sequence: facts.end_sequence,
            prewrite_sequence: facts.prewrite_failure_sequence,
            facts,
        }),
    };
    store
        .create_worktree(crate::rest::agents_tests::seeded_worktree(&agent))
        .await
        .unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    let baseline = AgentEventStreamSnapshot {
        source_generation: None,
        id: agent.id.clone(),
        created_at: agent.created_at.clone(),
        model: agent.selected_model(),
        initial_model: agent.profile.content.model.clone(),
        runtime_generation: agent.conversation.runtime_generation,
        active_turn: agent.conversation.active_turn,
        head: agent.conversation.last_seq,
        history: rows
            .iter()
            .map(|r| AgentVisibleLogEntry::from_entry(r.clone(), true))
            .collect(),
    };
    let data = Arc::new(Data {
        metadata,
        rows: rows.into_iter().map(|r| (r.seq, r)).collect(),
        changes,
        tools,
    });
    let source = Arc::new(Source {
        data,
        counts: Arc::new(Counts::default()),
    });
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(Authority),
    ));
    let reads = state
        .reads
        .with_resource_access(access.clone())
        .with_response_logs(Some(source.clone()));
    let reading = AgentResponseReading {
        deferred_failure_detail: None,
        metadata: BTreeMap::new(),
        command: CommandStatusView {
            receipt: CommandReceiptView {
                command_id: layer2_core::ids::AgentCommandId("cmd_indexed".into()),
                response_id: Some(layer2_core::ids::AgentResponseId("resp_indexed".into())),
                agent_id: agent.id.clone(),
                admitted_at: AT.into(),
                agent_order: 1,
                event_cursor: 0,
            },
            status: AgentResponseStatus::InProgress,
            queue_deadline: AT.into(),
            turn_sequence: source.data.metadata.index.as_ref().map(|i| i.turn_sequence),
            observed_cursor: None,
            delivery: None,
            failure: None,
        },
    };
    state.reads = reads.clone();
    IndexedFixture {
        state,
        agent,
        reads,
        access,
        store,
        source,
        reading,
        baseline,
    }
}

impl IndexedFixture {
    pub async fn purge(&self) {
        use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
        let mut parent = self
            .store
            .get_sandbox(&self.agent.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        parent.status = layer2_core::sandbox::SandboxStatus::Retired;
        parent.retired_at = Some(AT.into());
        parent.purge_after = Some(AT.into());
        parent.revision += 1;
        self.store.save_sandbox(revision, parent).await.unwrap();
        self.store
            .purge_sandbox(&self.agent.sandbox_id, revision + 1, AT)
            .await
            .unwrap();
    }

    pub fn mutate(&mut self, mutate: impl FnOnce(&mut Data)) {
        let mut data = (*self.source.data).clone();
        mutate(&mut data);
        self.source = Arc::new(Source {
            data: Arc::new(data),
            counts: self.source.counts.clone(),
        });
        self.reads = self.reads.with_response_logs(Some(self.source.clone()));
    }

    pub async fn observe<T: Send, F>(&self, operation: F) -> Result<T, AgentReadError>
    where
        F: Send
            + for<'a, 'b> FnOnce(
                &'a mut AgentOutputObservation<'b>,
            ) -> Pin<
                Box<dyn Future<Output = Result<T, AgentReadError>> + Send + 'a>,
            >,
    {
        self.reads
            .with_owned_output_observation(
                &context("owner-a").caller,
                &self.agent.id,
                self.reading.command.turn_sequence.unwrap(),
                operation,
            )
            .await?
            .ok_or(AgentReadError::NotFound)
    }
    pub fn golden(&self) -> crate::responses::v2::V2ResponseObject {
        projection::project(
            &crate::agent_page::AgentPageOrigin::test(),
            self.reading.clone(),
            &self.baseline,
        )
        .unwrap()
    }
}
