//! Process-local invalidation fences; no verification result is retained here.
use ring::{hmac, rand::SystemRandom};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

const CAPACITY: usize = 1024;
pub(super) type TokenHash = [u8; 32];
pub(super) type Epoch = Arc<()>;

pub(super) trait IdentityStore: Send + Sync {
    fn key(&self, token: &str) -> TokenHash;
    fn token_epoch(&self, key: &TokenHash) -> Option<Epoch>;
    fn token_current(&self, key: &TokenHash, epoch: &Epoch) -> bool;
    fn finish_verification(&self, key: &TokenHash, epoch: &Epoch) -> Option<Epoch>;
    fn invalidate_token(&self, token: &str);
    fn clear(&self);
    #[cfg(test)]
    fn active_receipts(&self) -> usize;
}

pub(super) struct MemoryIdentityStore {
    key: hmac::Key,
    capacity: usize,
    epochs: Mutex<HashMap<TokenHash, Weak<()>>>,
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
        epochs.retain(|_, epoch| epoch.strong_count() > 0);
        if let Some(epoch) = epochs.get(key).and_then(Weak::upgrade) {
            return Some(epoch);
        }
        if epochs.len() >= self.capacity {
            return None;
        }
        let epoch = Arc::new(());
        epochs.insert(*key, Arc::downgrade(&epoch));
        Some(epoch)
    }

    fn token_current(&self, key: &TokenHash, epoch: &Epoch) -> bool {
        self.epochs
            .lock()
            .expect("identity fence lock")
            .get(key)
            .is_some_and(|current| current.ptr_eq(&Arc::downgrade(epoch)))
    }

    fn finish_verification(&self, key: &TokenHash, epoch: &Epoch) -> Option<Epoch> {
        let mut epochs = self.epochs.lock().expect("identity fence lock");
        if !epochs
            .get(key)
            .is_some_and(|current| current.ptr_eq(&Arc::downgrade(epoch)))
        {
            return None;
        }
        // New verified authority supersedes an older receipt still held during
        // authorization IO. Only its admitted consumers hold this marker alive.
        let completed = Arc::new(());
        epochs.insert(*key, Arc::downgrade(&completed));
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

    #[cfg(test)]
    fn active_receipts(&self) -> usize {
        self.epochs
            .lock()
            .expect("identity fence lock")
            .values()
            .filter(|epoch| epoch.strong_count() > 0)
            .count()
    }
}

#[cfg(test)]
#[path = "identity_flight_tests.rs"]
mod tests;
