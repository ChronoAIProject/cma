use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::SandboxStoreError;
use layer2_core::SandboxId;
use layer3_adapter::HomeVolumeRequest;

impl MongoSandboxStore {
    pub(in crate::store) async fn native_prepared_home_reservation(
        &self,
        id: &SandboxId,
    ) -> Result<Option<layer2_core::sandbox::SandboxHome>, SandboxStoreError> {
        let Some(record) = self.native_prepared_record(id).await? else {
            return Ok(None);
        };
        Ok(record
            .payload
            .state
            .owns_runtime()
            .then_some(record.payload.home)
            .flatten()
            .map(|home| *home))
    }
    pub(in crate::store) async fn native_prepared_home_authority(
        &self,
        request: &HomeVolumeRequest,
    ) -> Result<bool, SandboxStoreError> {
        let Some(record) = self
            .native_prepared_record(&SandboxId(request.sandbox_id.clone()))
            .await?
        else {
            return Ok(false);
        };
        Ok(record.payload.state.owns_runtime()
            && record
                .payload
                .home
                .as_ref()
                .is_some_and(|home| !home.released && home.request == *request))
    }
    pub(in crate::store) async fn native_prepared_runtime_authority(
        &self,
        id: &SandboxId,
        generation: u64,
        runtime_id: &str,
    ) -> Result<bool, SandboxStoreError> {
        let Some(record) = self.native_prepared_record(id).await? else {
            return Ok(false);
        };
        Ok(record.payload.state.owns_runtime()
            && generation == 1
            && record.payload.home.as_ref().is_some_and(|home| {
                home.attachment().is_some()
                    && home
                        .native_pool
                        .as_ref()
                        .is_some_and(|native| !native.cleaned && native.claim.is_some())
            })
            && record
                .payload
                .runtime
                .as_ref()
                .is_none_or(|runtime| runtime.runtime_id == runtime_id))
    }
}
