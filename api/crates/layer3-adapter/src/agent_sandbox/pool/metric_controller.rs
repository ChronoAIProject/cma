//! Namespace-confined native controller proof for admin diagnostics only.
use super::*;
impl AgentSandboxPools {
    pub(crate) async fn metric_controller(&self, member: &DynamicObject) -> Result<(bool, String)> {
        let refs = member
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default();
        let owner = refs
            .first()
            .filter(|o| {
                refs.len() == 1 && o.controller == Some(true) && o.api_version == API_VERSION
            })
            .ok_or_else(|| rejected("metric member controller absent"))?;
        let (api, unassigned) = match owner.kind.as_str() {
            "SandboxWarmPool" => (&self.pools, true),
            "SandboxClaim" => (&self.claims, false),
            _ => return Err(rejected("unsupported metric controller")),
        };
        let parent = api.get(&owner.name).await.map_err(api_error)?;
        if member.namespace().as_deref() != Some(&self.config.namespace)
            || parent.namespace().as_deref() != Some(&self.config.namespace)
            || parent.uid().as_deref() != Some(&owner.uid)
            || parent.labels().get(OWNER_LABEL) != Some(&self.config.deployment)
            || (unassigned
                && (member.annotations().contains_key("cma/sandbox")
                    || member.annotations().contains_key(NATIVE_HOME_ANNOTATION)))
        {
            return Err(rejected("metric controller UID or deployment changed"));
        }
        Ok((unassigned, owner.uid.clone()))
    }
}
