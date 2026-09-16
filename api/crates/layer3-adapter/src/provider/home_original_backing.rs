use super::*;
use crate::{HomeBackingIdentity, HomeBackingObserver, HomeBackingReference, HomeProvisionReceipt};

const DISPATCH_LABEL: &str = "cma/provider-home-dispatch";

impl PersistentHomeVolumes {
    pub(super) async fn prepare_original(
        &self,
        request: &HomeVolumeRequest,
        operation_id: &str,
    ) -> Result<HomeProvisionReceipt> {
        request.validate()?;
        let marker = crate::provider::dispatch_marker(operation_id)?;
        self.verify_template(&request.policy).await?;
        if self.read(request).await?.is_some() {
            return Err(AdapterError::Unavailable(
                "existing home cannot acquire original creation acknowledgement".into(),
            ));
        }
        let mut desired = identity::desired_claim(&self.namespace, &self.controller, request)?;
        desired
            .metadata
            .labels
            .get_or_insert_default()
            .insert(DISPATCH_LABEL.into(), marker.clone());
        // A single acknowledged POST pins the original UID. Error, conflict and
        // lost response remain unresolved even if ordinary prepare later recovers.
        let claim = self
            .claims
            .create(&PostParams::default(), &desired)
            .await
            .map_err(|error| failure("create original durable home", error))?;
        if claim.metadata.deletion_timestamp.is_some()
            || claim
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get(DISPATCH_LABEL))
                != Some(&marker)
        {
            return Err(invalid());
        }
        let result = HomeProvisionReceipt {
            home: HomeVolumeAttachment {
                request: request.clone(),
                receipt: self.receipt(request, &claim)?,
            },
            operation_id: operation_id.to_owned(),
            creation_resource_version: claim
                .metadata
                .resource_version
                .clone()
                .ok_or_else(invalid)?,
            creation_response_sha256: crate::provider_request_digest(&claim)?,
        };
        result.validate()?;
        Ok(result)
    }

    pub(super) async fn capture_original_backing(
        &self,
        attachment: &HomeVolumeAttachment,
        observer: &dyn HomeBackingObserver,
    ) -> Result<HomeBackingIdentity> {
        let reference = self.exact_backing_reference(attachment).await?;
        let original = observer.capture_original(&reference).await?;
        original.validate()?;
        if original.reference != reference
            || self.exact_backing_reference(attachment).await? != reference
        {
            return Err(invalid());
        }
        Ok(original)
    }

    async fn exact_backing_reference(
        &self,
        attachment: &HomeVolumeAttachment,
    ) -> Result<HomeBackingReference> {
        attachment.validate_for(&attachment.request.sandbox_id)?;
        let claim = self.read(&attachment.request).await?.ok_or_else(invalid)?;
        if self.receipt(&attachment.request, &claim)? != attachment.receipt
            || claim.metadata.deletion_timestamp.is_some()
            || claim
                .status
                .as_ref()
                .and_then(|status| status.phase.as_deref())
                != Some("Bound")
        {
            return Err(invalid());
        }
        let name = claim
            .spec
            .as_ref()
            .and_then(|spec| spec.volume_name.as_deref())
            .filter(|name| !name.is_empty())
            .ok_or_else(invalid)?;
        let volume = self
            .volumes
            .get(name)
            .await
            .map_err(|error| failure("read exact original home persistent volume", error))?;
        let spec = volume.spec.as_ref().ok_or_else(invalid)?;
        let claim_ref = spec.claim_ref.as_ref().ok_or_else(invalid)?;
        let csi = spec.csi.as_ref().ok_or_else(invalid)?;
        if volume.metadata.name.as_deref() != Some(name)
            || volume.metadata.deletion_timestamp.is_some()
            || volume
                .status
                .as_ref()
                .and_then(|status| status.phase.as_deref())
                != Some("Bound")
            || claim_ref.uid.as_deref() != Some(attachment.receipt.storage_generation.as_str())
            || claim_ref.name.as_deref() != Some(attachment.receipt.storage_reference.as_str())
            || claim_ref.namespace.as_deref() != Some(self.namespace.as_str())
            || claim_ref
                .kind
                .as_deref()
                .is_some_and(|kind| kind != "PersistentVolumeClaim")
            || claim_ref
                .api_version
                .as_deref()
                .is_some_and(|version| version != "v1")
            || spec.storage_class_name.as_deref()
                != Some(attachment.request.policy.storage_class.as_str())
        {
            return Err(invalid());
        }
        let reference = HomeBackingReference {
            provider_scope: self.scope.clone(),
            home: attachment.clone(),
            pv_name: name.to_owned(),
            pv_uid: volume.metadata.uid.ok_or_else(invalid)?,
            csi_driver: csi.driver.clone(),
            volume_handle: csi.volume_handle.clone(),
        };
        reference.validate()?;
        Ok(reference)
    }
}

fn invalid() -> AdapterError {
    AdapterError::Rejected(
        "original home claim, persistent volume or backing identity changed".into(),
    )
}

#[cfg(test)]
#[path = "home_original_backing_tests.rs"]
mod tests;
