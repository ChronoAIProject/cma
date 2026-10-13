use kube::{
    api::{DeleteParams, DynamicObject, Preconditions, PropagationPolicy},
    ResourceExt,
};

use super::{
    api_error, create, model, rejected, unavailable, AgentSandboxPools, NativePoolClaim,
    NativePoolClaimRequest, Result,
};

impl AgentSandboxPools {
    /// Only call after storing the request. Name derives from logical Sandbox and
    /// generation, so a changed image/pool cannot allocate a second claim on retry.
    pub async fn claim(&self, request: &NativePoolClaimRequest) -> Result<NativePoolClaim> {
        let desired = model::claim(&self.config, request)?;
        let actual = match self
            .claims
            .get_opt(&desired.name_any())
            .await
            .map_err(api_error)?
        {
            Some(actual) => actual,
            None => {
                self.verify_binding(&request.pool).await?;
                create(&self.claims, &desired).await?
            }
        };
        verify_claim(&actual, &desired)?;
        receipt(request, &actual)
    }

    /// A persisted claim receipt never silently acquires a replacement claim UID.
    pub async fn observe_claim(&self, original: &NativePoolClaim) -> Result<NativePoolClaim> {
        let desired = model::claim(&self.config, &original.request)?;
        if original.name != desired.name_any() || !super::super::model::dns_label(&original.uid) {
            return Err(rejected("native claim receipt is invalid"));
        }
        let actual = self
            .claims
            .get_opt(&original.name)
            .await
            .map_err(api_error)?
            .ok_or_else(|| unavailable("native claim no longer exists"))?;
        verify_claim(&actual, &desired)?;
        if actual.uid().as_deref() != Some(&original.uid) {
            return Err(rejected("native claim UID changed"));
        }
        let current = receipt(&original.request, &actual)?;
        if original.sandbox_name.is_some() && original.sandbox_name != current.sandbox_name {
            return Err(rejected("native claim changed its assigned Sandbox"));
        }
        Ok(current)
    }

    /// Request foreground deletion, then let the lifecycle worker establish both
    /// claim and original owned Pod absence before releasing the adopted home.
    /// A missing claim alone is deliberately not returned as a finality receipt.
    pub async fn delete_claim(&self, original: &NativePoolClaim) -> Result<()> {
        let desired = model::claim(&self.config, &original.request)?;
        if original.name != desired.name_any() || !super::super::model::dns_label(&original.uid) {
            return Err(rejected("native claim receipt is invalid"));
        }
        let Some(actual) = self
            .claims
            .get_opt(&original.name)
            .await
            .map_err(api_error)?
        else {
            return Ok(());
        };
        if actual.uid().as_deref() != Some(&original.uid) {
            return Err(rejected("native claim UID changed before deletion"));
        }
        let mut observed = actual.clone();
        observed.metadata.deletion_timestamp = None;
        verify_claim(&observed, &desired)?;
        if original.sandbox_name.is_some()
            && receipt(&original.request, &observed)?.sandbox_name != original.sandbox_name
        {
            return Err(rejected(
                "native claim changed its assigned Sandbox before deletion",
            ));
        }
        if actual.metadata.deletion_timestamp.is_some() {
            return Ok(());
        }
        let params = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Foreground),
            preconditions: Some(Preconditions {
                uid: actual.uid(),
                resource_version: actual.resource_version(),
            }),
            ..Default::default()
        };
        match self.claims.delete(&original.name, &params).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(error)) if error.code == 404 => Ok(()),
            Err(error) => Err(api_error(error)),
        }
    }
}

pub(super) fn verify_claim(actual: &DynamicObject, desired: &DynamicObject) -> Result<()> {
    model::verify(actual, desired)?;
    // A claim with overrides or expiry no longer has the admitted warm semantics.
    if ["env", "volumeClaimTemplates"].into_iter().any(|key| {
        actual.data["spec"]
            .get(key)
            .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
    }) || ["lifecycle", "additionalPodMetadata"]
        .into_iter()
        .any(|key| {
            actual.data["spec"]
                .get(key)
                .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
        })
    {
        return Err(rejected("native claim contains unadmitted overrides"));
    }
    Ok(())
}

pub(super) fn receipt(
    request: &NativePoolClaimRequest,
    actual: &DynamicObject,
) -> Result<NativePoolClaim> {
    let assigned = actual
        .data
        .pointer("/status/sandbox/name")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty());
    if assigned.is_some_and(|name| !super::super::model::dns_label(name)) {
        return Err(rejected("native claim reports an invalid Sandbox name"));
    }
    Ok(NativePoolClaim {
        request: request.clone(),
        name: actual.name_any(),
        uid: actual
            .uid()
            .ok_or_else(|| rejected("native claim UID missing"))?,
        resource_version: actual
            .resource_version()
            .ok_or_else(|| rejected("native claim version missing"))?,
        sandbox_name: assigned.map(str::to_owned),
    })
}
