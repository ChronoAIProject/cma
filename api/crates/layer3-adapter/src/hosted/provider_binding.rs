//! Runtime ownership is independent of the retired CMA archive capability.
use super::*;

impl ProviderHostedControlTransport {
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

    pub(super) async fn bind_sandbox(
        &self,
        handle: &RuntimeHandle,
        sandbox_id: &str,
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
            return Ok(());
        }
        let store = self
            .archive_factory
            .as_ref()
            .map(|factory| factory.for_sandbox(sandbox_id))
            .transpose()?;
        state.bindings.insert(
            handle.clone(),
            BoundRuntime {
                sandbox_id: sandbox_id.into(),
                store,
            },
        );
        Ok(())
    }
}
