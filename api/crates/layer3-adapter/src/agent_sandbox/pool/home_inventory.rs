//! Resumable discovery preserves the existing Claim → Sandbox → PVC checks.
use super::{
    api_error, native_home_recovery::recovery_origin, NativeHomeVolumes, NativePoolClaim,
    NativePoolHomeOrigin, Result,
};
use crate::{
    inventory_cursor::{request, Child},
    HomeVolumeAttachment, HomeVolumeStore, InventoryCursor,
};
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::api::DynamicObject;
use std::collections::VecDeque;

pub(super) struct Cursor {
    store: NativeHomeVolumes,
    legacy: Option<Box<dyn InventoryCursor<HomeVolumeAttachment>>>,
    claims: Option<Box<dyn InventoryCursor<NativePoolClaim>>>,
    pending: VecDeque<NativePoolClaim>,
    origin: Option<NativePoolHomeOrigin>,
    member: Option<Option<DynamicObject>>,
    volume: Option<Option<PersistentVolumeClaim>>,
    items: Vec<HomeVolumeAttachment>,
}

impl Cursor {
    pub(super) fn new(store: NativeHomeVolumes) -> Self {
        Self {
            legacy: store.legacy.inventory_cursor(),
            claims: Some(store.pools.claim_inventory_cursor()),
            store,
            pending: VecDeque::new(),
            origin: None,
            member: None,
            volume: None,
            items: vec![],
        }
    }

    fn finish_item(&mut self) {
        self.pending.pop_front();
        self.origin = None;
        self.member = None;
        self.volume = None;
    }
}

#[async_trait::async_trait]
impl InventoryCursor<HomeVolumeAttachment> for Cursor {
    async fn advance(&mut self) -> Result<Option<Vec<HomeVolumeAttachment>>> {
        if let Some(legacy) = &mut self.legacy {
            if let Some(items) = legacy.advance().await? {
                self.items = items;
                self.legacy = None;
            }
            return Ok(None);
        }
        if let Some(claims) = &mut self.claims {
            if let Some(items) = claims.advance().await? {
                self.pending = items.into();
                self.claims = None;
            }
            return Ok(None);
        }
        let Some(original) = self.pending.front() else {
            return Ok(Some(std::mem::take(&mut self.items)));
        };
        if !original.request.home.policy.is_native_vct() {
            self.finish_item();
            return Ok(None);
        }
        self.store.native(&original.request.home)?;
        let pools = &self.store.pools;
        if self.origin.is_none() {
            let desired = pools.native_recovery_claim(original)?;
            let claim = request(Child::ClaimRead, async {
                pools
                    .claims
                    .get_opt(&original.name)
                    .await
                    .map_err(api_error)
            })
            .await?;
            self.origin = recovery_origin(original, &desired, claim)?;
            if self.origin.is_none() {
                self.finish_item();
            }
            return Ok(None);
        }
        let origin = self
            .origin
            .as_ref()
            .expect("claim discovery retained its origin");
        let name = origin
            .claim
            .sandbox_name
            .as_deref()
            .ok_or_else(|| super::rejected("native Sandbox missing"))?;
        let storage = origin.storage_reference()?;
        // Preserve the existing parallel attachment reads. Commit each result
        // independently, so cancellation retries only the unfinished child.
        let (member, volume) = (&mut self.member, &mut self.volume);
        let (member_result, volume_result) = tokio::join!(
            async {
                if member.is_none() {
                    *member = Some(
                        request(Child::AttachmentRead, async {
                            pools.members.get_opt(name).await.map_err(api_error)
                        })
                        .await?,
                    );
                }
                Ok::<_, crate::AdapterError>(())
            },
            async {
                if volume.is_none() {
                    *volume = Some(
                        request(Child::AttachmentRead, async {
                            pools.volumes.get_opt(&storage).await.map_err(api_error)
                        })
                        .await?,
                    );
                }
                Ok::<_, crate::AdapterError>(())
            }
        );
        member_result?;
        let attachment = pools.recovered_native_attachment(
            origin,
            Ok(self.member.as_ref().unwrap().clone()),
            volume_result.map(|()| self.volume.as_ref().unwrap().clone()),
        );
        match attachment {
            Ok(Some(home)) => self.items.push(home),
            Ok(None) => {}
            Err(error) => {
                // A rejected identity is not a validated checkpoint. Start this
                // item again from its claim while retaining completed items.
                if matches!(error, crate::AdapterError::Rejected(_)) {
                    self.origin = None;
                    self.member = None;
                    self.volume = None;
                }
                return Err(error);
            }
        }
        self.finish_item();
        Ok(None)
    }
}
