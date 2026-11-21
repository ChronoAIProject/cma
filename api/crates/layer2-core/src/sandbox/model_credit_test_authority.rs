use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

type Balances = HashMap<(String, String), u64>;

#[derive(Default)]
pub(in crate::sandbox) struct LedgerState {
    balances: Balances,
    active: HashMap<String, u32>,
    checkpoints: HashMap<PhaseId, Checkpoint>,
    expired: HashSet<PhaseId>,
    refused_versions: HashMap<PhaseId, NonZeroU64>,
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
        let version = state
            .refused_versions
            .get(&request.id)
            .map_or(units(1), |old| units(old.get() + 1));
        state.hold(request, version)
    }

    async fn read(&self, id: &PhaseId) -> Result<Checkpoint, FundingError> {
        self.lock()
            .unwrap()
            .checkpoints
            .get(id)
            .cloned()
            .ok_or(FundingError::NoReservation)
    }

    async fn release_refused(&self, expected: &Checkpoint) -> Result<(), FundingError> {
        let mut state = self.lock().unwrap();
        if expected.reservation.command_id.is_none() || expected.state != PhaseState::Reserved {
            return Err(FundingError::CheckpointChanged);
        }
        let id = &expected.reservation.id;
        if !state.checkpoints.contains_key(id) {
            return Ok(());
        }
        state.next(expected, PhaseState::Settled(Effect::NotStarted))?;
        state.add(
            &expected.reservation.binding,
            expected.reservation.limit.get(),
        );
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
            .filter_map(|row| row.reservation.command_id.as_ref())
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
        let mut state = self.lock().unwrap();
        if expected.state != PhaseState::Reserved {
            return Err(FundingError::CheckpointChanged);
        }
        if state.expired.contains(&expected.reservation.id) {
            return Err(FundingError::Expired);
        }
        let permit = state.next(expected, PhaseState::InFlight)?;
        state
            .checkpoints
            .insert(expected.reservation.id.clone(), permit.clone());
        Ok(permit)
    }

    async fn settle(
        &self,
        expected: &Checkpoint,
        effect: Effect,
    ) -> Result<Checkpoint, FundingError> {
        tokio::task::yield_now().await;
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
