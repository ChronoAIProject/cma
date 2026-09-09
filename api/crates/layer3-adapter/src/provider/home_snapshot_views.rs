use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::api::{DeleteParams, ListParams, PostParams, Preconditions};

use crate::{
    AdapterError, HomeSnapshotExportRequest, HomeSnapshotStore, HomeSnapshotViewInventoryEntry,
    HomeSnapshotViewObservation, HomeSnapshotViewReceipt, HomeSnapshotViewStore, Result,
};

use super::{failure, PersistentHomeVolumes};

#[path = "home_snapshot_view_identity.rs"]
mod identity;

impl PersistentHomeVolumes {
    async fn read_view(
        &self,
        request: &HomeSnapshotExportRequest,
    ) -> Result<Option<PersistentVolumeClaim>> {
        self.claims
            .get_opt(&identity::name(request)?)
            .await
            .map_err(|error| failure("read snapshot export view", error))
    }

    fn view_receipt(
        &self,
        request: &HomeSnapshotExportRequest,
        claim: &PersistentVolumeClaim,
    ) -> Result<HomeSnapshotViewReceipt> {
        identity::receipt(&self.namespace, &self.controller, request, claim)
    }

    async fn verify_view_source(&self, request: &HomeSnapshotExportRequest) -> Result<()> {
        if self
            .observe_snapshot(&request.snapshot, &request.observation)
            .await?
            != request.observation
        {
            return Err(identity::invalid());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl HomeSnapshotViewStore for PersistentHomeVolumes {
    async fn view_inventory(&self) -> Result<Vec<HomeSnapshotViewInventoryEntry>> {
        let mut params = ListParams::default()
            .labels(&format!("{}={}", identity::LABEL, self.controller))
            .limit(200);
        let mut cursors = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        let mut entries = Vec::new();
        loop {
            let page = self
                .claims
                .list(&params)
                .await
                .map_err(|error| failure("list snapshot export views", error))?;
            for claim in page.items {
                let request = identity::recorded_request(&claim)?;
                let receipt = self.view_receipt(&request, &claim)?;
                if !names.insert(receipt.volume_reference.clone()) {
                    return Err(identity::invalid());
                }
                entries.push(HomeSnapshotViewInventoryEntry { request, receipt });
            }
            let Some(cursor) = page.metadata.continue_.filter(|value| !value.is_empty()) else {
                return Ok(entries);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(identity::invalid());
            }
            params = params.continue_token(&cursor);
        }
    }

    async fn prepare_view(
        &self,
        request: &HomeSnapshotExportRequest,
    ) -> Result<HomeSnapshotViewObservation> {
        let desired = identity::desired(&self.namespace, &self.controller, request)?;
        self.verify_view_source(request).await?;
        let claim = match self.read_view(request).await? {
            Some(claim) => claim,
            None => match self.claims.create(&PostParams::default(), &desired).await {
                Ok(claim) => claim,
                Err(error) => match self.read_view(request).await? {
                    Some(claim) => claim,
                    None => return Err(failure("allocate snapshot export view", error)),
                },
            },
        };
        let receipt = self.view_receipt(request, &claim)?;
        // dataSource is resolved asynchronously by name. The captured snapshot's
        // original UID must survive allocation and the first observed binding.
        self.verify_view_source(request).await?;
        identity::observe(receipt, &claim)
    }

    async fn observe_view(
        &self,
        request: &HomeSnapshotExportRequest,
        previous: &HomeSnapshotViewObservation,
    ) -> Result<HomeSnapshotViewObservation> {
        previous.receipt.validate_for(request)?;
        let claim = self
            .read_view(request)
            .await?
            .ok_or_else(identity::invalid)?;
        let receipt = self.view_receipt(request, &claim)?;
        if receipt != previous.receipt {
            return Err(identity::invalid());
        }
        let observed = identity::observe(receipt, &claim)?;
        if previous.bound_volume.is_none() {
            self.verify_view_source(request).await?;
        } else if previous.bound_volume != observed.bound_volume {
            return Err(identity::invalid());
        }
        Ok(observed)
    }

    async fn remove_view(
        &self,
        request: &HomeSnapshotExportRequest,
        receipt: Option<&HomeSnapshotViewReceipt>,
    ) -> Result<()> {
        request.validate()?;
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
        }
        let Some(claim) = self.read_view(request).await? else {
            return Ok(());
        };
        let captured = self.view_receipt(request, &claim)?;
        if receipt.is_some_and(|receipt| *receipt != captured) {
            return Err(identity::invalid());
        }
        let version = claim
            .metadata
            .resource_version
            .filter(|value| !value.is_empty())
            .ok_or_else(identity::invalid)?;
        if claim.metadata.deletion_timestamp.is_none() {
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(captured.volume_generation.clone()),
                    resource_version: Some(version),
                }),
                ..DeleteParams::default()
            };
            match self
                .claims
                .delete(&captured.volume_reference, &params)
                .await
            {
                Ok(_) => (),
                Err(kube::Error::Api(response)) if response.is_not_found() => return Ok(()),
                Err(error) => return Err(failure("release exact snapshot export view", error)),
            }
        }
        match self.read_view(request).await? {
            None => Ok(()),
            Some(current)
                if current
                    .metadata
                    .uid
                    .as_deref()
                    .is_some_and(|uid| !uid.is_empty() && uid != captured.volume_generation) =>
            {
                Ok(())
            }
            Some(_) => Err(AdapterError::Unavailable(
                "snapshot export view release is still pending".into(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "home_snapshot_views_tests.rs"]
pub(super) mod tests;
