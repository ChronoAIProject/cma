use super::*;

pub(super) enum Owned {
    Unrelated,
    Blocked,
    Retained(Box<HomeVolumeAttachment>),
}

pub(super) fn owned_home(
    sandbox: &Sandbox,
    target: &PoolTarget,
    image: &str,
) -> Result<Owned, WarmPoolError> {
    let home = sandbox.lifecycle.home.as_deref();
    if home.is_some_and(|h| h.released) {
        return Ok(Owned::Unrelated);
    }
    // An incomplete initial allocation can have selected this pool without yet
    // recording its Claim. Its durable Sandbox must block retirement first.
    if matches!(
        sandbox.status,
        SandboxStatus::Provisioning | SandboxStatus::Installing
    ) {
        return Ok(Owned::Blocked);
    }
    if sandbox
        .runtime
        .iter()
        .chain(sandbox.lifecycle.suspended_runtime.as_deref())
        .any(|r| r.runtime_version == target.plan.spec.image.uri)
        || sandbox.runtime_update_request().is_some_and(|r| {
            r.completed_at.is_none() && r.target_version == target.plan.spec.image.uri
        })
    {
        return Ok(Owned::Blocked);
    }
    let Some(home) = home else {
        return Ok(Owned::Unrelated);
    };
    let Some(native) = &home.native_pool else {
        return Ok(Owned::Unrelated);
    };
    if native.request.pool.config_sha256 != target.plan.config_sha256 {
        return Ok(Owned::Unrelated);
    }
    if target.binding.as_ref() != Some(&native.request.pool) {
        return Err(WarmPoolError::Conflict);
    }
    if !matches!(
        sandbox.status,
        SandboxStatus::Active | SandboxStatus::Paused
    ) || sandbox.failure.is_some()
        || sandbox.lifecycle.operation.is_some()
        || sandbox
            .runtime_update_request()
            .is_some_and(|r| r.completed_at.is_none())
        || native.cleaned
        || native.cleanup.is_some()
        || native.claim.is_none()
        || sandbox
            .retained_runtime_binding()
            .is_none_or(|r| r.runtime_version != image)
    {
        return Ok(Owned::Blocked);
    }
    home.attachment()
        .map(|home| Owned::Retained(Box::new(home)))
        .ok_or(WarmPoolError::Conflict)
}

pub(super) fn warm_blocks(warm: &WarmSandbox, target: &PoolTarget) -> bool {
    if matches!(
        warm.state,
        WarmSandboxState::Claimed | WarmSandboxState::Retired
    ) {
        return false;
    }
    warm.plan.runtime_version == target.plan.spec.image.uri
        || warm
            .home
            .as_ref()
            .and_then(|h| h.native_pool.as_ref())
            .is_some_and(|n| n.request.pool.config_sha256 == target.plan.config_sha256)
}
