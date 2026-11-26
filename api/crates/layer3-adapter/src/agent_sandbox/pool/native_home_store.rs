//! Native homes are created/deleted by CRDs; legacy receipts retain their original store.
use super::{
    api_error, rejected, unavailable, AgentSandboxConfig, AgentSandboxPools, NativePoolCleanup,
    NativePoolHomeOrigin, Result,
};
use crate::{
    provider::PersistentHomeVolumes, AdapterError, HomeVolumeAttachment, HomeVolumePolicy,
    HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope, HomeVolumeStore,
};
use kube::Client;

#[derive(Clone)]
pub struct NativeHomeVolumes {
    pub(super) pools: AgentSandboxPools,
    pub(super) legacy: PersistentHomeVolumes,
    scope: HomeVolumeScope,
}
impl NativeHomeVolumes {
    pub fn new(client: Client, config: AgentSandboxConfig, api_authority: &str) -> Result<Self> {
        config.validate()?;
        let scope = HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &[api_authority, &config.namespace, &config.deployment],
        )?;
        Ok(Self {
            legacy: PersistentHomeVolumes::namespaced(
                client.clone(),
                &config.namespace,
                &config.deployment,
                api_authority,
            )?,
            pools: AgentSandboxPools::new(client, config),
            scope,
        })
    }
    pub async fn preflight(&self, policy: &HomeVolumePolicy) -> Result<()> {
        policy.validate()?;
        if !policy.is_native_vct() {
            return self.legacy.preflight(policy).await;
        }
        self.pools
            .volumes
            .list(&kube::api::ListParams::default().limit(1))
            .await
            .map_err(api_error)?;
        Ok(())
    }
    pub(super) fn native(&self, request: &HomeVolumeRequest) -> Result<bool> {
        request.validate()?;
        if request.policy.is_native_vct() {
            if request.scope.as_ref() != Some(&self.scope) {
                return Err(rejected("native VCT belongs to another authority"));
            }
            Ok(true)
        } else {
            if request
                .scope
                .as_ref()
                .is_some_and(|scope| !self.legacy.accepts_scope(scope))
            {
                return Err(rejected("legacy native home belongs to another authority"));
            }
            Ok(false)
        }
    }
    async fn absent(&self, origin: &NativePoolHomeOrigin) -> Result<()> {
        origin.validate_for(
            &self.pools.config.namespace,
            &self.pools.config.deployment,
            &origin.claim.request.home,
        )?;
        if self
            .pools
            .claims
            .get_opt(&origin.claim.name)
            .await
            .map_err(api_error)?
            .is_some()
            || self
                .pools
                .members
                .get_opt(
                    origin
                        .claim
                        .sandbox_name
                        .as_deref()
                        .ok_or_else(|| rejected("native Sandbox identity missing"))?,
                )
                .await
                .map_err(api_error)?
                .is_some()
            || self
                .pools
                .volumes
                .get_opt(&origin.storage_reference()?)
                .await
                .map_err(api_error)?
                .is_some()
        {
            return Err(unavailable(
                "native Claim, Sandbox and PVC absence is not established",
            ));
        }
        self.pools
            .cleanup_pods_absent(&NativePoolCleanup {
                request: origin.claim.request.clone(),
                claim: Some(origin.claim.clone()),
                members: vec![super::NativePoolMember {
                    name: origin.claim.sandbox_name.clone().unwrap(),
                    uid: origin.sandbox_uid.clone(),
                    pvc_uid: Some(origin.pvc_uid.clone()),
                }],
            })
            .await
    }
}
#[async_trait::async_trait]
impl HomeVolumeStore for NativeHomeVolumes {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(self.scope.clone())
    }
    fn accepts_scope(&self, scope: &HomeVolumeScope) -> bool {
        scope == &self.scope || self.legacy.accepts_scope(scope)
    }
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>> {
        let mut cursor = super::home_inventory::Cursor::new(self.clone());
        loop {
            if let Some(items) = crate::InventoryCursor::advance(&mut cursor).await? {
                return Ok(items);
            }
        }
    }
    fn inventory_cursor(&self) -> Option<Box<dyn crate::InventoryCursor<HomeVolumeAttachment>>> {
        Some(Box::new(super::home_inventory::Cursor::new(self.clone())))
    }
    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt> {
        if !self.native(request)? {
            return self.legacy.prepare(request).await;
        }
        Err(rejected(
            "native VCT must be allocated through its persisted SandboxClaim",
        ))
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> Result<()> {
        home.validate_for(&home.request.sandbox_id)?;
        if !self.native(&home.request)? {
            return self.legacy.verify(home).await;
        }
        let origin = home
            .receipt
            .native_authority
            .as_deref()
            .ok_or_else(|| rejected("native home authority missing"))?;
        if self.pools.recover_known_native_vct(origin).await?.as_ref() != Some(home) {
            return Err(rejected(AdapterError::NATIVE_HOME_IDENTITY_REFUSAL));
        }
        Ok(())
    }
    async fn remove_captures(&self, home: &HomeVolumeAttachment) -> Result<()> {
        home.validate_for(&home.request.sandbox_id)?;
        if self.native(&home.request)? {
            Ok(())
        } else {
            self.legacy.remove_captures(home).await
        }
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<()> {
        if !self.native(request)? {
            return self.legacy.remove(request, receipt).await;
        }
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
            self.absent(
                receipt
                    .native_authority
                    .as_deref()
                    .ok_or_else(|| rejected("native home authority missing"))?,
            )
            .await
        } else {
            // Unbound allocation cleanup is carried by a durable NativePoolCleanup
            // plan. Refuse success if any matching admission remains observable.
            if self
                .pools
                .inventory_claims()
                .await?
                .iter()
                .any(|c| c.request.home == *request)
            {
                Err(unavailable("native home claim cleanup remains pending"))
            } else {
                Ok(())
            }
        }
    }
    async fn prepare_receipted(
        &self,
        request: &HomeVolumeRequest,
        operation: &str,
    ) -> Result<crate::HomeProvisionReceipt> {
        if self.native(request)? {
            return Err(rejected(
                "native controller creation is not a direct PVC POST receipt",
            ));
        }
        self.legacy.prepare_receipted(request, operation).await
    }
    async fn delete_original_home(
        &self,
        original: &crate::HomeProvisionReceipt,
        operation: &str,
    ) -> Result<crate::HomeDeleteReceipt> {
        if self.native(&original.home().request)? {
            return Err(rejected(
                "native controller cleanup cannot use legacy PVC DELETE provenance",
            ));
        }
        self.legacy.delete_original_home(original, operation).await
    }
    async fn observe_home_finality(
        &self,
        delete: &crate::HomeDeleteReceipt,
    ) -> Result<crate::HomeFinalityReceipt> {
        if self.native(&delete.original().home().request)? {
            return Err(rejected(
                "native controller finality requires its UID chain",
            ));
        }
        self.legacy.observe_home_finality(delete).await
    }
}
