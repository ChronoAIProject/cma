//! Target generations and capacity change atomically on the original pool UID.
use super::{
    api_error, management, model, rejected, retirement, AgentSandboxPools, NativePoolBinding,
    NativePoolStatus, Result,
};
use kube::{
    api::{DynamicObject, Patch, PatchParams},
    ResourceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

const ANNOTATION: &str = "cma/native-target-v1";

/// The caller freezes this from its durable target, before any provider IO.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolTarget {
    pub target_id: String,
    pub revision: i64,
    pub replicas: u32,
}

impl NativePoolTarget {
    fn validate(&self) -> Result<()> {
        if self.revision < 1
            || self.replicas > 1000
            || self.target_id.strip_prefix("wp_").is_none_or(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
            })
        {
            return Err(rejected("native pool target authority is invalid"));
        }
        Ok(())
    }
}

fn retained(pool: &DynamicObject) -> Result<Option<NativePoolTarget>> {
    pool.annotations()
        .get(ANNOTATION)
        .map(|encoded| {
            if encoded.len() > 1024 {
                return Err(rejected("native pool target watermark exceeds its bound"));
            }
            let target: NativePoolTarget = serde_json::from_str(encoded)
                .map_err(|_| rejected("native pool target watermark is invalid"))?;
            target.validate()?;
            Ok(target)
        })
        .transpose()
}

/// Participating configure/legacy callers cannot erase an adopted watermark.
/// The permanent retirement marker permits only the terminal transition to zero.
pub(super) fn allow_unversioned(pool: &DynamicObject, replicas: u32) -> Result<()> {
    if retained(pool)?.is_some()
        && management::target(pool)? != replicas
        && !(replicas == 0
            && pool
                .annotations()
                .get(retirement::RETIRING)
                .is_some_and(|value| value == "true"))
    {
        return Err(rejected(
            "native pool resize requires its durable target generation",
        ));
    }
    Ok(())
}

pub(super) fn check_target_authority(
    pool: &DynamicObject,
    target: &NativePoolTarget,
) -> Result<bool> {
    target.validate()?;
    if let Some(previous) = retained(pool)? {
        if previous.target_id != target.target_id
            || previous.revision > target.revision
            || (previous.revision == target.revision && previous != *target)
        {
            return Err(rejected("native pool target generation was superseded"));
        }
        let capacity = management::target(pool)?;
        let retired = capacity == 0
            && pool
                .annotations()
                .get(retirement::RETIRING)
                .is_some_and(|value| value == "true");
        if capacity != previous.replicas && !retired {
            return Err(rejected("native pool target changed outside its watermark"));
        }
        return Ok(previous == *target && capacity == target.replicas);
    }
    Ok(false)
}

impl AgentSandboxPools {
    pub async fn resize_fenced(
        &self,
        binding: &NativePoolBinding,
        target: &NativePoolTarget,
    ) -> Result<NativePoolStatus> {
        target.validate()?;
        let resources = self
            .bound_resources_with(binding, |pool| {
                check_target_authority(pool, target).map(|_| ())
            })
            .await;
        if target.replicas > 0
            && matches!(
                &resources,
                Err(crate::AdapterError::NativePoolConfigurationDrift)
            )
        {
            self.verify_seed(&binding.spec.home).await?;
        }
        let (template, actual) = resources?;
        if target.replicas > 0 {
            retirement::admitting(&actual)?;
        }
        if check_target_authority(&actual, target)? {
            return management::status(binding.clone(), &actual);
        }
        if target.replicas > 0 {
            self.verify_seed(&binding.spec.home).await?;
        }
        let config = model::binding_config(&self.config, binding);
        let desired = model::pool(&config, &binding.spec, &template, target.replicas)?;
        let encoded = serde_json::to_string(target)
            .map_err(|_| rejected("native pool target cannot be encoded"))?;
        // The same resourceVersion fences both fields. Never retry a failed CAS
        // with a refreshed version after discarding the original generation.
        let patch = json!({"metadata":{
            "uid":actual.uid(),"resourceVersion":actual.resource_version(),
            "annotations":{ANNOTATION:encoded}},"spec":{"replicas":target.replicas}});
        let updated = match self
            .pools
            .patch(&binding.name, &PatchParams::default(), &Patch::Merge(patch))
            .await
        {
            Ok(updated) => updated,
            Err(error) => {
                let current = self.pools.get_opt(&binding.name).await.map_err(api_error)?;
                match current {
                    Some(current) if exact(&current, &actual, &desired, target)? => current,
                    _ => return Err(api_error(error)),
                }
            }
        };
        if !exact(&updated, &actual, &desired, target)? {
            return Err(rejected("native pool target acknowledgement is not exact"));
        }
        tracing::info!(
            revision = target.revision,
            replicas = target.replicas,
            "native pool target generation acknowledged"
        );
        management::status(binding.clone(), &updated)
    }
}

fn exact(
    actual: &DynamicObject,
    original: &DynamicObject,
    desired: &DynamicObject,
    target: &NativePoolTarget,
) -> Result<bool> {
    if target.replicas > 0 {
        retirement::admitting(actual)?;
    }
    Ok(actual.uid() == original.uid()
        && retained(actual)?.as_ref() == Some(target)
        && management::target(actual)? == target.replicas
        && model::verify(actual, desired).is_ok())
}

#[cfg(test)]
mod tests;
