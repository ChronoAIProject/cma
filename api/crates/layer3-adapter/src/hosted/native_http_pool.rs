//! Retain streams, never resource authority. Checkout always precedes a fresh Pod proof.
use super::*;
use crate::agent_sandbox::PodBinding;
use std::sync::Mutex as StdMutex;
use tokio::sync::{watch, OwnedMutexGuard};

pub(super) const MAX_HTTP_SESSIONS: usize = 16;
pub(super) const HTTP_IDLE_TTL: Duration = Duration::from_secs(60);

pub(super) struct AbortOnDrop(pub tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) struct NativeConnection {
    pub sender: hyper::client::conn::http1::SendRequest<http_body_util::Full<bytes::Bytes>>,
    pub _driver: AbortOnDrop,
}

pub(super) struct NativeHttpSlot {
    pub identity: NativeRuntimeIdentity,
    pub binding: PodBinding,
    connection: Arc<Mutex<Option<NativeConnection>>>,
    cancelled: watch::Sender<bool>,
    idle_timer: StdMutex<Option<AbortOnDrop>>,
    last_used: StdMutex<std::time::Instant>,
}

impl NativeHttpSlot {
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
        self.idle_timer.lock().unwrap().take();
    }

    pub fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    fn reusable(&self) -> bool {
        self.identity.request_sha256.is_some()
            && self.identity.sandbox_id.is_some()
            && self
                .binding
                .runtime_container_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
            && self
                .binding
                .runtime_restart_count
                .is_some_and(|count| count >= 0)
    }
}

impl Drop for NativeHttpSlot {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The connection leaves the cache before any await that can send/read bytes.
/// Cancellation or any error drops this guard and aborts its owning exec task.
pub(super) struct NativeLease {
    pub connection: Option<NativeConnection>,
    pub slot: Arc<NativeHttpSlot>,
    handle: RuntimeHandle,
    state: Arc<Mutex<TransportState>>,
    checkout: OwnedMutexGuard<Option<NativeConnection>>,
}

impl NativeLease {
    pub async fn current(&self) -> crate::Result<()> {
        let state = self.state.lock().await;
        if identity(&state, &self.handle) != self.slot.identity || *self.slot.cancelled.borrow() {
            return Err(retired());
        }
        Ok(())
    }

    pub async fn recycle(self) -> crate::Result<()> {
        self.recycle_with_idle_ttl(HTTP_IDLE_TTL).await
    }

    pub async fn recycle_with_idle_ttl(mut self, idle_ttl: Duration) -> crate::Result<()> {
        self.current().await?;
        if !self.slot.reusable()
            || self
                .connection
                .as_ref()
                .is_none_or(|connection| connection.sender.is_closed())
        {
            return Ok(());
        }
        let state = self.state.lock().await;
        if identity(&state, &self.handle) != self.slot.identity
            || !state
                .native_http
                .get(&self.handle)
                .is_some_and(|slot| Arc::ptr_eq(slot, &self.slot))
        {
            return Err(retired());
        }
        *self.checkout = self.connection.take();
        *self.slot.last_used.lock().unwrap() = std::time::Instant::now();
        let weak_state = Arc::downgrade(&self.state);
        let weak_slot = Arc::downgrade(&self.slot);
        let handle = self.handle.clone();
        *self.slot.idle_timer.lock().unwrap() = Some(AbortOnDrop(tokio::spawn(async move {
            tokio::time::sleep(idle_ttl).await;
            let (Some(state), Some(slot)) = (weak_state.upgrade(), weak_slot.upgrade()) else {
                return;
            };
            let mut state = state.lock().await;
            // A checked-out/queued operation cannot lose its entry to an idle timer.
            if slot.connection.try_lock().is_ok()
                && state
                    .native_http
                    .get(&handle)
                    .is_some_and(|current| Arc::ptr_eq(current, &slot))
            {
                state.native_http.remove(&handle);
                slot.cancel();
            }
        })));
        Ok(())
    }
}

impl ProviderHostedControlTransport {
    pub(super) async fn native_endpoint_state(
        &self,
        handle: &RuntimeHandle,
    ) -> (NativeRuntimeIdentity, Option<PodBinding>) {
        let state = self.state.lock().await;
        let identity = identity(&state, handle);
        // This is only a routing hint. The exclusive checkout freshly observes
        // and compares the complete Pod incarnation before every reused send.
        let hint = state
            .native_http
            .get(handle)
            .filter(|slot| slot.identity == identity && slot.reusable())
            .map(|slot| slot.binding.clone());
        (identity, hint)
    }

    pub(super) async fn native_slot(
        &self,
        endpoint: &RuntimeEndpoint,
    ) -> crate::Result<Arc<NativeHttpSlot>> {
        let (handle, binding) = endpoint.native_binding().ok_or_else(retired)?;
        let expected = endpoint.native_identity().ok_or_else(retired)?;
        let mut state = self.state.lock().await;
        if identity(&state, handle) != *expected {
            return Err(retired());
        }
        if let Some(slot) = state.native_http.get(handle) {
            if slot.identity == *expected && slot.binding == *binding {
                return Ok(slot.clone());
            }
            if let Some(slot) = state.native_http.remove(handle) {
                slot.cancel();
            }
        }
        if state.native_http.len() >= MAX_HTTP_SESSIONS {
            // Evict the least recently used idle resource before opening another.
            // Resource-name order can repeatedly discard hot prepared runtimes.
            // The client still enforces its shared 32 request-stream permits.
            let idle = state
                .native_http
                .iter()
                .filter(|(_, slot)| {
                    Arc::strong_count(slot) == 1 && slot.connection.try_lock().is_ok()
                })
                .min_by_key(|(_, slot)| *slot.last_used.lock().unwrap())
                .map(|(handle, _)| handle.clone());
            if let Some(idle) = idle {
                state.native_http.remove(&idle);
            } else {
                return Err(AdapterError::AtCapacity(
                    "native HTTP sessions are busy".into(),
                ));
            }
        }
        let slot = Arc::new(NativeHttpSlot {
            identity: expected.clone(),
            binding: binding.clone(),
            connection: Arc::new(Mutex::new(None)),
            cancelled: watch::channel(false).0,
            idle_timer: StdMutex::new(None),
            last_used: StdMutex::new(std::time::Instant::now()),
        });
        state.native_http.insert(handle.clone(), slot.clone());
        Ok(slot)
    }

    pub(super) async fn native_checkout(
        &self,
        endpoint: &RuntimeEndpoint,
        slot: Arc<NativeHttpSlot>,
    ) -> crate::Result<NativeLease> {
        let waiting = std::time::Instant::now();
        let mut checkout = crate::binding_diagnostics::measure(
            crate::binding_diagnostics::Stage::TransportWait,
            slot.connection.clone().lock_owned(),
            |_| crate::binding_diagnostics::Outcome::Complete,
        )
        .await;
        let wait_ms = u64::try_from(waiting.elapsed().as_millis()).unwrap_or(u64::MAX);
        slot.idle_timer.lock().unwrap().take();
        let lease = NativeLease {
            connection: checkout.take(),
            slot,
            handle: endpoint.native_binding().ok_or_else(retired)?.0.clone(),
            state: self.state.clone(),
            checkout,
        };
        lease.current().await?;
        let reused_connection = lease
            .connection
            .as_ref()
            .is_some_and(|connection| !connection.sender.is_closed());
        // Retain only bounded identity/timing metadata, never requests, headers,
        // tokens or runtime responses. Suppress uncontended sub-millisecond reuse.
        if wait_ms > 0 || !reused_connection {
            tracing::info!(
                runtime_id = lease.handle.as_str(),
                wait_ms,
                reused_connection,
                "Native runtime HTTP connection checked out"
            );
        }
        Ok(lease)
    }

    pub(super) async fn discard_native_slot(
        &self,
        endpoint: &RuntimeEndpoint,
        slot: &Arc<NativeHttpSlot>,
    ) {
        let Some((handle, _)) = endpoint.native_binding() else {
            return;
        };
        let mut state = self.state.lock().await;
        if state
            .native_http
            .get(handle)
            .is_some_and(|current| Arc::ptr_eq(current, slot))
        {
            state.native_http.remove(handle);
            slot.cancel();
        }
    }
}

fn identity(state: &TransportState, handle: &RuntimeHandle) -> NativeRuntimeIdentity {
    let bound = state.bindings.get(handle);
    NativeRuntimeIdentity {
        epoch: state.runtime_epochs.get(handle).copied().unwrap_or(0),
        sandbox_id: bound.map(|binding| binding.sandbox_id.clone()),
        request_sha256: bound.and_then(|binding| binding.request_sha256.clone()),
    }
}

pub(super) fn retired() -> AdapterError {
    AdapterError::Unavailable("native runtime transport was retired or rebound".into())
}
