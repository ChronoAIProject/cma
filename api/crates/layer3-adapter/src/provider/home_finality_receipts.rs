use super::*;
use crate::{HomeCleanupScope, HomeDeleteReceipt, HomeFinalityReceipt, HomeProvisionReceipt};

impl PersistentHomeVolumes {
    pub(super) async fn delete_home_acknowledged(
        &self,
        original: &HomeProvisionReceipt,
        operation_id: &str,
    ) -> Result<HomeDeleteReceipt> {
        original.validate()?;
        if original.provider_scope() != &self.scope
            || !crate::provider_finality::identifier(operation_id)
        {
            return Err(invalid());
        }
        let home = original.home();
        let claim = self.read(&home.request).await?.ok_or_else(invalid)?;
        if self.receipt(&home.request, &claim)? != home.receipt
            || claim.metadata.deletion_timestamp.is_some()
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

    pub(super) async fn observe_home_claim_absence(
        &self,
        delete: &HomeDeleteReceipt,
    ) -> Result<HomeFinalityReceipt> {
        delete.validate()?;
        if delete.original.provider_scope() != &self.scope {
            return Err(invalid());
        }
        let home = delete.original.home();
        // Existing or rebound names remain held. An authorized namespaced GET
        // must observe absence after the persisted original delete acknowledgement.
        if self.read(&home.request).await?.is_some() {
            return Err(AdapterError::Unavailable(
                "original home claim deletion is pending".into(),
            ));
        }
        let receipt = HomeFinalityReceipt {
            delete: delete.clone(),
            claim_absence_observation_sha256: HomeFinalityReceipt::absence_digest(delete)?,
            cleanup_scope: HomeCleanupScope::NamespacedClaim,
        };
        receipt.validate()?;
        Ok(receipt)
    }
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("original home deletion identity is unavailable or changed".into())
}
