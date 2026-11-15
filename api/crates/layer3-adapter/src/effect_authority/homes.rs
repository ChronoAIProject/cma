//! Every escaped capability retains the same durable effect authority.
use super::{dispatch, EffectAuthority, EffectIntent};
use crate::*;
use std::sync::Arc;
pub struct GuardedHomeVolumes {
    inner: Arc<dyn HomeVolumeStore>,
    authority: Arc<dyn EffectAuthority>,
}
impl GuardedHomeVolumes {
    pub fn new(inner: Arc<dyn HomeVolumeStore>, authority: Arc<dyn EffectAuthority>) -> Self {
        Self { inner, authority }
    }
}
#[async_trait::async_trait]
impl HomeVolumeStore for GuardedHomeVolumes {
    async fn delete_original_home(
        &self,
        _original: &crate::HomeProvisionReceipt,
        _operation_id: &str,
    ) -> Result<crate::HomeDeleteReceipt> {
        let intent = EffectIntent::new(
            "delete_original_home",
            &_original.home().request.volume_id,
            &(&_original, &_operation_id),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.delete_original_home(_original, _operation_id),
        )
        .await
    }
    async fn observe_home_finality(
        &self,
        _delete: &crate::HomeDeleteReceipt,
    ) -> Result<crate::HomeFinalityReceipt> {
        self.inner.observe_home_finality(_delete).await
    }
    async fn prepare_receipted(
        &self,
        _request: &HomeVolumeRequest,
        _operation_id: &str,
    ) -> Result<crate::HomeProvisionReceipt> {
        let intent = EffectIntent::new(
            "prepare_receipted",
            &_request.volume_id,
            &(&_request, &_operation_id),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.prepare_receipted(_request, _operation_id),
        )
        .await
    }
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        self.inner.allocation_scope()
    }
    fn accepts_scope(&self, scope: &HomeVolumeScope) -> bool {
        self.inner.accepts_scope(scope)
    }
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>> {
        self.inner.inventory().await
    }
    fn inventory_cursor(&self) -> Option<Box<dyn crate::InventoryCursor<HomeVolumeAttachment>>> {
        self.inner.inventory_cursor()
    }
    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt> {
        let intent = EffectIntent::new("prepare", &request.volume_id, &(&request,))?;
        dispatch(&self.authority, intent, self.inner.prepare(request)).await
    }
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        self.inner.verify(attachment).await
    }
    async fn remove_captures(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        let intent = EffectIntent::new(
            "remove_captures",
            &attachment.request.volume_id,
            &(&attachment,),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.remove_captures(attachment),
        )
        .await
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<()> {
        let intent = EffectIntent::new("remove", &request.volume_id, &(&request, &receipt))?;
        dispatch(&self.authority, intent, self.inner.remove(request, receipt)).await
    }
}
