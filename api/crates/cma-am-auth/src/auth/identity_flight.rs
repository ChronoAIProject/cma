//! Process-local invalidation fences; no verification result is retained here.
use ring::{hmac, rand::SystemRandom};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use super::{
    identity::VerifiedSubject,
    identity_evidence::{self, EvidenceScope, EvidenceStamp},
    verification_error::ProviderVerificationError,
};

const CAPACITY: usize = 1024;
pub(super) type TokenHash = [u8; 32];
pub(super) type Epoch = Arc<()>;

pub(super) trait IdentityStore: Send + Sync {
    fn key(&self, token: &str) -> TokenHash;
    fn token_epoch(&self, key: &TokenHash) -> Option<Epoch>;
    fn token_current(&self, key: &TokenHash, epoch: &Epoch) -> bool;
    fn evidence_stamp(
        &self,
        key: &TokenHash,
        scope: EvidenceScope<'_>,
        evidence: &VerifiedSubject,
    ) -> Result<EvidenceStamp, ProviderVerificationError>;
    fn finish_verification(
        &self,
        key: &TokenHash,
        epoch: &Epoch,
        stamp: &EvidenceStamp,
    ) -> Option<Epoch>;
    fn invalidate_token(&self, token: &str);
    fn clear(&self);
    #[cfg(any(test, feature = "test-support"))]
    fn active_receipts(&self) -> usize;
}

struct Entry {
    epoch: Weak<()>,
    stamp: Option<EvidenceStamp>,
}

pub(super) struct MemoryIdentityStore {
    key: hmac::Key,
    capacity: usize,
    epochs: Mutex<HashMap<TokenHash, Entry>>,
}

impl Default for MemoryIdentityStore {
    fn default() -> Self {
        Self::new(CAPACITY)
    }
}

impl MemoryIdentityStore {
    fn new(capacity: usize) -> Self {
        Self {
            key: hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new())
                .expect("operating system randomness is required for identity hashing"),
            capacity,
            epochs: Mutex::new(HashMap::new()),
        }
    }
}

impl IdentityStore for MemoryIdentityStore {
    fn key(&self, token: &str) -> TokenHash {
        let mut context = hmac::Context::with_key(&self.key);
        context.update(b"cma-identity-flight-v1\0");
        context.update(token.as_bytes());
        context.sign().as_ref().try_into().expect("SHA-256 length")
    }

    fn token_epoch(&self, key: &TokenHash) -> Option<Epoch> {
        let mut epochs = self.epochs.lock().expect("identity fence lock");
        epochs.retain(|_, entry| entry.epoch.strong_count() > 0);
        if let Some(epoch) = epochs.get(key).and_then(|entry| entry.epoch.upgrade()) {
            return Some(epoch);
        }
        if epochs.len() >= self.capacity {
            return None;
        }
        let epoch = Arc::new(());
        epochs.insert(
            *key,
            Entry {
                epoch: Arc::downgrade(&epoch),
                stamp: None,
            },
        );
        Some(epoch)
    }

    fn token_current(&self, key: &TokenHash, epoch: &Epoch) -> bool {
        self.epochs
            .lock()
            .expect("identity fence lock")
            .get(key)
            .is_some_and(|current| current.epoch.ptr_eq(&Arc::downgrade(epoch)))
    }

    fn evidence_stamp(
        &self,
        key: &TokenHash,
        scope: EvidenceScope<'_>,
        evidence: &VerifiedSubject,
    ) -> Result<EvidenceStamp, ProviderVerificationError> {
        identity_evidence::stamp(&self.key, key, scope, evidence)
    }

    fn finish_verification(
        &self,
        key: &TokenHash,
        epoch: &Epoch,
        stamp: &EvidenceStamp,
    ) -> Option<Epoch> {
        let mut epochs = self.epochs.lock().expect("identity fence lock");
        let current = epochs.get_mut(key)?;
        // Check capture identity before equality: revocation and A→B→A must
        // never let a stale publisher adopt or resurrect replacement authority.
        if !current.epoch.ptr_eq(&Arc::downgrade(epoch)) {
            return None;
        }
        if current
            .stamp
            .as_ref()
            .is_none_or(|previous| previous == stamp)
        {
            current.stamp = Some(stamp.clone());
            return Some(epoch.clone());
        }
        let completed = Arc::new(());
        *current = Entry {
            epoch: Arc::downgrade(&completed),
            stamp: Some(stamp.clone()),
        };
        Some(completed)
    }

    fn invalidate_token(&self, token: &str) {
        self.epochs
            .lock()
            .expect("identity fence lock")
            .remove(&self.key(token));
    }

    fn clear(&self) {
        self.epochs.lock().expect("identity fence lock").clear();
    }

    #[cfg(any(test, feature = "test-support"))]
    fn active_receipts(&self) -> usize {
        self.epochs
            .lock()
            .expect("identity fence lock")
            .values()
            .filter(|entry| entry.epoch.strong_count() > 0)
            .count()
    }
}

#[cfg(test)]
#[path = "identity_flight_tests.rs"]
mod tests;
