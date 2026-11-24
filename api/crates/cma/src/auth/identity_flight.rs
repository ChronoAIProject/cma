//! Process-local invalidation fences and the opt-in positive-cache prototype.
use ring::{hmac, rand::SystemRandom};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, SystemTime},
};

use tokio::time::Instant;

use super::{
    identity::VerifiedSubject,
    identity_evidence::{self, EvidenceScope, EvidenceStamp},
    verification_error::ProviderVerificationError,
    ProviderMetadata,
};

const CAPACITY: usize = 1024;
const POSITIVE_CAPACITY: usize = 128;
pub(super) const POSITIVE_TTL: Duration = Duration::from_secs(30);
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
    fn cached_positive(
        &self,
        _key: &TokenHash,
        _metadata: &ProviderMetadata,
        _expiry: Option<SystemTime>,
        _expected_subject: Option<&str>,
    ) -> Option<Arc<VerifiedSubject>> {
        None
    }
    fn remember_positive(
        &self,
        _key: &TokenHash,
        _epoch: &Epoch,
        _metadata: ProviderMetadata,
        _expiry: Option<SystemTime>,
        _evidence: Arc<VerifiedSubject>,
    ) {
    }
    fn positive_cache_enabled(&self) -> bool {
        false
    }
    fn invalidate_token(&self, token: &str);
    fn clear(&self);
    #[cfg(test)]
    fn active_receipts(&self) -> usize;
}

struct Entry {
    epoch: Weak<()>,
    stamp: Option<EvidenceStamp>,
    positive: Option<Positive>,
}

struct Positive {
    metadata: ProviderMetadata,
    expires_at: Option<SystemTime>,
    until: Instant,
    evidence: Arc<VerifiedSubject>,
}

impl Positive {
    fn alive(&self) -> bool {
        Instant::now() < self.until
            && self
                .expires_at
                .is_none_or(|expiry| SystemTime::now() < expiry)
    }
}

pub(super) struct MemoryIdentityStore {
    key: hmac::Key,
    capacity: usize,
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
            if entry
                .positive
                .as_ref()
                .is_some_and(|cached| !cached.alive())
            {
                entry.positive = None;
            }
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
            positive: None,
        };
        Some(completed)
    }

    fn positive_cache_enabled(&self) -> bool {
        self.positive_cache_enabled
    }

    fn cached_positive(
        &self,
        key: &TokenHash,
        metadata: &ProviderMetadata,
        expiry: Option<SystemTime>,
        expected_subject: Option<&str>,
    ) -> Option<Arc<VerifiedSubject>> {
        let epochs = self.epochs.lock().expect("identity fence lock");
        let cached = epochs.get(key)?.positive.as_ref()?;
        if !cached.alive()
            || &cached.metadata != metadata
            || expiry.is_some_and(|expiry| cached.expires_at.is_none_or(|older| older > expiry))
            || expected_subject.is_some_and(|subject| cached.evidence.subject.as_str() != subject)
        {
            return None;
        }
        Some(cached.evidence.clone())
    }

    fn remember_positive(
        &self,
        key: &TokenHash,
        epoch: &Epoch,
        metadata: ProviderMetadata,
        expiry: Option<SystemTime>,
        evidence: Arc<VerifiedSubject>,
    ) {
        use super::permissions::PermissionEvidence;
        if !self.positive_cache_enabled
            || !matches!(&evidence.permissions, PermissionEvidence::Active(_))
        {
            return;
        }
        let mut epochs = self.epochs.lock().expect("identity fence lock");
        let occupied = epochs
            .values()
            .filter(|entry| entry.positive.is_some())
            .count();
        let Some(current) = epochs.get_mut(key) else {
            return;
        };
        if !current.epoch.ptr_eq(&Arc::downgrade(epoch))
            || (occupied >= POSITIVE_CAPACITY && current.positive.is_none())
        {
            return;
        }
        current.positive = Some(Positive {
            metadata,
            expires_at: expiry,
            until: Instant::now() + POSITIVE_TTL,
            evidence,
        });
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
