use super::*;
use crate::{HomeBackingIdentity, HomeBackingObserver, HomeDeleteReceipt, HomeFinalityReceipt};

impl PersistentHomeVolumes {
    pub(super) async fn delete_home_acknowledged(
        &self,
        original: &HomeBackingIdentity,
        operation_id: &str,
        observer: &dyn HomeBackingObserver,
    ) -> Result<HomeDeleteReceipt> {
        original.validate()?;
        if original.reference.provider_scope != self.scope
            || !crate::provider_finality::identifier(operation_id)
            || self
                .capture_original_backing(&original.reference.home, observer)
                .await?
                != *original
        {
            return Err(invalid());
        }
        let home = &original.reference.home;
        let claim = self.read(&home.request).await?.ok_or_else(invalid)?;
        if self.receipt(&home.request, &claim)? != home.receipt
            || claim.metadata.deletion_timestamp.is_some()
            || claim
                .spec
                .as_ref()
                .and_then(|spec| spec.volume_name.as_deref())
                != Some(original.reference.pv_name.as_str())
        {
            return Err(invalid());
        }
        let version = claim
            .metadata
            .resource_version
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid)?;
        let params = DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(home.receipt.storage_generation.clone()),
                resource_version: Some(version.clone()),
            }),
            propagation_policy: Some(kube::api::PropagationPolicy::Foreground),
            ..DeleteParams::default()
        };
        let response = self
            .claims
            .delete(&home.receipt.storage_reference, &params)
            .await
            .map_err(|error| failure("delete exact original home claim", error))?;
        let response_sha256 = response.either(
            |row| crate::provider_request_digest(&row),
            |row| crate::provider_request_digest(&row),
        )?;
        let receipt = HomeDeleteReceipt {
            original: original.clone(),
            operation_id: operation_id.to_owned(),
            resource_version: version,
            response_sha256,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub(super) async fn observe_home_backing_absence(
        &self,
        delete: &HomeDeleteReceipt,
        observer: &dyn HomeBackingObserver,
    ) -> Result<HomeFinalityReceipt> {
        delete.validate()?;
        if delete.original.reference.provider_scope != self.scope {
            return Err(invalid());
        }
        let home = &delete.original.reference.home;
        // A name rebound to another claim remains conservative; a PVC 404 only
        // permits the separate exact backing-disk observation below.
        if self.read(&home.request).await?.is_some() {
            return Err(AdapterError::Unavailable(
                "original home claim deletion is pending".into(),
            ));
        }
        let backing_absence = observer.observe_original_absence(&delete.original).await?;
        let receipt = HomeFinalityReceipt {
            original: delete.original.clone(),
            claim_delete_operation_id: delete.operation_id.clone(),
            claim_delete_resource_version: delete.resource_version.clone(),
            claim_delete_response_sha256: delete.response_sha256.clone(),
            backing_absence,
        };
        receipt.validate()?;
        Ok(receipt)
    }
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("original home deletion identity is unavailable or changed".into())
}
