//! A challenge pins the original endpoint and assignment through confirmation.
//! No ambiguous desktop mutation is retried or rediscovered on another receiver.
use super::*;
use crate::desktop_ownership::{wire::*, DesktopOwnedRuntime};
use crate::{DesktopOwnership, DesktopOwnershipChallenge, DesktopOwnershipControl};
use tokio::time::Instant;

#[path = "provider_desktop_control.rs"]
mod control;
#[path = "provider_desktop_io.rs"]
mod io;

#[derive(Clone)]
struct Captured {
    handle: RuntimeHandle,
    endpoint: RuntimeEndpoint,
    identity: NativeRuntimeIdentity,
    pod: Option<crate::agent_sandbox::PodBinding>,
}

pub(super) struct Pending {
    captured: Captured,
    challenge: DesktopOwnershipChallenge,
    deadline: Instant,
}

#[async_trait::async_trait]
impl DesktopOwnedRuntime for ProviderHostedControlTransport {
    async fn challenge(&self, handle: &RuntimeHandle) -> crate::Result<DesktopOwnershipChallenge> {
        let started = Instant::now();
        let captured = bounded(
            started + Duration::from_secs(3),
            self.capture_desktop(handle),
        )
        .await?;
        let request = DesktopChallengeRequest {
            sandbox_id: captured.identity.sandbox_id.clone().ok_or_else(refused)?,
            runtime_id: handle.as_str().into(),
            previous: None,
        };
        let challenge: DesktopOwnershipChallenge = bounded(
            started + Duration::from_secs(3),
            captured.exchange(self, "challenge", &request),
        )
        .await?;
        let until = deadline(started, &challenge)?;
        let mut state = self.state.lock().await;
        state
            .desktop_challenges
            .retain(|_, pending| pending.deadline > Instant::now());
        if state.desktop_challenges.len() >= 128
            || state.desktop_challenges.contains_key(&challenge.nonce)
        {
            return Err(refused());
        }
        state.desktop_challenges.insert(
            challenge.nonce.clone(),
            Pending {
                captured,
                challenge: challenge.clone(),
                deadline: until,
            },
        );
        Ok(challenge)
    }

    async fn open(
        &self,
        handle: &RuntimeHandle,
        owner: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> crate::Result<crate::DesktopConnectionHandle> {
        owner.validate()?;
        let pending = self
            .state
            .lock()
            .await
            .desktop_challenges
            .remove(&challenge.nonce)
            .ok_or_else(refused)?;
        if pending.challenge != *challenge
            || pending.captured.handle != *handle
            || owner.runtime_id != handle.as_str()
            || pending.captured.identity.sandbox_id.as_deref() != Some(&owner.sandbox_id)
        {
            return Err(refused());
        }
        bounded(pending.deadline, async {
            let request = DesktopConfirmation {
                ownership: owner.clone(),
                nonce: challenge.nonce.clone(),
            };
            let reply: DesktopAcknowledgement =
                pending.captured.exchange(self, "confirm", &request).await?;
            if reply.ownership != *owner {
                return Err(refused());
            }
            let control = Arc::new(control::Control::new(
                self.clone(),
                pending.captured.clone(),
                owner.clone(),
                pending.deadline,
            ));
            pending.captured.connect(self, owner, control).await
        })
        .await
    }
}

impl ProviderHostedControlTransport {
    async fn capture_desktop(&self, handle: &RuntimeHandle) -> crate::Result<Captured> {
        if matches!(self.client, RuntimeDiscovery::AgentSandbox(_))
            && self.native_transport == NativeRuntimeTransport::Direct
        {
            return Err(AdapterError::Unsupported(
                "owned desktop requires a fenced Kubernetes stream".into(),
            ));
        }
        let (identity, _) = self.native_endpoint_state(handle).await;
        if identity.sandbox_id.is_none() {
            return Err(refused());
        }
        let (endpoint, pod) = match &self.client {
            RuntimeDiscovery::Provider(_) => (self.endpoint(handle).await?, None),
            RuntimeDiscovery::AgentSandbox(client) => {
                if identity.request_sha256.is_none() {
                    return Err(refused());
                }
                let pod = client.pod_binding(handle).await?;
                if pod
                    .runtime_container_id
                    .as_deref()
                    .is_none_or(str::is_empty)
                    || pod.runtime_restart_count.is_none_or(|count| count < 0)
                {
                    return Err(refused());
                }
                let address = std::net::SocketAddr::new(pod.pod_ip, SUPERVISOR_PORT);
                let endpoint =
                    RuntimeEndpoint::parse("http://runtime.invalid", &format!("http://{address}"))?
                        .with_control_token(client.control_token(&pod.sandbox_name))
                        .with_native_binding(handle.clone(), pod.clone())
                        .with_native_identity(identity.clone());
                (endpoint, Some(pod))
            }
        };
        let captured = Captured {
            handle: handle.clone(),
            endpoint,
            identity,
            pod,
        };
        captured.current(self).await?;
        Ok(captured)
    }
}

fn same_session(left: &DesktopOwnership, right: &DesktopOwnership) -> bool {
    left.sandbox_id == right.sandbox_id
        && left.runtime_id == right.runtime_id
        && left.boot_id == right.boot_id
        && left.session_id == right.session_id
        && left.epoch == right.epoch
}

fn deadline(started: Instant, challenge: &DesktopOwnershipChallenge) -> crate::Result<Instant> {
    challenge.validate()?;
    if challenge.valid_for_ms != DESKTOP_RECEIVER_LEASE_MS {
        return Err(refused());
    }
    let until = started + Duration::from_millis(challenge.valid_for_ms);
    if until <= Instant::now() {
        return Err(refused());
    }
    Ok(until)
}

async fn bounded<T>(
    until: Instant,
    operation: impl std::future::Future<Output = crate::Result<T>>,
) -> crate::Result<T> {
    if until <= Instant::now() {
        return Err(refused());
    }
    tokio::select! { biased;
        _ = tokio::time::sleep_until(until) => Err(refused()),
        result = operation => {
            if until <= Instant::now() { return Err(refused()); }
            result
        },
    }
}

fn refused() -> AdapterError {
    AdapterError::Unavailable("desktop receiver authority could not be established".into())
}

#[cfg(test)]
#[path = "provider_desktop_tests.rs"]
pub(super) mod tests;

#[cfg(test)]
#[path = "provider_desktop_legacy_tests.rs"]
mod legacy_tests;
