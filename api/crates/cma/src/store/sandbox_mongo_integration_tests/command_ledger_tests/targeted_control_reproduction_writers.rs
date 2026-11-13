use super::super::super::work_tests::live_request;
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Kind {
    Checkpoint,
    Admission,
    QueuedCancellation,
}

fn chunk() -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::from("x")).message_id(MessageId::new("quiet")),
    ))
}

pub(super) async fn attach_output(store: &MongoSandboxStore, agent: &Agent) -> Agent {
    // The first output confirms the dispatch and advances the public log.
    // Establish that once before measuring subsequent quiet checkpoint writes.
    DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk(),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

pub(super) struct Writer {
    kind: Kind,
    store: MongoSandboxStore,
    agent: Agent,
    parent: Sandbox,
    recorder: Arc<DurableAgentUpdateRecorder>,
    assembly: AgentMessageAssembly,
    cancellation: Option<QueuedCancellationReservation>,
}

impl Writer {
    pub(super) async fn new(
        kind: Kind,
        store: MongoSandboxStore,
        agent: Agent,
        template: &TargetedControlReservation,
    ) -> Self {
        let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let cancellation = if matches!(kind, Kind::QueuedCancellation) {
            let source = record(
                store
                    .reserve_command(live_request(&agent, &parent, "queued-writer-source"))
                    .await
                    .unwrap(),
            );
            let mut command = template.command.clone();
            command.envelope.command_id = AgentCommandId::generate();
            command.key_sha256 = command_key_digest("queued-writer-cancel").unwrap();
            let request = QueuedCancellationReservation {
                command,
                target: QueuedCancellationTarget {
                    response_id: source.receipt.response_id.unwrap(),
                    source_command_id: source.receipt.envelope.command_id,
                    expected_command_revision: source.revision,
                },
                observed_agent_revision: agent.revision,
                agent_created_at: agent.created_at.clone(),
            };
            store.cancel_queued_response(request.clone()).await.unwrap();
            Some(request)
        } else {
            None
        };
        Self {
            kind,
            recorder: DurableAgentUpdateRecorder::new(Arc::new(store.clone())),
            store,
            agent,
            parent,
            cancellation,
            assembly: AgentMessageAssembly::default(),
        }
    }

    pub(super) async fn tick(&mut self, index: u64) -> Result<(), &'static str> {
        match self.kind {
            Kind::Checkpoint => {
                let outcome = self
                    .recorder
                    .record_update(
                        &self.agent.id,
                        1,
                        &mut self.assembly,
                        chunk(),
                        layer2_core::now_rfc3339(),
                    )
                    .await
                    .map_err(|_| "checkpoint_failure")?;
                assert!(outcome.appended.is_empty());
            }
            Kind::Admission => {
                let mut request =
                    live_request(&self.agent, &self.parent, &format!("writer-{index}"));
                request.queue_capacity = MAX_COMMAND_QUEUE_CAPACITY;
                let accepted = record(
                    self.store
                        .reserve_command(request)
                        .await
                        .map_err(|error| error.code())?,
                );
                self.store
                    .reject_unclaimed_command(
                        &accepted.receipt.envelope.command_id,
                        accepted.revision,
                        CommandFailureCode::TargetTurnChanged,
                        layer2_core::now_rfc3339(),
                    )
                    .await
                    .map_err(|error| error.code())?;
            }
            Kind::QueuedCancellation => {
                // Replay is itself a resource-fencing write in the production
                // path; it does not manufacture additional queued work.
                self.store
                    .cancel_queued_response(self.cancellation.clone().unwrap())
                    .await
                    .map_err(|error| error.code())?;
            }
        }
        Ok(())
    }
}
