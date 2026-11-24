use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

type Balances = HashMap<(String, String), u64>;

#[derive(Clone, Copy, Debug)]
pub(in crate::sandbox) enum BeginReply {
    Error(FundingError),
    UnchangedVersion,
    ChangedReservation,
    WrongState,
}

#[derive(Default)]
pub(in crate::sandbox) struct LedgerState {
    balances: Balances,
    active: HashMap<String, u32>,
    checkpoints: HashMap<PhaseId, Checkpoint>,
    expired: HashSet<PhaseId>,
    refused_versions: HashMap<PhaseId, NonZeroU64>,
    admissions: HashMap<PhaseId, HashSet<NonZeroU64>>,
    last_participant: u64,
    pub(in crate::sandbox) begin_reply: Option<BeginReply>,
    pub(in crate::sandbox) begin_barrier: Option<std::sync::Arc<tokio::sync::Barrier>>,
    pub(in crate::sandbox) settle_barrier: Option<std::sync::Arc<tokio::sync::Barrier>>,
    pub(in crate::sandbox) read_unavailable: bool,
}

pub(in crate::sandbox) type Ledger = Mutex<LedgerState>;

impl LedgerState {
    pub(super) fn expire(&mut self, id: PhaseId) {
        self.expired.insert(id);
    }

    pub(in crate::sandbox) fn add(&mut self, binding: &PayerBinding, amount: u64) {
        *self.available(binding) += amount;
    }

    pub(in crate::sandbox) fn available(&mut self, binding: &PayerBinding) -> &mut u64 {
        self.balances
            .entry((binding.payer.clone(), binding.pool.clone()))
            .or_default()
    }

    fn hold(
        &mut self,
        reservation: &Reservation,
        version: NonZeroU64,
    ) -> Result<Checkpoint, FundingError> {
        let available = self.available(&reservation.binding);
        *available = available
            .checked_sub(reservation.limit.get())
            .ok_or(FundingError::Insufficient)?;
        let checkpoint = Checkpoint {
            reservation: reservation.clone(),
            version,
            state: PhaseState::Reserved,
        };
        self.active
            .insert(reservation.id.turn.clone(), reservation.id.phase);
        self.checkpoints
            .insert(reservation.id.clone(), checkpoint.clone());
        Ok(checkpoint)
    }

    fn hold_first(&mut self, request: &Reservation) -> Result<Checkpoint, FundingError> {
        let version = self
            .refused_versions
            .get(&request.id)
            .map_or(units(1), |old| units(old.get() + 1));
        self.hold(request, version)
    }

    fn next(&self, checkpoint: &Checkpoint, state: PhaseState) -> Result<Checkpoint, FundingError> {
        let id = &checkpoint.reservation.id;
        if self.active.get(&id.turn) != Some(&id.phase)
            || self.checkpoints.get(id) != Some(checkpoint)
        {
            return Err(FundingError::CheckpointChanged);
        }
        Ok(Checkpoint {
            version: units(checkpoint.version.get() + 1),
            state,
            ..checkpoint.clone()
        })
    }
}

#[async_trait]
impl ReservationAuthority for Ledger {
    async fn reserve_first(&self, request: &Reservation) -> Result<Checkpoint, FundingError> {
        tokio::task::yield_now().await;
        let mut state = self.lock().unwrap();
        if let Some(existing) = state.checkpoints.get(&request.id) {
            require_reservation(existing, request)?;
            return Ok(existing.clone());
        }
        state.hold_first(request)
    }

    async fn reserve_command(&self, request: &Reservation) -> Result<AdmissionHold, FundingError> {
        tokio::task::yield_now().await;
        let mut state = self.lock().unwrap();
        let proposed = request
            .command
            .as_ref()
            .ok_or(FundingError::CheckpointChanged)?;
        let participant = state
            .last_participant
            .checked_add(1)
            .map(units)
            .ok_or(FundingError::Unavailable)?;
        let checkpoint = if let Some(existing) = state.checkpoints.get(&request.id).cloned() {
            if existing.state != PhaseState::Reserved {
                return Err(FundingError::CheckpointChanged);
            }
            let existing_identity = existing
                .reservation
                .command
                .as_ref()
                .ok_or(FundingError::CheckpointChanged)?;
            if existing.reservation.binding != request.binding {
                return Err(FundingError::BindingChanged);
            }
            if existing_identity.intent_fingerprint != proposed.intent_fingerprint {
                return Err(FundingError::CheckpointChanged);
            }
            let matched = Reservation {
                command: Some(existing_identity.clone()),
                ..request.clone()
            };
            require_reservation(&existing, &matched)?;
            existing
        } else {
            state.hold_first(request)?
        };
        state.last_participant = participant.get();
        state
            .admissions
            .entry(request.id.clone())
            .or_default()
            .insert(participant);
        Ok(AdmissionHold {
            checkpoint,
            participant,
        })
    }

    async fn read(&self, id: &PhaseId) -> Result<Checkpoint, FundingError> {
        let state = self.lock().unwrap();
        if state.read_unavailable {
            return Err(FundingError::Unavailable);
        }
        state
            .checkpoints
            .get(id)
            .cloned()
            .ok_or(FundingError::NoReservation)
    }

    async fn refuse_admission(&self, hold: &AdmissionHold) -> Result<(), FundingError> {
        let mut state = self.lock().unwrap();
        let expected = &hold.checkpoint;
        if expected.reservation.command.is_none() || expected.state != PhaseState::Reserved {
            return Err(FundingError::CheckpointChanged);
        }
        let id = &expected.reservation.id;
        if state.checkpoints.get(id) != Some(expected) {
            return Ok(());
        }
        let Some(participants) = state.admissions.get_mut(id) else {
            return Ok(());
        };
        if !participants.remove(&hold.participant) || !participants.is_empty() {
            return Ok(());
        }
        state.add(
            &expected.reservation.binding,
            expected.reservation.limit.get(),
        );
        state.admissions.remove(id);
        state.checkpoints.remove(id);
        state.active.remove(&id.turn);
        state.refused_versions.insert(id.clone(), expected.version);
        Ok(())
    }

    async fn pending_commands(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentCommandId>, FundingError> {
        use std::collections::BTreeSet;
        let ledger = self.lock().unwrap();
        let ids = ledger
            .checkpoints
            .values()
            .filter(|row| !matches!(row.state, PhaseState::Settled(_)))
            .filter_map(|row| {
                row.reservation
                    .command
                    .as_ref()
                    .map(|identity| &identity.command_id)
            })
            .filter(|id| {
                page.cursor
                    .as_deref()
                    .is_none_or(|cursor| cursor < id.as_str())
            });
        let sorted: BTreeSet<_> = ids.collect();
        let limit = usize::from(page.limit);
        let mut items: Vec<_> = sorted.into_iter().take(limit + 1).cloned().collect();
        let next = (items.len() > limit).then(|| items[limit - 1].to_string());
        items.truncate(limit);
        Ok(Page { items, next })
    }

    async fn begin(&self, expected: &Checkpoint) -> Result<Checkpoint, FundingError> {
        tokio::task::yield_now().await;
        let barrier = self.lock().unwrap().begin_barrier.clone();
        if let Some(barrier) = barrier {
            barrier.wait().await;
        }
        let mut state = self.lock().unwrap();
        if expected.state != PhaseState::Reserved {
            return Err(FundingError::CheckpointChanged);
        }
        let permit = state.next(expected, PhaseState::InFlight)?;
        if state.expired.contains(&expected.reservation.id) {
            return Err(FundingError::Expired);
        }
        state.admissions.remove(&expected.reservation.id);
        state
            .checkpoints
            .insert(expected.reservation.id.clone(), permit.clone());
        let mut reply = permit;
        match state.begin_reply.take() {
            Some(BeginReply::Error(error)) => return Err(error),
            Some(BeginReply::UnchangedVersion) => reply.version = expected.version,
            Some(BeginReply::ChangedReservation) => reply.reservation.request_digest[0] ^= 1,
            Some(BeginReply::WrongState) => reply.state = PhaseState::Reserved,
            None => {}
        }
        Ok(reply)
    }

    async fn settle(
        &self,
        expected: &Checkpoint,
        effect: Effect,
    ) -> Result<Checkpoint, FundingError> {
        tokio::task::yield_now().await;
        let barrier = self.lock().unwrap().settle_barrier.take();
        if let Some(barrier) = barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        let mut state = self.lock().unwrap();
        let reservation = &expected.reservation;
        if expected.state != PhaseState::InFlight
            && !(expected.state == PhaseState::Reserved && effect == Effect::NotStarted)
        {
            return Err(FundingError::CheckpointChanged);
        }
        if let Some(old) = state.checkpoints.get(&reservation.id) {
            if old.reservation == *reservation
                && old.version.get() == expected.version.get() + 1
                && old.state == PhaseState::Settled(effect)
            {
                return Ok(old.clone());
            }
        }
        let settled = state.next(expected, PhaseState::Settled(effect))?;
        let spent = match effect {
            Effect::Finished { spent } => spent,
            Effect::NotStarted => 0,
        };
        let release = reservation
            .limit
            .get()
            .checked_sub(spent)
            .ok_or(FundingError::ReconciliationRequired)?;
        state.add(&reservation.binding, release);
        state.admissions.remove(&reservation.id);
        state
            .checkpoints
            .insert(reservation.id.clone(), settled.clone());
        Ok(settled)
    }

    async fn advance(
        &self,
        expected: &Checkpoint,
        next: &Reservation,
    ) -> Result<Checkpoint, FundingError> {
        tokio::task::yield_now().await;
        let mut state = self.lock().unwrap();
        let revision = state.next(expected, PhaseState::Reserved)?.version;
        state.hold(next, revision)
    }
}
