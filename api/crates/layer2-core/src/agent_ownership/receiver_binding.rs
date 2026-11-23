//! Content-free observations for advisory retry episodes, never ownership proof.
use super::Authority;
use sha2::{Digest, Sha256};

impl Authority {
    pub(super) fn receiver_binding(&self) -> Option<([u8; 32], bool)> {
        let receivers = self.receivers.lock().unwrap();
        if receivers.is_empty() {
            return None;
        }
        let confirmed = self.confirmed_receivers.lock().unwrap();
        let mut hash = Sha256::new();
        for identity in receivers.keys() {
            hash.update((identity.len() as u64).to_be_bytes());
            hash.update(identity.as_bytes());
        }
        Some((
            hash.finalize().into(),
            receivers.keys().all(|key| confirmed.contains(key)),
        ))
    }

    pub(super) fn confirm_receiver_binding(&self, identity: &str) {
        let receivers = self.receivers.lock().unwrap();
        if self.is_current() && receivers.contains_key(identity) {
            self.confirmed_receivers
                .lock()
                .unwrap()
                .insert(identity.to_owned());
        }
    }
}
