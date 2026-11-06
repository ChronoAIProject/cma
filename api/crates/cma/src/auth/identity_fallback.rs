//! Process-local successful evidence. No raw credentials or durable storage.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::http::Method;
use ring::{hmac, rand::SystemRandom};
use tokio::time::Instant;

use super::identity::VerifiedSubject;

pub(super) const MAX_AGE: Duration = Duration::from_secs(60);
const CAPACITY: usize = 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024;
pub(super) type TokenHash = [u8; 32];
// A fresh allocation fences in-flight work without a wrapping generation counter.
pub(super) type Epoch = Arc<()>;

pub(super) trait FallbackPolicy: Send + Sync {
    fn permits(&self, method: &Method, path: &str) -> bool;
}

pub(super) struct SessionReadOnly;
impl FallbackPolicy for SessionReadOnly {
    fn permits(&self, method: &Method, path: &str) -> bool {
        method == Method::GET && path == "/auth/session"
    }
}

pub(super) trait IdentityStore: Send + Sync {
    fn key(&self, token: &str) -> TokenHash;
    fn epoch(&self) -> Epoch;
    fn remember(
        &self,
        key: TokenHash,
        epoch: &Epoch,
        evidence: Arc<VerifiedSubject>,
        expiry: SystemTime,
    );
    fn get(&self, key: &TokenHash, epoch: &Epoch) -> Option<Arc<VerifiedSubject>>;
    fn invalidate(&self, key: &TokenHash);
    fn clear(&self);
}

struct Record {
    evidence: Arc<VerifiedSubject>,
    verified_at: Instant,
    expires_at: SystemTime,
    expiry_deadline: Instant,
}

struct Records {
    epoch: Epoch,
    entries: HashMap<TokenHash, Record>,
}

pub(super) struct MemoryIdentityStore {
    key: hmac::Key,
    capacity: usize,
    records: Mutex<Records>,
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
            records: Mutex::new(Records {
                epoch: Arc::new(()),
                entries: HashMap::new(),
            }),
        }
    }
}

impl IdentityStore for MemoryIdentityStore {
    fn key(&self, token: &str) -> TokenHash {
        hmac::sign(&self.key, token.as_bytes())
            .as_ref()
            .try_into()
            .expect("SHA-256 length")
    }

    fn epoch(&self) -> Epoch {
        self.records
            .lock()
            .expect("identity store lock")
            .epoch
            .clone()
    }

    fn remember(
        &self,
        key: TokenHash,
        epoch: &Epoch,
        evidence: Arc<VerifiedSubject>,
        expiry: SystemTime,
    ) {
        let mut records = self.records.lock().expect("identity store lock");
        if !Arc::ptr_eq(epoch, &records.epoch) || expiry <= SystemTime::now() || self.capacity == 0
        {
            return;
        }
        if retained_bytes(&evidence) > MAX_RECORD_BYTES {
            records.entries.remove(&key);
            records.epoch = Arc::new(());
            return;
        }
        records.entries.retain(|_, record| record.valid());
        if records.entries.len() >= self.capacity && !records.entries.contains_key(&key) {
            if let Some(oldest) = records
                .entries
                .iter()
                .min_by_key(|(_, record)| record.verified_at)
                .map(|(key, _)| *key)
            {
                records.entries.remove(&oldest);
            }
        }
        records.entries.insert(
            key,
            Record {
                evidence,
                verified_at: Instant::now(),
                expires_at: expiry,
                expiry_deadline: Instant::now()
                    + expiry
                        .duration_since(SystemTime::now())
                        .unwrap_or_default()
                        .min(MAX_AGE + Duration::from_secs(1)),
            },
        );
    }

    fn get(&self, key: &TokenHash, epoch: &Epoch) -> Option<Arc<VerifiedSubject>> {
        let mut records = self.records.lock().expect("identity store lock");
        if !Arc::ptr_eq(epoch, &records.epoch) {
            return None;
        }
        records.entries.retain(|_, record| record.valid());
        records
            .entries
            .get(key)
            .map(|record| record.evidence.clone())
    }

    fn invalidate(&self, key: &TokenHash) {
        let mut records = self.records.lock().expect("identity store lock");
        records.entries.remove(key);
        records.epoch = Arc::new(());
    }

    fn clear(&self) {
        let mut records = self.records.lock().expect("identity store lock");
        records.entries.clear();
        records.epoch = Arc::new(());
    }
}

fn retained_bytes(evidence: &VerifiedSubject) -> usize {
    let mut bytes = std::mem::size_of::<VerifiedSubject>() + evidence.subject.as_str().len();
    for value in [
        &evidence.profile.display_name,
        &evidence.profile.email,
        &evidence.profile.avatar_url,
    ]
    .into_iter()
    .flatten()
    {
        bytes = bytes.saturating_add(value.capacity());
    }
    if let super::permissions::PermissionEvidence::Active(names) = &evidence.permissions {
        bytes = bytes.saturating_add(
            names
                .capacity()
                .saturating_mul(std::mem::size_of::<String>()),
        );
        for name in names {
            bytes = bytes.saturating_add(name.capacity());
        }
    }
    bytes
}

impl Record {
    fn valid(&self) -> bool {
        self.verified_at.elapsed() <= MAX_AGE
            && self.expires_at > SystemTime::now()
            && Instant::now() < self.expiry_deadline
    }
}

pub(super) fn report(key: &TokenHash, reason: &'static str) {
    // Process-keyed, truncated HMAC; never a subject, raw token or provider body.
    let token_hash: String = key[..12].iter().map(|byte| format!("{byte:02x}")).collect();
    tracing::warn!(
        event = "identity_fallback",
        token_hash,
        reason,
        count = 1u64,
        "Identity fallback accepted for session inspection"
    );
}

#[cfg(test)]
#[path = "identity_fallback_tests.rs"]
mod tests;
