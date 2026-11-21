use super::*;
use crate::HomeProvisionReceipt;

const DISPATCH_LABEL: &str = "cma/provider-home-dispatch";

impl PersistentHomeVolumes {
    pub(super) async fn prepare_original(
        &self,
        request: &HomeVolumeRequest,
        operation_id: &str,
    ) -> Result<HomeProvisionReceipt> {
        self.validate_request_scope(request)?;
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
            provider_scope: self.scope.clone(),
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
}
fn invalid() -> AdapterError {
    AdapterError::Rejected("original home creation response is invalid".into())
}
#[cfg(test)]
#[path = "home_original_claim_tests.rs"]
mod tests;
