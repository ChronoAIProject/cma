//! Optional, process-local evidence; never a receiving lease or permission to act.
use std::collections::BTreeMap;
use std::sync::Mutex;

use super::{OwnershipFailure, TransportOwner};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnershipReceiver {
    runtime_id: String,
    pod_uid: String,
    container_id: String,
    restart_count: i32,
}

impl OwnershipReceiver {
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub(crate) fn from_native(
        handle: &crate::RuntimeHandle,
        binding: &crate::agent_sandbox::PodBinding,
    ) -> Option<Self> {
        let receiver = Self {
            runtime_id: handle.as_str().into(),
            pod_uid: binding.pod_uid.clone(),
            container_id: binding.runtime_container_id.clone()?,
            restart_count: binding.runtime_restart_count?,
        };
        receiver.valid().then_some(receiver)
    }

    fn valid(&self) -> bool {
        [&self.runtime_id, &self.pod_uid, &self.container_id]
            .iter()
            .all(|value| !value.is_empty() && value.len() <= 512)
            && self.restart_count >= 0
    }
}

#[derive(Clone, Debug)]
pub struct FirstOwnershipFailure {
    owner: TransportOwner,
    receiver: Option<OwnershipReceiver>,
    failure: OwnershipFailure,
    confirmation_sent: bool,
    previously_confirmed: bool,
    challenge_valid: bool,
}

impl FirstOwnershipFailure {
    pub fn owner(&self) -> &TransportOwner {
        &self.owner
    }
    pub fn receiver(&self) -> Option<&OwnershipReceiver> {
        self.receiver.as_ref()
    }
    pub fn failure(&self) -> OwnershipFailure {
        self.failure
    }
    pub fn confirmation_sent(&self) -> bool {
        self.confirmation_sent
    }
    pub fn previously_confirmed(&self) -> bool {
        self.previously_confirmed
    }
    pub fn challenge_valid(&self) -> bool {
        self.challenge_valid
    }
}

#[derive(Default)]
struct State {
    receivers: BTreeMap<OwnershipReceiver, (bool, bool)>,
    first: Option<FirstOwnershipFailure>,
    uncertain: bool,
    negotiations: usize,
}

/// Installed before the first receiver registration. Histories are bounded by
/// the authority's receiver limit, and confirmation state never moves backward.
#[derive(Default)]
pub struct OwnershipEvidence(Mutex<State>);

impl OwnershipEvidence {
    pub(crate) fn negotiation(&self) -> Negotiation<'_> {
        let mut state = self.0.lock().unwrap();
        // Overlap can outlive a failure snapshot. Conservatively exclude the
        // whole attempt before either negotiation can send a confirmation.
        state.uncertain |= state.negotiations != 0;
        state.negotiations += 1;
        Negotiation { evidence: self }
    }

    pub fn first_failure(&self) -> Option<FirstOwnershipFailure> {
        let state = self.0.lock().unwrap();
        let mut first = state.first.clone()?;
        // Concurrent negotiations can confirm after the initiating rejection.
        // Keep its cause immutable, but consult all history at consumption time.
        let (sent, confirmed) = first
            .receiver
            .as_ref()
            .and_then(|receiver| state.receivers.get(receiver))
            .copied()
            .unwrap_or_default();
        first.confirmation_sent |= sent || state.uncertain;
        first.previously_confirmed |= confirmed || state.uncertain;
        Some(first)
    }

    pub(crate) fn confirmation(&self, receiver: Option<&OwnershipReceiver>, succeeded: bool) {
        let mut state = self.0.lock().unwrap();
        let Some(receiver) = receiver.filter(|receiver| receiver.valid()) else {
            state.uncertain = true;
            return;
        };
        if state.receivers.len() >= 64 && !state.receivers.contains_key(receiver) {
            state.uncertain = true;
            return;
        }
        let entry = state.receivers.entry(receiver.clone()).or_default();
        entry.0 = true;
        entry.1 |= succeeded;
    }

    pub fn record_unknown(&self, owner: TransportOwner, failure: OwnershipFailure) {
        self.record(owner, None, failure, false);
    }

    pub(crate) fn record(
        &self,
        owner: TransportOwner,
        receiver: Option<OwnershipReceiver>,
        failure: OwnershipFailure,
        challenge_valid: bool,
    ) {
        let mut state = self.0.lock().unwrap();
        if state.first.is_some() {
            return;
        }
        let receiver = receiver.filter(OwnershipReceiver::valid);
        let (sent, confirmed) = receiver
            .as_ref()
            .and_then(|receiver| state.receivers.get(receiver))
            .copied()
            .unwrap_or_default();
        state.first = Some(FirstOwnershipFailure {
            owner,
            receiver,
            failure,
            confirmation_sent: sent || state.uncertain,
            previously_confirmed: confirmed || state.uncertain,
            challenge_valid,
        });
    }
}

pub(crate) struct Negotiation<'a> {
    evidence: &'a OwnershipEvidence,
}

impl Drop for Negotiation<'_> {
    fn drop(&mut self) {
        self.evidence.0.lock().unwrap().negotiations -= 1;
    }
}

#[cfg(test)]
#[path = "provider_ownership_evidence_tests.rs"]
mod tests;
