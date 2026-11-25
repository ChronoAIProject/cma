//! Fenced desktop streams renew through an independent receiver control port.
use super::*;
use crate::sandbox::store::{DesktopSessionRequest, SandboxSessionStore, SandboxStoreError};
use layer3_adapter::{DesktopOwnership, DesktopOwnershipChallenge, DesktopOwnershipControl};
use tokio::{sync::watch, task::JoinHandle, time::Instant};

pub(super) struct ConnectionState {
    authority: Arc<Mutex<DesktopOwnership>>,
    control: Arc<dyn DesktopOwnershipControl>,
    validity: watch::Receiver<Option<Instant>>,
    renewal: Option<JoinHandle<()>>,
}

impl Drop for ConnectionState {
    fn drop(&mut self) {
        if let Some(renewal) = &self.renewal {
            renewal.abort();
        }
    }
}

impl ConnectionState {
    pub(super) fn valid(&self) -> bool {
        self.deadline().is_ok()
    }

    fn deadline(&self) -> Result<Instant, DesktopError> {
        self.validity
            .has_changed()
            .map_err(|_| DesktopError::Unavailable)?;
        self.validity
            .borrow()
            .filter(|deadline| *deadline > Instant::now())
            .ok_or(DesktopError::Unavailable)
    }

    pub(super) async fn while_authorized<T>(
        &mut self,
        operation: impl std::future::Future<Output = T>,
    ) -> Result<T, DesktopError> {
        tokio::pin!(operation);
        loop {
            let deadline = self.deadline()?;
            tokio::select! {
                biased;
                // A confirmed renewal can already have replaced this deadline.
                // Recheck the current authority before polling I/O again.
                changed = self.validity.changed() => {
                    changed.map_err(|_| DesktopError::Unavailable)?;
                },
                _ = tokio::time::sleep_until(deadline) => {
                    self.deadline()?;
                },
                result = &mut operation => {
                    self.deadline()?;
                    return Ok(result);
                },
            }
        }
    }

    pub(super) async fn send(
        &mut self,
        connection: &mut DesktopConnectionHandle,
        frame: DesktopFrame,
    ) -> Result<(), DesktopError> {
        self.while_authorized(connection.send(frame))
            .await?
            .map_err(DesktopError::adapter)
    }

    pub(super) async fn recv(
        &mut self,
        connection: &mut DesktopConnectionHandle,
    ) -> Result<Option<DesktopFrame>, DesktopError> {
        self.while_authorized(connection.recv())
            .await?
            .map_err(|_| DesktopError::RuntimeUnavailable)
    }

    pub(super) async fn close(&mut self, reads: &AgentReadService) {
        // The exact claim survives an ambiguous renew; revoke covers its epoch,
        // not a guessed latest expiry. Await cancellation before taking the token.
        if let Some(renewal) = self.renewal.take() {
            renewal.abort();
            if let Err(error) = renewal.await {
                if !error.is_cancelled() {
                    tracing::warn!("desktop renewal task failed during close");
                }
            }
        }
        let authority = self.authority.lock().await.clone();
        let revoked = tokio::time::timeout(
            Duration::from_secs(2),
            self.control.revoke_ownership(&authority),
        )
        .await;
        if !matches!(revoked, Ok(Ok(()))) {
            tracing::warn!("desktop revocation uncertain; retaining claim until expiry");
            return;
        }
        match reads.session_store() {
            Ok(store) => release(&store, &authority).await,
            Err(_) => tracing::warn!("desktop session store unavailable during close"),
        }
    }
}

impl DesktopService {
    pub(super) async fn open_owned(
        &self,
        caller: &Caller,
        id: &AgentId,
        sandbox_id: SandboxId,
        runtime: RuntimeBinding,
        authorization: Arc<dyn DesktopAuthorization>,
    ) -> Result<DesktopConnection, DesktopError> {
        let migration_guard = if self.migration_tracking {
            let guard =
                try_acquire_desktop_runtime_guard(&sandbox_id).map_err(|_| DesktopError::Busy)?;
            let (current_id, current_runtime) = self.authorize(caller, id).await?;
            if current_id != sandbox_id || current_runtime != runtime {
                return Err(DesktopError::Unavailable);
            }
            Some(guard)
        } else {
            None
        };
        let store = self
            .reads
            .session_store()
            .map_err(|_| DesktopError::AuthorityUnavailable)?;
        let request = DesktopSessionRequest {
            sandbox_id,
            runtime,
            session_id: SandboxCommandId::generate().to_string(),
        };
        let handle = RuntimeHandle::new(request.runtime.runtime_id.clone());
        let started = Instant::now();
        let challenge = timing::observe("receiver_challenge", async {
            tokio::time::timeout(
                Duration::from_secs(3),
                self.adapter.desktop_ownership_challenge(&handle),
            )
            .await
            .map_err(|_| DesktopError::RuntimeUnavailable)?
            .map_err(DesktopError::adapter)
        })
        .await?;
        let deadline = deadline(started, &challenge)?;
        let authority = timing::observe("durable_claim", async {
            tokio::time::timeout_at(deadline, store.acquire_exclusive_sandbox_session(&request))
                .await
                .map_err(|_| DesktopError::AuthorityUnavailable)?
                .map_err(|error| match error {
                    SandboxStoreError::Conflict => DesktopError::Busy,
                    _ => DesktopError::AuthorityUnavailable,
                })
        })
        .await?;
        let opened = timing::observe("runtime_open", async {
            tokio::time::timeout_at(
                deadline,
                self.adapter
                    .open_desktop_owned(&handle, &authority, &challenge),
            )
            .await
            .map_err(|_| {
                layer3_adapter::AdapterError::Unavailable("desktop open deadline elapsed".into())
            })?
        })
        .await;
        let connection = match opened {
            Ok(connection) => connection,
            Err(layer3_adapter::AdapterError::Unsupported(_)) => {
                // Unsupported is a definite no-effect outcome. A timeout or
                // transport error may have installed authority at the receiver.
                release(&store, &authority).await;
                return Err(DesktopError::RuntimeUnavailable);
            }
            Err(error) => return Err(DesktopError::adapter(error)),
        };
        let control = connection
            .ownership_control()
            .ok_or(DesktopError::RuntimeUnavailable)?;
        // An open may wait for provider I/O. Recheck the actual caller and exact
        // binding before exposing its socket; heartbeat revalidation follows.
        let current = tokio::time::timeout_at(deadline, async {
            let (_, binding) = tokio::try_join!(
                timing::observe("post_open_credential", authorization.current(caller)),
                timing::observe("post_open_lineage", self.authorize(caller, id)),
            )?;
            Ok::<_, DesktopError>(binding)
        })
        .await;
        if !matches!(current, Ok(Ok((ref sandbox_id, ref runtime)))
            if sandbox_id == &request.sandbox_id && runtime == &request.runtime)
        {
            if matches!(
                tokio::time::timeout(Duration::from_secs(2), control.revoke_ownership(&authority))
                    .await,
                Ok(Ok(()))
            ) {
                release(&store, &authority).await;
            } else {
                tracing::warn!("desktop post-open revocation uncertain; retaining claim");
            }
            return Err(DesktopError::Unavailable);
        }
        if let Some(guard) = &migration_guard {
            guard
                .opened()
                .map_err(|_| DesktopError::AuthorityUnavailable)?;
        }
        let authority = Arc::new(Mutex::new(authority));
        let (validity, receiver) = watch::channel(Some(deadline));
        let renewal = tokio::spawn(renew(
            self.clone(),
            RenewalCaller {
                caller: caller.clone(),
                id: id.clone(),
                authorization,
            },
            request.clone(),
            control.clone(),
            authority.clone(),
            validity,
        ));
        Ok(DesktopConnection {
            connection: Some(connection),
            sandbox_id: request.sandbox_id,
            session_id: request.session_id,
            runtime_id: request.runtime.runtime_id,
            _lease: None,
            migration_guard,
            owned: Some(ConnectionState {
                authority,
                control,
                validity: receiver,
                renewal: Some(renewal),
            }),
        })
    }
}

struct RenewalCaller {
    caller: Caller,
    id: AgentId,
    authorization: Arc<dyn DesktopAuthorization>,
}

async fn renew(
    service: DesktopService,
    context: RenewalCaller,
    request: DesktopSessionRequest,
    control: Arc<dyn DesktopOwnershipControl>,
    authority: Arc<Mutex<DesktopOwnership>>,
    validity: watch::Sender<Option<Instant>>,
) {
    loop {
        // Open already revalidated after provider I/O. Avoid an immediate
        // duplicate read/renew transaction while the first frames are arriving.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let current = *validity.borrow();
        let Some(current) = current else { return };
        let timing = timing::Renewal::start();
        let result = renew_before_expiry(current, async {
            context.authorization.current(&context.caller).await?;
            let (sandbox_id, runtime) = service.authorize(&context.caller, &context.id).await?;
            if sandbox_id != request.sandbox_id || runtime != request.runtime {
                return Err(DesktopError::Unavailable);
            }
            let started = Instant::now();
            let challenge = control
                .ownership_challenge()
                .await
                .map_err(|_| DesktopError::RuntimeUnavailable)?;
            let deadline = deadline(started, &challenge)?;
            let store = service
                .reads
                .session_store()
                .map_err(|_| DesktopError::Unavailable)?;
            let previous = authority.lock().await.clone();
            let next = store
                .renew_exclusive_sandbox_session(&request, &previous)
                .await
                .map_err(|_| DesktopError::Unavailable)?;
            *authority.lock().await = next.clone();
            tokio::time::timeout_at(deadline, control.renew_ownership(&next, &challenge))
                .await
                .map_err(|_| DesktopError::Unavailable)?
                .map_err(|_| DesktopError::RuntimeUnavailable)?;
            Ok::<_, DesktopError>(deadline)
        })
        .await;
        match result {
            Ok(deadline) if current > Instant::now() && deadline > Instant::now() => {
                if validity.send(Some(deadline)).is_err() {
                    return;
                }
                timing.finish(timing::RenewalOutcome::Renewed);
            }
            _ => {
                tracing::warn!(
                    "desktop authority lost; closing stream without early lease release"
                );
                validity.send_replace(None);
                timing.finish(timing::RenewalOutcome::Refused);
                return;
            }
        }
    }
}

async fn renew_before_expiry<T>(
    current: Instant,
    operation: impl std::future::Future<Output = Result<T, DesktopError>>,
) -> Result<T, DesktopError> {
    tokio::select! {
        biased;
        // A ready late ACK must not restore local authority after expiry.
        // timeout_at polls the operation first, including after a task pause.
        _ = tokio::time::sleep_until(current) => Err(DesktopError::Unavailable),
        result = operation => {
            if current <= Instant::now() {
                Err(DesktopError::Unavailable)
            } else {
                result
            }
        }
    }
}

fn deadline(
    started: Instant,
    challenge: &DesktopOwnershipChallenge,
) -> Result<Instant, DesktopError> {
    challenge
        .validate()
        .map_err(|_| DesktopError::RuntimeUnavailable)?;
    let deadline = started + Duration::from_millis(challenge.valid_for_ms);
    if deadline <= Instant::now() {
        return Err(DesktopError::Unavailable);
    }
    Ok(deadline)
}

async fn release(store: &Arc<dyn SandboxSessionStore>, authority: &DesktopOwnership) {
    if !matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            store.release_exclusive_sandbox_session(authority)
        )
        .await,
        Ok(Ok(()))
    ) {
        tracing::warn!("desktop claim release failed; expiry retains recovery");
    }
}

#[cfg(test)]
mod tests;
