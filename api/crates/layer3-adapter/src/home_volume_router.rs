//! Exact storage authority follows the persisted request, including before its
//! first PVC response. Provider defaults are consulted only during reservation.
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{
    AdapterError, HomeDeleteReceipt, HomeFinalityReceipt, HomeProvisionReceipt,
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope,
    HomeVolumeStore, Result,
};

struct Route {
    store: Arc<dyn HomeVolumeStore>,
    policy: HomeVolumePolicy,
    scope: HomeVolumeScope,
}

/// Local historical homes and remote native homes coexist. A missing route is
/// unavailable; the router never probes a different cluster after any failure.
pub struct RoutedHomeVolumes {
    local: Option<Route>,
    native: Option<Route>,
}

type ConfiguredStore = (Arc<dyn HomeVolumeStore>, HomeVolumePolicy);

impl RoutedHomeVolumes {
    pub fn new(local: Option<ConfiguredStore>, native: Option<ConfiguredStore>) -> Result<Self> {
        let route = |value: ConfiguredStore| -> Result<Route> {
            let (store, policy) = value;
            policy.validate()?;
            let scope = store
                .allocation_scope()
                .ok_or_else(|| rejected("configured home store has no allocation authority"))?;
            scope.validate()?;
            Ok(Route {
                store,
                policy,
                scope,
            })
        };
        let value = Self {
            local: local.map(route).transpose()?,
            native: native.map(route).transpose()?,
        };
        if value.local.is_none() && value.native.is_none() {
            return Err(rejected("no home storage authority is configured"));
        }
        if value
            .local
            .as_ref()
            .zip(value.native.as_ref())
            .is_some_and(|(local, native)| local.scope == native.scope)
        {
            return Err(rejected("home storage authorities are ambiguous"));
        }
        Ok(value)
    }

    /// Pure planning only. The caller must persist this result before any
    /// allocation, native claim, or runtime dispatch. An existing scoped request
    /// is never repinned to a new policy or another cluster.
    pub fn reserve_for(
        &self,
        native: bool,
        request: &HomeVolumeRequest,
    ) -> Result<HomeVolumeRequest> {
        request.validate()?;
        let route = self.selected(native)?;
        if request.scope.is_some() {
            if self.is_native(request)? != native {
                return Err(rejected(
                    "home allocation conflicts with its recorded runtime provider",
                ));
            }
            return Ok(request.clone());
        }
        let mut reserved = request.clone();
        reserved.policy = route.policy.clone();
        reserved.scope = Some(route.scope.clone());
        reserved.validate()?;
        Ok(reserved)
    }

    /// An unscoped historical native home is valid only when both routes
    /// explicitly recognize the same original API/namespace authority.
    pub(crate) fn shares_legacy_authority(&self) -> bool {
        self.local
            .as_ref()
            .zip(self.native.as_ref())
            .is_some_and(|(local, native)| native.store.accepts_scope(&local.scope))
    }

    pub fn is_native(&self, request: &HomeVolumeRequest) -> Result<bool> {
        request.validate()?;
        match &request.scope {
            None => {
                self.selected(false)?;
                Ok(false)
            }
            Some(scope)
                if self
                    .local
                    .as_ref()
                    .is_some_and(|route| route.store.accepts_scope(scope)) =>
            {
                Ok(false)
            }
            Some(scope)
                if self
                    .native
                    .as_ref()
                    .is_some_and(|route| route.store.accepts_scope(scope)) =>
            {
                Ok(true)
            }
            Some(_) => Err(rejected(
                "recorded home storage authority is not configured",
            )),
        }
    }

    fn selected(&self, native: bool) -> Result<&Route> {
        (if native { &self.native } else { &self.local })
            .as_ref()
            .ok_or_else(|| {
                AdapterError::Unavailable("recorded home storage provider is unavailable".into())
            })
    }

    fn route(&self, request: &HomeVolumeRequest) -> Result<&Route> {
        self.selected(self.is_native(request)?)
    }

    fn original_route(&self, original: &HomeProvisionReceipt) -> Result<&Route> {
        original.validate()?;
        let route = self.route(&original.home().request)?;
        if !route.store.accepts_scope(original.provider_scope()) {
            return Err(rejected(
                "original home receipt conflicts with its allocation authority",
            ));
        }
        Ok(route)
    }
}

#[async_trait::async_trait]
impl HomeVolumeStore for RoutedHomeVolumes {
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>> {
        let mut all = Vec::new();
        let mut volumes = BTreeSet::new();
        for (native, route) in [(false, &self.local), (true, &self.native)] {
            let Some(route) = route else { continue };
            for attachment in route.store.inventory().await? {
                attachment.validate_for(&attachment.request.sandbox_id)?;
                if self.is_native(&attachment.request)? != native {
                    // A migration store may retain the same legacy API scope as
                    // the local route. Only an identical earlier observation can
                    // be shared; a different disk or lineage remains a conflict.
                    if all.iter().any(|prior| prior == &attachment) {
                        continue;
                    }
                    return Err(rejected("home inventory is outside its recorded cluster"));
                }
                if !volumes.insert(attachment.request.volume_id.clone()) {
                    return Err(rejected(
                        "home inventory repeats an allocation across providers",
                    ));
                }
                all.push(attachment);
            }
        }
        Ok(all)
    }

    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt> {
        let receipt = self.route(request)?.store.prepare(request).await?;
        receipt.validate_for(request)?;
        Ok(receipt)
    }

    async fn verify(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        attachment.validate_for(&attachment.request.sandbox_id)?;
        self.route(&attachment.request)?
            .store
            .verify(attachment)
            .await
    }

    async fn remove_captures(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        attachment.validate_for(&attachment.request.sandbox_id)?;
        self.route(&attachment.request)?
            .store
            .remove_captures(attachment)
            .await
    }

    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<()> {
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
        }
        self.route(request)?.store.remove(request, receipt).await
    }

    async fn prepare_receipted(
        &self,
        request: &HomeVolumeRequest,
        operation_id: &str,
    ) -> Result<HomeProvisionReceipt> {
        let receipt = self
            .route(request)?
            .store
            .prepare_receipted(request, operation_id)
            .await?;
        self.original_route(&receipt)?;
        if receipt.home().request != *request || receipt.operation_id() != operation_id {
            return Err(rejected(
                "home creation acknowledgement belongs to another request",
            ));
        }
        Ok(receipt)
    }

    async fn delete_original_home(
        &self,
        original: &HomeProvisionReceipt,
        operation_id: &str,
    ) -> Result<HomeDeleteReceipt> {
        let receipt = self
            .original_route(original)?
            .store
            .delete_original_home(original, operation_id)
            .await?;
        receipt.validate()?;
        if receipt.original() != original || receipt.operation_id() != operation_id {
            return Err(rejected(
                "home deletion acknowledgement belongs to another allocation",
            ));
        }
        Ok(receipt)
    }

    async fn observe_home_finality(
        &self,
        delete: &HomeDeleteReceipt,
    ) -> Result<HomeFinalityReceipt> {
        delete.validate()?;
        let receipt = self
            .original_route(delete.original())?
            .store
            .observe_home_finality(delete)
            .await?;
        receipt.validate()?;
        if &receipt.delete_receipt() != delete {
            return Err(rejected(
                "home absence acknowledgement belongs to another allocation",
            ));
        }
        Ok(receipt)
    }
}

fn rejected(message: &str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

#[cfg(test)]
#[path = "home_volume_router_tests.rs"]
mod tests;
