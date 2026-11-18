use super::*;
impl NativeBackend {
    pub(super) async fn retire_verified_fixture(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        self.calls.push("retire-alternative");
        let homes = self
            .retirement_home
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(absent)?;
        homes.verify(home).await?;
        {
            let rows = self.row.lock().await;
            if let Some(row) = rows.as_ref() {
                if row.observation.handle != *handle || row.request != *request {
                    return Err(layer3_adapter::AdapterError::Rejected(
                        "fixture retirement identity changed".into(),
                    ));
                }
            }
        }
        self.teardown_retained(handle).await
    }
    pub(super) async fn verify_retired_fixture(
        &self,
        _: &RuntimeHandle,
        _: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        self.calls.push("verify-retired");
        if self.hold_finality.load(Ordering::SeqCst) || self.row.lock().await.is_some() {
            return Err(absent());
        }
        Ok(())
    }
}
