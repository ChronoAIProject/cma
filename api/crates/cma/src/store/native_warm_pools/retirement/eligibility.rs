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
    // recording its Claim. Until its home records the selected pool, its durable
    // Sandbox blocks every retirement. Once recorded, it blocks only that pool
    // (below): a single stuck installation must not keep every obsolete pool's
    // spare capacity running.
    if matches!(
        sandbox.status,
        SandboxStatus::Provisioning | SandboxStatus::Installing
    ) && home.is_none_or(|h| h.native_pool.is_none())
    {
        return Ok(Owned::Blocked);
    }
    // Two Desktop sizing plans can use the same image. Once a durable home
    // has selected another exact pool, its image alone does not reserve spare
    // capacity from the superseded plan.
    if target.machine && target.plan.spec.image.uri == image {
        if let Some(native) = home.and_then(|home| home.native_pool.as_ref()) {
            if selected_other_pool(target, &native.request.pool)? {
                return Ok(Owned::Unrelated);
            }
        }
    }
    // A completed cold-canary home uses its own pool even when its image
    // matches an ordinary target. Keep the image fence for incomplete homes.
    if !target.plan.spec.cold_canary
        && home
            .and_then(|home| home.native_pool.as_ref())
            .is_some_and(|native| {
                let pool = &native.request.pool;
                pool.spec.cold_canary
                    && pool.config_sha256 != target.plan.config_sha256
                    && target
                        .binding
                        .as_ref()
                        .is_none_or(|binding| binding.uid != pool.uid && binding.name != pool.name)
                    && sandbox.profile.content.runtime_image.as_deref()
                        == Some(pool.spec.image.uri.as_str())
                    && sandbox
                        .retained_runtime_binding()
                        .is_some_and(|runtime| runtime.runtime_version == pool.spec.image.uri)
            })
        && settled_attachment(sandbox, None)?.is_some()
    {
        return Ok(Owned::Unrelated);
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
        || native
            .request
            .pool
            .pod_policy
            .template_network_policy
            .is_some()
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

fn selected_other_pool(
    target: &PoolTarget,
    pool: &NativePoolBinding,
) -> Result<bool, WarmPoolError> {
    if pool.config_sha256 == target.plan.config_sha256 {
        return Ok(false);
    }
    if target
        .binding
        .as_ref()
        .is_some_and(|binding| binding.uid == pool.uid || binding.name == pool.name)
    {
        return Err(WarmPoolError::Conflict);
    }
    Ok(true)
}

pub(super) fn warm_blocks(
    warm: &WarmSandbox,
    target: &PoolTarget,
    image: &str,
) -> Result<bool, WarmPoolError> {
    if matches!(
        warm.state,
        WarmSandboxState::Claimed | WarmSandboxState::Retired
    ) {
        return Ok(false);
    }
    let native = warm
        .home
        .as_ref()
        .and_then(|home| home.native_pool.as_ref());
    if target.machine && target.plan.spec.image.uri == image {
        if let Some(native) = native {
            if selected_other_pool(target, &native.request.pool)? {
                return Ok(false);
            }
        }
    }
    Ok(warm.plan.runtime_version == target.plan.spec.image.uri
        || native
            .is_some_and(|native| native.request.pool.config_sha256 == target.plan.config_sha256))
}

/// An adopted home cannot consume another spare. Its original definitions may
/// still be needed for recovery even when its runtime is failed or not upgraded.
pub(super) fn settled_native_home(
    sandbox: &Sandbox,
    target: &PoolTarget,
) -> Result<Option<HomeVolumeAttachment>, WarmPoolError> {
    settled_attachment(sandbox, Some(target))
}

fn settled_attachment(
    sandbox: &Sandbox,
    target: Option<&PoolTarget>,
) -> Result<Option<HomeVolumeAttachment>, WarmPoolError> {
    if !matches!(
        sandbox.status,
        SandboxStatus::Active | SandboxStatus::Paused | SandboxStatus::Failed
    ) || sandbox.lifecycle.operation.is_some()
        || sandbox
            .runtime_update_request()
            .is_some_and(|request| request.completed_at.is_none())
    {
        return Ok(None);
    }
    let Some(home) = sandbox.lifecycle.home.as_deref() else {
        return Ok(None);
    };
    let Some(native) = home.native_pool.as_deref() else {
        return Ok(None);
    };
    if let Some(target) = target {
        if native.request.pool.config_sha256 != target.plan.config_sha256 {
            return Ok(None);
        }
        if target.binding.as_ref() != Some(&native.request.pool) {
            return Err(WarmPoolError::Conflict);
        }
    }
    if native.cleaned || native.cleanup.is_some() {
        return Ok(None);
    }
    let Some(attachment) = home.attachment() else {
        return Ok(None);
    };
    let Some(origin) = attachment.receipt.native_authority.as_deref() else {
        return Ok(None);
    };
    // A persisted Claim alone can still be waiting for controller adoption.
    // Require the exact completed Claim/Sandbox/PVC attachment authority.
    attachment
        .validate_for(sandbox.id.as_str())
        .map_err(|_| WarmPoolError::Conflict)?;
    if native.claim.as_ref().is_none_or(|claim| {
        claim.request != origin.claim.request
            || claim.name != origin.claim.name
            || claim.uid != origin.claim.uid
            || claim.sandbox_name != origin.claim.sandbox_name
    }) || origin.claim.request != native.request
    {
        return Err(WarmPoolError::Conflict);
    }
    Ok(Some(attachment))
}
