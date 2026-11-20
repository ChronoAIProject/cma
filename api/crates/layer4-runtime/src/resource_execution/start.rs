use super::*;
use crate::manager_ownership::OwnerSession;
use layer3_adapter::owner_protocol::TransportOwner;
use layer3_adapter::ResourceDispatch;
use runner::{Cell, Termination};

impl Receiver {
    pub(crate) async fn start(
        self: &Arc<Self>,
        authority: OwnerSession,
        dispatch: ResourceDispatch,
    ) -> ProtocolResult<StartObservationV1> {
        let receiver = self.clone();
        tokio::spawn(async move { receiver.start_owned(authority, dispatch).await })
            .await
            .map_err(|_| ProtocolError::Unknown)?
    }

    async fn start_owned(
        self: Arc<Self>,
        authority: OwnerSession,
        dispatch: ResourceDispatch,
    ) -> ProtocolResult<StartObservationV1> {
        self.require_admission()?;
        let owner = authority.owner().ok_or(ProtocolError::AudienceMismatch)?;
        authority
            .permit()
            .await
            .map_err(|_| ProtocolError::AudienceMismatch)?;
        let receiver = self.clone();
        let prepared = dispatch.clone();
        let plan = tokio::task::spawn_blocking(move || receiver.intent(&owner, &prepared))
            .await
            .map_err(|_| ProtocolError::Unknown)??;
        match plan {
            Start::Observed(observation) => Ok(*observation),
            Start::Prepare { cell, gate } => self.enter(authority, dispatch, *cell, gate).await,
        }
    }

    fn intent(&self, owner: &TransportOwner, dispatch: &ResourceDispatch) -> ProtocolResult<Start> {
        let ticket = dispatch.ticket();
        self.require_owner(owner, &ticket.execution)?;
        if ticket.execution.receiver != self.identity {
            return Err(ProtocolError::AudienceMismatch);
        }
        ticket.validate()?;
        if !ticket.manager.matches_owner(owner) {
            return Err(ProtocolError::AudienceMismatch);
        }
        let mut state = self.state()?;
        let mut snapshot = state.snapshot.clone().ok_or(ProtocolError::Unsupported)?;
        let index = snapshot
            .records
            .iter()
            .position(|r| r.execution == ticket.execution)
            .ok_or(ProtocolError::EvidenceMismatch)?;
        let record = &snapshot.records[index];
        if let Some(original) = &record.ticket {
            if original != ticket {
                return Err(ProtocolError::EvidenceMismatch);
            }
            return record
                .start
                .clone()
                .map(|o| Start::Observed(Box::new(o)))
                .ok_or(ProtocolError::Unknown);
        }
        let now = self.clock.tick_ms()?;
        ticket.validate_for(owner, &ticket.execution, dispatch.input().digest(), now)?;
        if record.challenge != ticket.challenge {
            return Err(ProtocolError::EvidenceMismatch);
        }
        if record.barrier.is_some()
            || ticket
                .basis
                .family()
                .is_some_and(|family| snapshot.floors.iter().any(|floor| &floor.family == family))
        {
            return Err(ProtocolError::Expired);
        }
        let cell = Cell {
            execution: ticket.execution.clone(),
            nonce: OpaqueId::new(&uuid::Uuid::new_v4().simple().to_string())?,
        };
        let gate = state.gates[index].clone();
        snapshot.records[index].ticket = Some(ticket.clone());
        snapshot.records[index].cell = Some(cell.clone());
        snapshot.next()?;
        let watchdog = self.watchdog.as_ref().ok_or(ProtocolError::Unsupported)?;
        gate.prepare(now)?;
        if let Err(error) = state.publish(snapshot) {
            gate.prepared();
            return Err(error);
        }
        if watchdog.register(cell.clone(), gate.clone()).is_err() {
            gate.prepared();
            state.poison();
            return Err(ProtocolError::Unknown);
        }
        Ok(Start::Prepare {
            cell: Box::new(cell),
            gate,
        })
    }

    async fn enter(
        self: Arc<Self>,
        authority: OwnerSession,
        dispatch: ResourceDispatch,
        cell: Cell,
        gate: Arc<Gate>,
    ) -> ProtocolResult<StartObservationV1> {
        let result = self.runner.prepare(&cell, dispatch.input().clone()).await;
        gate.prepared();
        let release = async {
            result?;
            let permit = authority
                .permit()
                .await
                .map_err(|_| ProtocolError::AudienceMismatch)?;
            let now = self.clock.tick_ms()?;
            gate.enter(now, dispatch.ticket().basis.family().is_some(), |entry| {
                self.runner.enter(&cell, entry)
            })?;
            drop(permit);
            Ok(())
        }
        .await;
        if let Err(error) = release {
            gate.close();
            let _ = self.runner.cancel(&cell, Termination::Force);
            return Err(error);
        }
        let receiver = self.clone();
        let ticket = dispatch.ticket().clone();
        tokio::task::spawn_blocking(move || {
            let mut state = receiver.state()?;
            let mut snapshot = state.snapshot.clone().ok_or(ProtocolError::Unknown)?;
            let index = snapshot
                .records
                .iter()
                .position(|r| r.ticket.as_ref() == Some(&ticket))
                .ok_or(ProtocolError::Unknown)?;
            let observation = StartObservationV1 {
                protocol: PROTOCOL_VERSION,
                execution: ticket.execution,
                registration: ticket.registration,
                nonce: ticket.nonce,
                state: NativeStartState::Entered,
                sequence: snapshot.next()?,
            };
            snapshot.records[index].start = Some(observation.clone());
            state.publish(snapshot)?;
            Ok(observation)
        })
        .await
        .map_err(|_| ProtocolError::Unknown)?
    }
}

enum Start {
    Observed(Box<StartObservationV1>),
    Prepare { cell: Box<Cell>, gate: Arc<Gate> },
}
