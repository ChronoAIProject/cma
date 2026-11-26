//! Runtime ownership is independent of the retired CMA archive capability.
use super::*;

impl ProviderHostedControlTransport {
    pub fn with_open_reply_loss(
        mut self,
        sandbox_id: String,
        claim: Arc<dyn crate::hosted::HostedOpenReplyLoss>,
    ) -> Self {
        self.open_reply_loss = Some((sandbox_id, claim));
        self
    }

    pub(super) async fn open_reply_loss_for(
        &self,
        handle: &RuntimeHandle,
    ) -> Option<&dyn crate::hosted::HostedOpenReplyLoss> {
        let (sandbox_id, claim) = self.open_reply_loss.as_ref()?;
        let bound = self.binding(handle).await.ok()?;
        (bound.sandbox_id == *sandbox_id).then_some(claim.as_ref())
    }

    pub(super) async fn binding(&self, handle: &RuntimeHandle) -> crate::Result<BoundRuntime> {
        self.state
            .lock()
            .await
            .bindings
            .get(handle)
            .cloned()
            .ok_or_else(|| AdapterError::Unavailable("runtime has no sandbox binding".into()))
    }

    pub(super) async fn archive(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<Arc<dyn CheckpointArchiveStore>> {
        if self.archive_factory.is_none() {
            return Err(AdapterError::Unsupported(
                "CMA archives are retired for this runtime transport".into(),
            ));
        }
        self.binding(handle).await?.store.ok_or_else(|| {
            AdapterError::Unavailable("runtime has no sandbox-bound checkpoint store".into())
        })
    }

    pub(super) async fn bind_native_identity(
        &self,
        handle: &RuntimeHandle,
        sandbox_id: &str,
        digest: &str,
    ) -> crate::Result<()> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AdapterError::Rejected(
                "native runtime request digest is invalid".into(),
            ));
        }
        self.bind_identity(handle, sandbox_id, Some(digest)).await
    }

    pub(super) async fn bind_sandbox(
        &self,
        handle: &RuntimeHandle,
        sandbox_id: &str,
    ) -> crate::Result<()> {
        self.bind_identity(handle, sandbox_id, None).await
    }

    async fn bind_identity(
        &self,
        handle: &RuntimeHandle,
        sandbox_id: &str,
        digest: Option<&str>,
    ) -> crate::Result<()> {
        if sandbox_id.is_empty()
            || sandbox_id.len() > 256
            || sandbox_id.trim() != sandbox_id
            || sandbox_id.chars().any(char::is_control)
        {
            return Err(AdapterError::Rejected(
                "runtime sandbox identity is invalid".into(),
            ));
        }
        let mut state = self.state.lock().await;
        if let Some(existing) = state.bindings.get(handle) {
            if existing.sandbox_id != sandbox_id {
                return Err(AdapterError::Rejected(
                    "runtime handle is already bound to another sandbox".into(),
                ));
            }
        } else {
            let store = self
                .archive_factory
                .as_ref()
                .map(|factory| factory.for_sandbox(sandbox_id))
                .transpose()?;
            state.bindings.insert(
                handle.clone(),
                BoundRuntime {
                    sandbox_id: sandbox_id.into(),
                    request_sha256: None,
                    store,
                },
            );
        }
        if let Some(digest) = digest {
            let binding = state
                .bindings
                .get_mut(handle)
                .expect("binding exists under the same lock");
            if binding.request_sha256.as_deref() != Some(digest) {
                binding.request_sha256 = Some(digest.into());
                let epoch = state.runtime_epochs.entry(handle.clone()).or_default();
                *epoch = epoch.wrapping_add(1);
                state.native_http.retain(|(runtime, _), slot| {
                    if runtime == handle {
                        slot.cancel();
                        false
                    } else {
                        true
                    }
                });
            }
        }
        Ok(())
    }
}
