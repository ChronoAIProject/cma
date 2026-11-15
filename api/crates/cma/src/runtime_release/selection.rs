//! Exact original targets are selected without editing activation history.
use super::bootstrap::ProcessIdentity;
use crate::{
    release_authority::{Phase, ReleaseHead, ReleaseTarget, WriterIdentity},
    warm_pools::PoolTarget,
};

pub(super) fn selected_writer(
    head: &ReleaseHead,
    process: &ProcessIdentity,
) -> Result<WriterIdentity, String> {
    head.validate().map_err(|error| error.to_string())?;
    let writer = head
        .writer
        .as_ref()
        .ok_or("release head has no selected writer")?;
    if head.phase != Phase::Active
        || writer.deployment != process.deployment
        || writer.instance_id != process.instance_id
        || writer.config_sha256 != process.config_sha256
        || head.pair.agent.image != process.agent_image
        || head.pair.machine.as_ref().map(|target| &target.image) != process.machine_image.as_ref()
        || writer.epoch != head.epoch
        || writer.fence != head.writer_fence
    {
        return Err("release head does not authorize this exact process/configuration/pair".into());
    }
    Ok(writer.clone())
}

pub(crate) fn target_matches(selected: &ReleaseTarget, target: &PoolTarget) -> bool {
    let Some(binding) = &target.binding else {
        return false;
    };
    binding.spec == target.plan.spec
        && binding.pod_policy == target.plan.pod_policy
        && binding.config_sha256 == target.plan.config_sha256
        && selected.target_id == target.id
        && selected.target_revision == target.revision
        && selected.image == target.plan.spec.image.uri
        && selected.plan_sha256 == target.plan.config_sha256
        && selected.pool_name == binding.name
        && selected.pool_uid == binding.uid
        && selected.template_name == binding.template_name
        && selected.template_uid == binding.template_uid
        && target.applied_revision == target.revision
        && target.target_replicas > 0
        && !target.retiring
        && !target.retired
        && target.error.is_none()
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
