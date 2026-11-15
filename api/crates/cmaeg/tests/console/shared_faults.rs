//! Faults wrap actual Mongo operations, including a committed write whose
//! response is lost. No alternate storage implementation supplies authority.
use super::fake_nyxid::console::barrier::ResponseGate;
use cmaeg::console::{
    session::SessionLimits,
    session_backend::{SessionBackend, StoreError, Stored},
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

pub struct Faults {
    pub inner: Arc<dyn SessionBackend>,
    pub unavailable: AtomicBool,
    pub lose_completion: AtomicBool,
    pub fail_readback: AtomicBool,
    pub lose_claim: AtomicBool,
    pub completion_gate: Mutex<Option<Arc<ResponseGate>>>,
    pub observation_delay_ms: AtomicU64,
}

impl Faults {
    pub fn new(inner: Arc<dyn SessionBackend>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            unavailable: AtomicBool::new(false),
            lose_completion: AtomicBool::new(false),
            fail_readback: AtomicBool::new(false),
            lose_claim: AtomicBool::new(false),
            completion_gate: Mutex::new(None),
            observation_delay_ms: AtomicU64::new(0),
        })
    }
}

impl SessionBackend for Faults {
    fn create<'a>(
        &'a self,
        id: &'a str,
        subject: &'a str,
        payload: Vec<u8>,
        ttl: i64,
        limits: &'a SessionLimits,
    ) -> BoxFuture<'a, Result<Stored, StoreError>> {
        self.inner.create(id, subject, payload, ttl, limits)
    }
    fn load<'a>(
        &'a self,
        id: &'a str,
        touch: bool,
    ) -> BoxFuture<'a, Result<Option<Stored>, StoreError>> {
        Box::pin(async move {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(StoreError::Unavailable);
            }
            let result = self.inner.load(id, touch).await;
            tokio::time::sleep(std::time::Duration::from_millis(
                self.observation_delay_ms.load(Ordering::SeqCst),
            ))
            .await;
            result
        })
    }
    fn compare_exchange<'a>(
        &'a self,
        id: &'a str,
        expected: &'a [u8],
        next: Vec<u8>,
        expiry: Option<i64>,
    ) -> BoxFuture<'a, Result<Option<Stored>, StoreError>> {
        Box::pin(async move {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(StoreError::Unavailable);
            }
            let changed = next != expected;
            let result = self
                .inner
                .compare_exchange(id, expected, next, expiry)
                .await;
            if result.as_ref().is_ok_and(Option::is_some) {
                if expiry.is_some() && self.lose_completion.swap(false, Ordering::SeqCst) {
                    let gate = self.completion_gate.lock().unwrap().clone();
                    if let Some(gate) = gate {
                        gate.hold().await;
                    }
                    if self.fail_readback.load(Ordering::SeqCst) {
                        self.unavailable.store(true, Ordering::SeqCst);
                    }
                    return Err(StoreError::Unavailable);
                }
                if changed && expiry.is_none() && self.lose_claim.swap(false, Ordering::SeqCst) {
                    return Err(StoreError::Unavailable);
                }
            }
            result
        })
    }
    fn admit<'a>(
        &'a self,
        id: &'a str,
        expected: &'a [u8],
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(StoreError::Unavailable);
            }
            self.inner.admit(id, expected).await
        })
    }
    fn revoke<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, StoreError>> {
        Box::pin(async move {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(StoreError::Unavailable);
            }
            self.inner.revoke(id).await
        })
    }
}
