//! Process-local invalidation fences. Only test builds can retain completed results.
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

#[cfg(test)]
use super::ProviderMetadata;

const CAPACITY: usize = 1024;
#[cfg(test)]
const POSITIVE_CAPACITY: usize = 128;
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
    #[cfg(test)]
    fn positive_cache(&self) -> Option<&MemoryIdentityStore> {
        None
    }
    fn invalidate_token(&self, token: &str);
    fn clear(&self);
    #[cfg(test)]
    fn active_receipts(&self) -> usize;
}

struct Entry {
    epoch: Weak<()>,
    stamp: Option<EvidenceStamp>,
    #[cfg(test)]
    positive: Option<(ProviderMetadata, Arc<VerifiedSubject>)>,
}

pub(super) struct MemoryIdentityStore {
    key: hmac::Key,
    capacity: usize,
    #[cfg(test)]
    positive_cache_enabled: bool,
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
            #[cfg(test)]
            positive_cache_enabled: false,
            epochs: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(super) fn positive_cache_prototype() -> Self {
        Self {
            positive_cache_enabled: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl MemoryIdentityStore {
    pub(super) fn cached_positive(
        &self,
        key: &TokenHash,
        metadata: &ProviderMetadata,
        expiry: Option<std::time::SystemTime>,
        owner: Option<&str>,
    ) -> Option<Arc<VerifiedSubject>> {
        let epochs = self.epochs.lock().expect("identity fence lock");
        let (scope, evidence) = epochs.get(key)?.positive.as_ref()?;
        (scope == metadata
            && owner.is_none_or(|owner| evidence.subject.as_str() == owner)
            && super::identity_lifetime::cache_fresh(evidence, expiry))
        .then(|| evidence.clone())
    }

    pub(super) fn remember_positive(
        &self,
        key: &TokenHash,
        epoch: &Epoch,
        metadata: ProviderMetadata,
        evidence: Arc<VerifiedSubject>,
    ) {
        let mut epochs = self.epochs.lock().expect("identity fence lock");
        let occupied = epochs
            .values()
            .filter(|entry| entry.positive.is_some())
            .count();
        let Some(entry) = epochs.get_mut(key) else {
            return;
        };
        if entry.epoch.ptr_eq(&Arc::downgrade(epoch))
            && super::identity_lifetime::cache_fresh(&evidence, None)
            && (occupied < POSITIVE_CAPACITY || entry.positive.is_some())
        {
            entry.positive = Some((metadata, evidence));
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
        epochs.retain(|_, entry| {
            #[cfg(test)]
            entry
                .positive
                .take_if(|(_, evidence)| !super::identity_lifetime::cache_fresh(evidence, None));
            entry.epoch.strong_count() > 0
        });
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
                #[cfg(test)]
                positive: None,
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
            #[cfg(test)]
            positive: None,
        };
        Some(completed)
    }

    #[cfg(test)]
    fn positive_cache(&self) -> Option<&MemoryIdentityStore> {
        self.positive_cache_enabled.then_some(self)
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

    #[cfg(test)]
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
