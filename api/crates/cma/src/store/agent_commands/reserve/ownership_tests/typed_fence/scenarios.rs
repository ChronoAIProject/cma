use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Kind {
    Ordinary,
    Pair,
    Targeted,
    Edit,
    Cancel,
}

#[derive(Debug)]
pub(super) enum Outcome {
    Reserved,
    QueuedInput(QueuedInputOutcome),
}

#[derive(Clone)]
pub(super) enum Request {
    Ordinary(Box<CommandReservation>),
    Pair(Box<InterruptingMessageReservation>),
    Targeted(Box<TargetedControlReservation>),
    Edit(Box<QueuedInputReservation>),
    Cancel(Box<QueuedCancellationReservation>),
}
impl Request {
    pub async fn prepare(store: &MongoSandboxStore, kind: Kind) -> (AgentId, Self) {
        let (parent, mut agent) = fixture(store, "m1-typed-leaf").await;
        if matches!(kind, Kind::Ordinary) {
            return (
                agent.id.clone(),
                Self::Ordinary(Box::new(live_request(&agent, &parent, "leaf-ordinary"))),
            );
        }
        if matches!(kind, Kind::Pair | Kind::Targeted) {
            let previous = agent.revision;
            agent.capabilities = layer2_core::RuntimeCapabilities::new([
                layer2_core::RuntimeCapability::Conversation,
                layer2_core::RuntimeCapability::Interrupt,
            ]);
            agent.revision += 1;
            store.save_agent(previous, agent.clone()).await.unwrap();
            let pair = super::pair::prepare_pair(store, &agent, &parent).await;
            return (
                agent.id,
                match kind {
                    Kind::Pair => Self::Pair(Box::new(pair)),
                    Kind::Targeted => Self::Targeted(Box::new(pair.control)),
                    _ => unreachable!(),
                },
            );
        }
        let original = record(
            store
                .reserve_command(live_request(&agent, &parent, "queued-original"))
                .await
                .unwrap(),
        );
        let mut command = live_request(&agent, &parent, "leaf-queued");
        command.envelope.command = AgentCommand::Interrupt;
        command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
        command.response_id = None;
        let cancellation = QueuedCancellationReservation {
            command,
            target: QueuedCancellationTarget {
                response_id: original.receipt.response_id.unwrap(),
                source_command_id: original.receipt.envelope.command_id,
                expected_command_revision: original.revision,
            },
            observed_agent_revision: agent.revision,
            agent_created_at: agent.created_at,
        };
        (
            agent.id,
            if matches!(kind, Kind::Edit) {
                Self::Edit(Box::new(QueuedInputReservation {
                    cancellation,
                    text: "edited once".into(),
                }))
            } else {
                Self::Cancel(Box::new(cancellation))
            },
        )
    }
    pub fn added_commands(&self) -> usize {
        if matches!(self, Self::Pair(_)) {
            2
        } else {
            1
        }
    }
    pub async fn run(&self, store: &MongoSandboxStore) -> Result<Outcome, CommandLedgerError> {
        match self {
            Self::Ordinary(r) => store
                .reserve_command(*r.clone())
                .await
                .map(|_| Outcome::Reserved),
            Self::Pair(r) => store
                .reserve_interrupting_message(*r.clone())
                .await
                .map(|_| Outcome::Reserved),
            Self::Targeted(r) => store
                .reserve_targeted_control(*r.clone())
                .await
                .map(|_| Outcome::Reserved),
            Self::Edit(r) => store
                .edit_queued_input(*r.clone())
                .await
                .map(Outcome::QueuedInput),
            Self::Cancel(r) => store
                .cancel_queued_response(*r.clone())
                .await
                .map(|_| Outcome::Reserved),
        }
    }
}
