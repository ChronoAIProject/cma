//! Bound file reads to a verified native assignment while the Runtime checks its live home.
use super::*;
use crate::file_read_diagnostics::{measure, Stage as ReadStage};
use tokio::time::{Duration, Instant};

const BINDING_VALIDITY: Duration = Duration::from_millis(crate::owner_protocol::RECEIVER_LEASE_MS);
const DISPATCH_MARGIN: Duration = Duration::from_secs(2);

fn dispatch_deadline(expires: Instant, now: Instant) -> Result<Instant> {
    if expires > now + DISPATCH_MARGIN {
        Ok(expires)
    } else {
        Err(AdapterError::Unavailable(
            "file read binding expired".into(),
        ))
    }
}

#[derive(Default)]
pub(super) struct FileReadBinding {
    verified: Option<(String, Instant)>,
    legacy_boot: Option<String>,
}

impl FileReadBinding {
    fn valid_until(&self, boot: &str, now: Instant) -> Option<Instant> {
        self.verified
            .as_ref()
            .filter(|(verified_boot, _)| verified_boot == boot)
            .and_then(|(_, expires)| dispatch_deadline(*expires, now).ok())
    }
}

impl AgentSandboxBackend {
    pub fn with_single_exchange_file_reads(mut self, enabled: bool) -> Self {
        self.single_exchange_file_reads = enabled;
        self
    }

    pub(super) fn assignment_is_current(
        &self,
        handle: &RuntimeHandle,
        assigned: &Arc<binding::Assignment>,
    ) -> bool {
        self.assignments
            .lock()
            .unwrap()
            .get(handle)
            .is_some_and(|current| Arc::ptr_eq(current, assigned))
    }

    pub(super) async fn read_bound_file(
        &self,
        handle: &RuntimeHandle,
        home: &HomeVolumeAttachment,
        request: &AgentFileReadRequest,
    ) -> Result<Option<AgentFileReadReceipt>> {
        if !self.single_exchange_file_reads || !home.request.policy.is_native_vct() {
            return Ok(None);
        }
        let assigned = self.assignments.lock().unwrap().get(handle).cloned();
        let Some(assigned) = assigned else {
            return Ok(None);
        };
        let identity = crate::hosted::provider_request_identity(&assigned.request)?;
        let home_identity = home.identity()?;
        if identity.sandbox_id != request.sandbox_id
            || identity.generation != request.runtime_generation
            || assigned.request.metadata.get("cma/home-identity") != Some(&home_identity)
        {
            return Err(binding_changed());
        }
        let expires = {
            let mut proof = assigned.file_read.lock().await;
            if proof.legacy_boot.as_ref() == Some(&request.boot_id) {
                return Ok(None);
            }
            let started = Instant::now();
            let policy = || {
                measure(
                    ReadStage::ReadPolicy,
                    self.client.verify_native_runtime_policy(handle, home),
                )
            };
            if let Some(expires) = proof.valid_until(&request.boot_id, started) {
                drop(proof);
                // The template can change without changing the Pod, home or boot.
                // An upgraded retained member may use its own live policy proof.
                if let Err(error) = policy().await {
                    let current = self.observe_read_binding(handle, home, &assigned).await?;
                    self.client.complete_native_runtime_policy(
                        home,
                        current.member(),
                        Err(error),
                    )?;
                }
                expires
            } else {
                proof.verified = None;
                let (current, policy) =
                    tokio::join!(self.observe_read_binding(handle, home, &assigned), policy());
                let current = current?;
                self.client
                    .complete_native_runtime_policy(home, current.member(), policy)?;
                // Age includes observation time. A slow authority check cannot
                // extend the lifetime of the resources it read first.
                let expires = dispatch_deadline(started + BINDING_VALIDITY, Instant::now())?;
                proof.verified = Some((request.boot_id.clone(), expires));
                expires
            }
        };
        if !self.assignment_is_current(handle, &assigned) {
            return Err(binding_changed());
        }
        let mut checked = request.clone();
        checked.home_identity = Some(home_identity);
        let reply = measure(ReadStage::RuntimeRead, async {
            let expires = dispatch_deadline(expires, Instant::now())?;
            tokio::time::timeout_at(
                expires,
                self.transport.exchange_bound_file_read(
                    handle,
                    &assigned.pod,
                    &assigned.request_sha256,
                    checked,
                ),
            )
            .await
            .map_err(|_| AdapterError::Unavailable("file read binding expired".into()))?
        })
        .await;
        if !self.assignment_is_current(handle, &assigned) {
            return Err(binding_changed());
        }
        let reply = match reply {
            Ok(Some(reply)) => reply,
            Ok(None) => return Ok(None),
            Err(error) => {
                assigned.file_read.lock().await.verified = None;
                return Err(error);
            }
        };
        match reply {
            ResponseFrame::ReadAgentFile(receipt) => {
                measure(ReadStage::ReceiptValidation, async {
                    if receipt.path != request.path || receipt.size_bytes > request.max_bytes {
                        return Err(AdapterError::unexpected_reply("file read"));
                    }
                    receipt.content()?;
                    Ok(Some(receipt))
                })
                .await
            }
            ResponseFrame::Failure(failure)
                if failure.operation == wire::WireOperation::ReadAgentFile =>
            {
                // A pre-extension Runtime rejects the extra field before dispatch.
                // Never repeat a sent read after a transport or authority failure.
                if !failure.retryable
                    && failure.code == "malformed_frame"
                    && failure.detail == "runtime frame does not match the shared schema"
                {
                    let mut proof = assigned.file_read.lock().await;
                    proof.legacy_boot = Some(request.boot_id.clone());
                    proof.verified = None;
                    return Ok(None);
                }
                assigned.file_read.lock().await.verified = None;
                Err(crate::hosted::reply::failure_error(failure))
            }
            _ => Err(AdapterError::unexpected_reply("file read")),
        }
    }

    async fn observe_read_binding(
        &self,
        handle: &RuntimeHandle,
        home: &HomeVolumeAttachment,
        assigned: &binding::Assignment,
    ) -> Result<crate::agent_sandbox::NativeRuntimeProof> {
        measure(ReadStage::ReadBinding, async {
            let current = self
                .client
                .native_runtime_proof(handle, &assigned.request, home)
                .await?;
            if current.pod() != &assigned.pod || !current.matches(&assigned.request, home)? {
                return Err(binding_changed());
            }
            Ok(current)
        })
        .await
    }
}

fn binding_changed() -> AdapterError {
    AdapterError::Rejected("runtime file read binding changed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_reuse_requires_exact_boot_and_enough_validity_for_dispatch() {
        let now = Instant::now();
        let expires = now + BINDING_VALIDITY;
        let proof = FileReadBinding {
            verified: Some(("boot".into(), expires)),
            legacy_boot: None,
        };
        assert_eq!(proof.valid_until("boot", now), Some(expires));
        assert_eq!(proof.valid_until("replacement", now), None);
        assert_eq!(proof.valid_until("boot", expires - DISPATCH_MARGIN), None);
    }
}
