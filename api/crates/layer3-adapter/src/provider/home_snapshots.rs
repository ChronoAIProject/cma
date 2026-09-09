use kube::api::{DeleteParams, DynamicObject, ListParams, PostParams, Preconditions};

use crate::{
    AdapterError, HomeSnapshotInventoryEntry, HomeSnapshotObservation, HomeSnapshotReceipt,
    HomeSnapshotRequest, HomeSnapshotStore, HomeVolumeStore, Result,
};

use super::{failure, PersistentHomeVolumes};

#[path = "home_snapshot_identity.rs"]
mod identity;

impl PersistentHomeVolumes {
    async fn read_snapshot(&self, request: &HomeSnapshotRequest) -> Result<Option<DynamicObject>> {
        self.templates
            .get_opt(&identity::name(request))
            .await
            .map_err(|error| failure("read home snapshot", error))
    }

    fn snapshot_receipt(
        &self,
        request: &HomeSnapshotRequest,
        snapshot: &DynamicObject,
    ) -> Result<HomeSnapshotReceipt> {
        identity::receipt(&self.namespace, &self.controller, request, snapshot)
    }
}

#[async_trait::async_trait]
impl HomeSnapshotStore for PersistentHomeVolumes {
    async fn snapshot_inventory(&self) -> Result<Vec<HomeSnapshotInventoryEntry>> {
        let mut params = ListParams::default()
            .labels(&format!("{}={}", identity::LABEL, self.controller))
            .limit(200);
        let mut cursors = std::collections::BTreeSet::new();
        let mut captures = std::collections::BTreeSet::new();
        let mut entries = Vec::new();
        loop {
            let page = self
                .templates
                .list(&params)
                .await
                .map_err(|error| failure("list home snapshots", error))?;
            for snapshot in page.items {
                let request = identity::recorded_request(&snapshot)?;
                let receipt = self.snapshot_receipt(&request, &snapshot)?;
                if !captures.insert(receipt.snapshot_reference.clone()) {
                    return Err(AdapterError::Rejected(
                        "home snapshot inventory repeats an identity".into(),
                    ));
                }
                entries.push(HomeSnapshotInventoryEntry { request, receipt });
            }
            let Some(cursor) = page.metadata.continue_.filter(|value| !value.is_empty()) else {
                return Ok(entries);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(AdapterError::Rejected(
                    "home snapshot inventory did not advance".into(),
                ));
            }
            params = params.continue_token(&cursor);
        }
    }

    async fn prepare_snapshot(
        &self,
        request: &HomeSnapshotRequest,
    ) -> Result<HomeSnapshotObservation> {
        let desired = identity::desired(&self.namespace, &self.controller, request)?;
        self.verify(&request.home).await?;
        let snapshot = match self.read_snapshot(request).await? {
            Some(snapshot) => snapshot,
            None => match self
                .templates
                .create(&PostParams::default(), &desired)
                .await
            {
                Ok(snapshot) => snapshot,
                Err(error) => match self.read_snapshot(request).await? {
                    Some(snapshot) => snapshot,
                    None => return Err(failure("allocate home snapshot", error)),
                },
            },
        };
        let receipt = self.snapshot_receipt(request, &snapshot)?;
        // A deleted/recreated claim must not authorize a snapshot of a different disk.
        self.verify(&request.home).await?;
        identity::observe(receipt, &snapshot)
    }

    async fn observe_snapshot(
        &self,
        request: &HomeSnapshotRequest,
        previous: &HomeSnapshotObservation,
    ) -> Result<HomeSnapshotObservation> {
        let receipt = &previous.receipt;
        receipt.validate_for(request)?;
        let snapshot = self.read_snapshot(request).await?.ok_or_else(|| {
            AdapterError::Rejected("the recorded home snapshot no longer exists".into())
        })?;
        if self.snapshot_receipt(request, &snapshot)? != *receipt {
            return Err(AdapterError::Rejected(
                "the recorded home snapshot changed identity".into(),
            ));
        }
        let observed = identity::observe(receipt.clone(), &snapshot)?;
        if previous.captured_at.is_none() {
            // The controller resolves a PVC name asynchronously. Verify the
            // original UID until the capture boundary is observed, including
            // the first observation that establishes that boundary.
            self.verify(&request.home).await?;
        } else if observed.captured_at != previous.captured_at {
            return Err(AdapterError::Rejected(
                "the recorded home snapshot moved its capture boundary".into(),
            ));
        }
        if previous.ready && (!observed.ready || previous.captured_at.is_none()) {
            return Err(AdapterError::Rejected(
                "the recorded home snapshot lost its ready state".into(),
            ));
        }
        // After a verified capture boundary, observation needs no live source.
        Ok(observed)
    }

    async fn remove_snapshot(
        &self,
        request: &HomeSnapshotRequest,
        receipt: Option<&HomeSnapshotReceipt>,
    ) -> Result<()> {
        request.validate()?;
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
        }
        let Some(snapshot) = self.read_snapshot(request).await? else {
            return Ok(());
        };
        let captured = self.snapshot_receipt(request, &snapshot)?;
        if receipt.is_some_and(|receipt| *receipt != captured) {
            return Err(AdapterError::Rejected(
                "home snapshot cleanup target was replaced".into(),
            ));
        }
        let version = snapshot
            .metadata
            .resource_version
            .filter(|version| !version.is_empty())
            .ok_or_else(|| {
                AdapterError::Rejected("home snapshot has no resource version".into())
            })?;
        if snapshot.metadata.deletion_timestamp.is_none() {
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(captured.snapshot_generation.clone()),
                    resource_version: Some(version),
                }),
                ..DeleteParams::default()
            };
            match self
                .templates
                .delete(&captured.snapshot_reference, &params)
                .await
            {
                Ok(_) => (),
                Err(kube::Error::Api(response)) if response.is_not_found() => return Ok(()),
                Err(error) => return Err(failure("release exact home snapshot", error)),
            }
        }
        match self.read_snapshot(request).await? {
            None => Ok(()),
            Some(current)
                if current
                    .metadata
                    .uid
                    .as_deref()
                    .is_some_and(|uid| !uid.is_empty() && uid != captured.snapshot_generation) =>
            {
                Ok(())
            }
            Some(_) => Err(AdapterError::Unavailable(
                "home snapshot release is still pending".into(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "home_snapshots_tests.rs"]
mod tests;
