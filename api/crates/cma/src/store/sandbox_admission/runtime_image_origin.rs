//! Capture only image intent retained by the operation that owns this profile.
use super::SandboxAdmission;
use crate::store::agent_lifecycle_operations::LifecycleRecord;
use layer2_core::{
    profiles::private_sandbox_profile_id,
    released_runtime::ReleasedLane,
    sandbox::{SandboxOrigin, SandboxRuntimeImageOrigin},
};

pub(super) fn selection(
    admission: &SandboxAdmission,
    row: &LifecycleRecord,
) -> Option<SandboxRuntimeImageOrigin> {
    let sandbox = &admission.sandbox;
    let SandboxOrigin::Lifecycle { operation_id } = &sandbox.origin else {
        return None;
    };
    let image = sandbox.profile.content.runtime_image.as_deref()?;
    let request = &row.input.request;
    let profile_key = format!("agent-manager:{}:sandbox", row.key);
    if row.document_schema_version != crate::store::schema::DOCUMENT_SCHEMA_VERSION
        || crate::store::agent_lifecycle_operations::canary_key::is_reserved(&row.key)
        || row.id != *operation_id
        || row.owner != sandbox.owner.as_str()
        || !row.input.behavior.uses_repository_profile()
        || row.status != "running"
        || row.sandbox_id.is_some()
        || row.lease_token.as_deref() != Some(admission.lifecycle_lease()?)
        || row.input.resolved_image.as_deref() != Some(image)
        || request.workspace_id.is_some()
        || request.sandbox_id.is_some()
        || admission.binding.is_some()
        || private_sandbox_profile_id(&sandbox.owner, &profile_key).ok()? != sandbox.profile.id
        || sandbox.profile.revision != 1
    {
        return None;
    }
    if request.image.is_some() {
        return Some(SandboxRuntimeImageOrigin::UserSelected);
    }
    // The default Desktop path resolves the qualified Agent image at operation
    // admission. Pointer-backed deployments also fence that release and record its lane.
    (request.desktop
        && !request.machine_included
        && row.released_lane != Some(ReleasedLane::Machine))
    .then_some(SandboxRuntimeImageOrigin::ManagedDesktop)
}
