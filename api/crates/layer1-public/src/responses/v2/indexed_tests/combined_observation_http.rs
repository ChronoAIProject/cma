use super::*;
use layer2_core::sandbox::store::AgentLogStore;

struct BoundCommands(Arc<InMemorySandboxStore>);
impl BoundCommands {
    fn bound(&self, mut command: AdmittedAgentCommand) -> AdmittedAgentCommand {
        command.status = AgentResponseStatus::InProgress;
        command.work.dispatch = Some(CommandDispatch {
            claim: CommandClaim {
                agent_id: command.receipt.envelope.agent_id.clone(),
                command_id: command.receipt.envelope.command_id.clone(),
                holder: "combined-test".into(),
                fence: 1,
            },
            operation_sequence: 1,
            turn_sequence: 3,
            runtime_generation: 1,
            fenced_at: AT.into(),
            delivery: layer2_core::sandbox::AgentOperationDelivery::DispatchFenced,
        });
        command
    }
}
#[async_trait]
impl AgentCommandStore for BoundCommands {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.0.reserve_command(request).await
    }
    async fn get_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        Ok(self.0.get_command(id).await?.map(|c| self.bound(c)))
    }
    async fn get_response_command(
        &self,
        id: &layer2_core::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        Ok(self
            .0
            .get_response_command(id)
            .await?
            .map(|c| self.bound(c)))
    }
}

struct Legacy {
    indexed: Arc<Source>,
    bounded: bool,
    switch: AtomicBool,
}
#[async_trait]
impl AgentResponseLogStore for Legacy {
    async fn agent_output_reader(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<Box<dyn AgentOutputReader + '_>>, SandboxStoreError> {
        if self.switch.load(Ordering::SeqCst) {
            self.indexed.agent_output_reader(agent, turn).await
        } else {
            Ok(None)
        }
    }
    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        Ok(self.bounded.then(|| AgentResponseLogWindow {
            through: agent.conversation.last_seq,
            turn_sequence: turn,
            entries: self
                .indexed
                .data
                .rows
                .range(turn.unwrap()..)
                .map(|(_, row)| row.clone())
                .collect(),
        }))
    }
}

async fn state(f: &IndexedFixture) -> (crate::responses::v2::ResponsesV2State, CommandReceiptView) {
    for row in f.source.data.rows.values() {
        f.store
            .append_agent_log(&f.agent.id, row.appended_at.clone(), row.kind.clone())
            .await
            .unwrap();
    }
    let commands = AgentCommandService::new(
        Arc::new(BoundCommands(f.store.clone())),
        f.access.clone(),
        Arc::new(Authority),
    );
    let state = crate::responses::v2::ResponsesV2State::new(
        f.state.agent_page_origin.clone(),
        commands,
        f.reads.clone(),
    );
    let router = crate::responses::v2::router(state.clone());
    let response = router
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some("combined-http"),
            Some(json!({"conversation":"agt_indexed","input":"Hello","stream":true})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    let held = f
        .reads
        .authorize_event_stream(&context("owner-a"), None, &f.agent.id)
        .await
        .unwrap();
    let reading = state
        .commands
        .response_for_stream(&held, record.receipt.response_id.as_ref().unwrap())
        .await
        .unwrap();
    (state, reading.command.receipt)
}

#[tokio::test]
async fn public_retrieve_preserves_indexed_bounded_and_complete_legacy_bytes() {
    let mut kinds = beginning();
    kinds.extend([
        message("answer"),
        tool("call", "running"),
        output("call", "output"),
        update("call", "completed"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    let (state, receipt) = state(&f).await;
    authorize(&f, true).await;
    let path = format!(
        "/api/v2/responses/{}",
        receipt.response_id.unwrap().as_str()
    );
    for caller in ["owner-a", "reader"] {
        let mut expected = None;
        for mode in ["indexed", "bounded", "complete"] {
            let mut state = state.clone();
            if mode != "indexed" {
                state.reads = f.reads.with_response_logs(Some(Arc::new(Legacy {
                    indexed: f.source.clone(),
                    bounded: mode == "bounded",
                    switch: AtomicBool::new(false),
                })));
            }
            let (status, body) = read_json(
                &crate::responses::v2::router(state),
                Method::GET,
                &path,
                caller,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{mode}: {body}");
            let bytes = serde_json::to_vec(&body).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&bytes, expected, "{caller} {mode}");
            } else {
                expected = Some(bytes);
            }
        }
    }
}

#[tokio::test]
async fn public_retrieve_keeps_indexed_pagination_refusal() {
    let mut kinds = beginning();
    kinds.extend((0..12).map(|_| message(&"x".repeat(400_000))));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let (state, receipt) = state(&f).await;
    let path = format!(
        "/api/v2/responses/{}",
        receipt.response_id.unwrap().as_str()
    );
    let (status, body) = read_json(
        &crate::responses::v2::router(state),
        Method::GET,
        &path,
        "owner-a",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "response_pagination_required");
}

#[tokio::test]
async fn legacy_stream_refuses_an_indexed_format_change_at_its_original_cursor() {
    let mut kinds = beginning();
    kinds.push(message("partial answer"));
    let f = make(kinds).await;
    let (mut state, receipt) = state(&f).await;
    let source = Arc::new(Legacy {
        indexed: f.source.clone(),
        bounded: true,
        switch: AtomicBool::new(false),
    });
    state.reads = f.reads.with_response_logs(Some(source.clone()));
    let held = state
        .reads
        .authorize_event_stream(&context("owner-a"), None, &f.agent.id)
        .await
        .unwrap();
    let response = crate::responses::v2::stream::open(state, held, receipt, None, Some(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    source.switch.store(true, Ordering::SeqCst);
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        axum::body::to_bytes(response.into_body(), 100_000),
    )
    .await
    .unwrap()
    .unwrap();
    let frames = events(&bytes);
    assert_eq!(
        frames.last().unwrap()["code"],
        "response_observation_unavailable"
    );
    assert!(!frames
        .iter()
        .any(|frame| frame["type"] == "response.completed"));
}
