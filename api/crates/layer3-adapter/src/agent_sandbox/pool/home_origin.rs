use super::{rejected, NativePoolClaim, Result};
use crate::HomeVolumeRequest;
use serde::{Deserialize, Serialize};

/// Physical identities frozen on the Claim and Sandbox before runtime admission.
/// Legacy adoption also records this on the detached PVC; native VCT homes keep
/// the controller owner chain and persist this authority in the home receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolHomeOrigin {
    pub claim: NativePoolClaim,
    pub sandbox_uid: String,
    pub pvc_uid: String,
}

impl NativePoolHomeOrigin {
    pub fn validate_for(
        &self,
        namespace: &str,
        controller: &str,
        request: &HomeVolumeRequest,
    ) -> Result<()> {
        self.claim.request.validate()?;
        let pool = &self.claim.request.pool;
        let config_sha256 = crate::provider_request_digest(&(
            "native-pool-v1",
            namespace,
            controller,
            &pool.pod_policy.runtime_class,
            &pool.pod_policy.service_account,
            &pool.pod_policy.image_pull_secrets,
            &pool.spec,
        ))?;
        let claim_name = format!(
            "cma-claim-{}",
            &crate::provider_request_digest(&(
                controller,
                &request.sandbox_id,
                self.claim.request.runtime_generation,
            ))?[..48]
        );
        if &self.claim.request.home != request
            || pool.config_sha256 != config_sha256
            || self.claim.name != claim_name
            || self.claim.resource_version.is_empty()
            || [&self.claim.uid, &self.sandbox_uid, &self.pvc_uid]
                .into_iter()
                .any(|v| !super::super::model::dns_label(v))
            || pool.name != format!("cma-pool-{}", &pool.config_sha256[..48])
            || pool.template_name != format!("cma-template-{}", &pool.config_sha256[..48])
        {
            return Err(rejected("native home origin does not match its allocation"));
        }
        self.storage_reference()?;
        Ok(())
    }

    pub fn storage_reference(&self) -> Result<String> {
        let name = self
            .claim
            .sandbox_name
            .as_deref()
            .filter(|name| super::super::model::dns_label(name))
            .ok_or_else(|| rejected("native home has no assigned Sandbox"))?;
        Ok(format!("home-{name}"))
    }
}
