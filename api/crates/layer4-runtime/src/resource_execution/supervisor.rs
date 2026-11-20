use super::*;
use crate::manager_ownership::OwnerSession;
use journal::{Record, MAX_RECORDS};
use layer3_adapter::owner_protocol::TransportOwner;

impl Receiver {
    pub(crate) async fn challenge(
        self: &Arc<Self>,
        owner: TransportOwner,
        execution: ExecutionKey,
    ) -> ProtocolResult<ReceiverChallengeV1> {
        let receiver = self.clone();
        tokio::task::spawn_blocking(move || receiver.challenge_now(&owner, &execution))
            .await
            .map_err(|_| ProtocolError::Unknown)?
    }

    fn challenge_now(
        &self,
        owner: &TransportOwner,
        execution: &ExecutionKey,
    ) -> ProtocolResult<ReceiverChallengeV1> {
        self.require_admission()?;
        self.require_owner(owner, execution)?;
        if execution.receiver != self.identity {
            return Err(ProtocolError::AudienceMismatch);
        }
        let now = self.clock.tick_ms()?;
        let mut state = self.state()?;
        let mut snapshot = state.snapshot.clone().ok_or(ProtocolError::Unsupported)?;
        if let Some(index) = snapshot
            .records
            .iter()
            .position(|record| &record.execution == execution)
        {
            let record = &mut snapshot.records[index];
            if record.barrier.is_some() || state.gates[index].expire(now) {
                return Err(ProtocolError::Expired);
            }
            let accepted = match (&record.ticket, &record.continuation) {
                (Some(ticket), Some(lease))
                    if record.start.is_some() && ticket.basis.family().is_some() =>
                {
                    &lease.challenge
                }
                (Some(ticket), None)
                    if record.start.is_some() && ticket.basis.family().is_some() =>
                {
                    &ticket.challenge
                }
                (None, None) => {
                    return record
                        .challenge
                        .check_at(&self.identity, now)
                        .map(|()| record.challenge.clone())
                }
                _ => return Err(ProtocolError::Unknown),
            };
            if record.challenge != *accepted {
                return record
                    .challenge
                    .check_at(&self.identity, now)
                    .map(|()| record.challenge.clone());
            }
            if now <= accepted.created_tick_ms {
                return Err(ProtocolError::Unavailable);
            }
            record.challenge = self.new_challenge(now)?;
            let challenge = record.challenge.clone();
            snapshot.next()?;
            state.publish(snapshot)?;
            return Ok(challenge);
        }
        if snapshot.records.len() >= MAX_RECORDS {
            return Err(ProtocolError::Capacity);
        }
        let challenge = self.new_challenge(now)?;
        snapshot.records.push(Record {
            execution: execution.clone(),
            challenge: challenge.clone(),
            ticket: None,
            cell: None,
            continuation: None,
            start: None,
            barrier: None,
            stop: None,
        });
        snapshot.next()?;
        state.publish(snapshot)?;
        state
            .gates
            .push(Arc::new(Gate::new(challenge.deadline_tick_ms, false)));
        Ok(challenge)
    }

    fn new_challenge(&self, now: u64) -> ProtocolResult<ReceiverChallengeV1> {
        Ok(ReceiverChallengeV1 {
            protocol: PROTOCOL_VERSION,
            receiver: self.identity.clone(),
            nonce: OpaqueId::new(&uuid::Uuid::new_v4().simple().to_string())?,
            created_tick_ms: now,
            deadline_tick_ms: now
                .checked_add(MAX_LEASE_MS)
                .ok_or(ProtocolError::Unavailable)?,
        })
    }

    pub(crate) async fn continue_execution(
        self: &Arc<Self>,
        authority: OwnerSession,
        lease: ContinuationV1,
    ) -> ProtocolResult<()> {
        let owner = authority.owner().ok_or(ProtocolError::AudienceMismatch)?;
        let receiver = self.clone();
        let renewal = tokio::task::spawn_blocking(move || {
            receiver.require_admission()?;
            receiver.require_owner(&owner, &lease.execution)?;
            if lease.execution.receiver != receiver.identity || !lease.manager.matches_owner(&owner)
            {
                return Err(ProtocolError::AudienceMismatch);
            }
            let now = receiver.clock.tick_ms()?;
            lease.challenge.check_at(&receiver.identity, now)?;
            let mut state = receiver.state()?;
            let mut snapshot = state.snapshot.clone().ok_or(ProtocolError::Unsupported)?;
            let index = snapshot
                .records
                .iter()
                .position(|r| r.execution == lease.execution)
                .ok_or(ProtocolError::EvidenceMismatch)?;
            let record = &snapshot.records[index];
            let ticket = record
                .ticket
                .as_ref()
                .ok_or(ProtocolError::EvidenceMismatch)?;
            lease.validate_for(ticket)?;
            if ticket.basis.family().is_none()
                || record.start.is_none()
                || record.barrier.is_some()
                || state.gates[index].expire(now)
            {
                return Err(ProtocolError::Expired);
            }
            if record.continuation.as_ref() == Some(&lease) {
                return Ok((state.gates[index].clone(), lease.challenge.deadline_tick_ms));
            }
            match &record.continuation {
                Some(previous) => lease.validate_after(previous)?,
                None if lease.sequence.get() == 2 => (),
                None => return Err(ProtocolError::EvidenceMismatch),
            }
            if record.challenge != lease.challenge {
                return Err(ProtocolError::EvidenceMismatch);
            }
            let deadline = lease.challenge.deadline_tick_ms;
            snapshot.records[index].continuation = Some(lease);
            snapshot.next()?;
            state.publish(snapshot)?;
            // Persistence cannot extend the challenge or reopen an expiry
            // that the independent watchdog observed during the disk write.
            Ok((state.gates[index].clone(), deadline))
        })
        .await
        .map_err(|_| ProtocolError::Unknown)??;
        let permit = authority
            .permit()
            .await
            .map_err(|_| ProtocolError::AudienceMismatch)?;
        let (gate, deadline) = renewal;
        gate.renew(self.clock.tick_ms()?, deadline)?;
        drop(permit);
        Ok(())
    }

    pub(super) fn require_admission(&self) -> ProtocolResult<()> {
        self.capabilities()
            .ok_or(ProtocolError::Unsupported)
            .map(|_| ())
    }

    pub(super) fn require_owner(
        &self,
        owner: &TransportOwner,
        execution: &ExecutionKey,
    ) -> ProtocolResult<()> {
        owner
            .validate()
            .map_err(|_| ProtocolError::AudienceMismatch)?;
        if owner.agent_id != execution.agent_id.as_str()
            || execution.receiver.kind != ReceiverKind::Runtime
            || execution.receiver.instance != self.identity.instance
            || execution.receiver.incarnation != self.identity.incarnation
        {
            return Err(ProtocolError::AudienceMismatch);
        }
        Ok(())
    }
}
