use super::*;
use journal::{Floor, Record, MAX_RECORDS};
use layer3_adapter::owner_protocol::TransportOwner;
use runner::Termination;

impl Receiver {
    pub(crate) async fn barrier(
        self: &Arc<Self>,
        owner: TransportOwner,
        request: BarrierRequestV1,
    ) -> ProtocolResult<BarrierReceiptV1> {
        let receiver = self.clone();
        tokio::task::spawn_blocking(move || receiver.barrier_now(&owner, &request))
            .await
            .map_err(|_| ProtocolError::Unknown)?
    }

    fn barrier_now(
        &self,
        owner: &TransportOwner,
        request: &BarrierRequestV1,
    ) -> ProtocolResult<BarrierReceiptV1> {
        request.validate()?;
        let MemberKey::Execution { execution } = &request.member else {
            return Err(ProtocolError::AudienceMismatch);
        };
        self.require_owner(owner, execution)?;
        let mut state = self.state()?;
        let mut snapshot = state.snapshot.clone().ok_or(ProtocolError::Unsupported)?;
        let index = snapshot
            .records
            .iter()
            .position(|r| &r.execution == execution)
            .ok_or(ProtocolError::Unknown)?;
        let record = &snapshot.records[index];
        if let Some(barrier) = &record.barrier {
            return if &barrier.request == request {
                Ok(barrier.clone())
            } else {
                Err(ProtocolError::EvidenceMismatch)
            };
        }
        if record.ticket.as_ref().is_some_and(|ticket| {
            ticket.registration != request.registration
                || ticket.basis.family() != Some(&request.revocation.family)
        }) {
            return Err(ProtocolError::EvidenceMismatch);
        }
        // Close every sibling in this grant before starting durable IO. The
        // exact member still gets its own immutable receipt and stop obligation.
        for (record, gate) in snapshot.records.iter().zip(&state.gates) {
            if &record.execution == execution
                || record
                    .ticket
                    .as_ref()
                    .is_some_and(|ticket| ticket.basis.family() == Some(&request.revocation.family))
            {
                gate.close();
            }
        }
        let floor_capacity = snapshot.floors.len() < MAX_RECORDS;
        match snapshot
            .floors
            .iter_mut()
            .find(|floor| floor.family == request.revocation.family)
        {
            Some(floor) => floor.watermark = floor.watermark.max(request.revocation.watermark),
            None if floor_capacity => snapshot.floors.push(Floor {
                family: request.revocation.family.clone(),
                watermark: request.revocation.watermark,
            }),
            None => {
                state.poison();
                return Err(ProtocolError::Capacity);
            }
        }
        let receipt = BarrierReceiptV1 {
            protocol: PROTOCOL_VERSION,
            request: request.clone(),
            barrier_sequence: snapshot.next()?,
            durable_watermark: request.revocation.watermark,
        };
        snapshot.records[index].barrier = Some(receipt.clone());
        state.publish(snapshot)?;
        Ok(receipt)
    }

    pub(crate) async fn stop(
        self: &Arc<Self>,
        owner: TransportOwner,
        request: BarrierRequestV1,
        cancel: bool,
    ) -> ProtocolResult<Option<StopReceiptV1>> {
        let barrier = if cancel {
            self.barrier(owner.clone(), request.clone()).await?
        } else {
            let state = self.state()?;
            let record = find_record(&state, &owner, &request)?;
            if let Some(stop) = &record.stop {
                return Ok(Some(stop.clone()));
            }
            return Ok(None);
        };
        let (cell, gate, stored) = {
            let state = self.state()?;
            let record = find_record(&state, &owner, &request)?;
            let index = state
                .snapshot
                .as_ref()
                .expect("active snapshot")
                .records
                .iter()
                .position(|r| r.execution == record.execution)
                .ok_or(ProtocolError::Unknown)?;
            (
                record.cell.clone(),
                state.gates[index].clone(),
                record.stop.clone(),
            )
        };
        if stored.is_some() {
            return Ok(stored);
        }
        gate.close();
        let (quiescence, evidence) = if let Some(cell) = &cell {
            let _ = self.runner.cancel(cell, Termination::Cooperative);
            if !gate.ready_to_observe_stop() {
                return Err(ProtocolError::Unknown);
            }
            let observed = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                self.runner.observe_quiescence(cell),
            )
            .await
            .map_err(|_| ProtocolError::Unknown)??;
            let evidence =
                if gate.never_entered() && cell.execution.receiver.boot == self.identity.boot {
                    StopEvidenceKind::RejectedBeforeStart
                } else {
                    if observed.evidence == StopEvidenceKind::RejectedBeforeStart {
                        return Err(ProtocolError::EvidenceMismatch);
                    }
                    observed.evidence
                };
            (observed.state, evidence)
        } else {
            (empty_quiescence()?, StopEvidenceKind::RejectedBeforeStart)
        };
        quiescence.validate()?;
        let receiver = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = receiver.state()?;
            if let Some(stop) = &find_record(&state, &owner, &request)?.stop {
                return Ok(Some(stop.clone()));
            }
            let mut snapshot = state.snapshot.clone().ok_or(ProtocolError::Unknown)?;
            let index = snapshot
                .records
                .iter()
                .position(|r| r.barrier.as_ref() == Some(&barrier))
                .ok_or(ProtocolError::EvidenceMismatch)?;
            let receipt = StopReceiptV1 {
                protocol: PROTOCOL_VERSION,
                barrier,
                stop_sequence: snapshot.next()?,
                evidence,
                quiescence,
            };
            receipt.validate_for(&request)?;
            snapshot.records[index].stop = Some(receipt.clone());
            state.publish(snapshot)?;
            Ok(Some(receipt))
        })
        .await
        .map_err(|_| ProtocolError::Unknown)?
    }
}

fn find_record<'a>(
    state: &'a State,
    owner: &TransportOwner,
    request: &BarrierRequestV1,
) -> ProtocolResult<&'a Record> {
    request.validate()?;
    owner
        .validate()
        .map_err(|_| ProtocolError::AudienceMismatch)?;
    let MemberKey::Execution { execution } = &request.member else {
        return Err(ProtocolError::AudienceMismatch);
    };
    if owner.agent_id != execution.agent_id.as_str() {
        return Err(ProtocolError::AudienceMismatch);
    }
    let snapshot = state.snapshot.as_ref().ok_or(ProtocolError::Unsupported)?;
    let record = snapshot
        .records
        .iter()
        .find(|record| &record.execution == execution)
        .ok_or(ProtocolError::Unknown)?;
    if record
        .barrier
        .as_ref()
        .is_none_or(|barrier| &barrier.request != request)
    {
        return Err(ProtocolError::EvidenceMismatch);
    }
    Ok(record)
}

fn empty_quiescence() -> ProtocolResult<QuiescenceV1> {
    use sha2::{Digest as _, Sha256};
    let digest = Digest::new(&format!(
        "{:x}",
        Sha256::digest(b"cma-resource-no-entry-v1")
    ))?;
    Ok(QuiescenceV1 {
        queues_purged: true,
        descendants_absent: true,
        output_closed: true,
        submitters_closed: true,
        pending_local_effects_settled: true,
        local_changes_digest: digest.clone(),
        effects: EffectJournalV1 {
            high_water: 0,
            digest,
            unresolved_count: 0,
        },
    })
}
