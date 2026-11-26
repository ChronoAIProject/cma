//! Native home transitions are returned one at a time for the worker to commit.
use std::time::Duration;

use layer2_core::sandbox::{SandboxHome, SandboxNativePool};
use layer3_adapter::agent_sandbox::{
    NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup, NativePoolControl,
};
use layer3_adapter::{AdapterError, RuntimeProvisionRequest, SandboxAdapter};

pub(super) async fn reserve(
    adapter: &dyn SandboxAdapter,
    request: &RuntimeProvisionRequest,
    mut home: SandboxHome,
) -> Result<SandboxHome, AdapterError> {
    home.request = adapter.reserve_home_request(request, &home.request).await?;
    home.validate(&layer2_core::SandboxId(home.request.sandbox_id.clone()))
        .map_err(|error| AdapterError::Rejected(error.to_string()))?;
    let pool = adapter.select_native_pool(request, &home.request).await?;
    if let Some(image) = &request.native_canary_image {
        if pool
            .as_ref()
            .is_none_or(|binding| !binding.spec.cold_canary || &binding.spec.image != image)
        {
            return Err(rejected(
                "canary Sandbox native pool does not match its exact image",
            ));
        }
    }
    if let Some(pool) = pool {
        home.native_pool = Some(Box::new(SandboxNativePool {
            request: NativePoolClaimRequest {
                pool,
                home: home.request.clone(),
                runtime_generation: request.generation,
            },
            claim: None,
            cleanup: None,
            cleaned: false,
        }));
        home.validate(&layer2_core::SandboxId(home.request.sandbox_id.clone()))
            .map_err(|error| AdapterError::Rejected(error.to_string()))?;
    }
    Ok(home)
}

pub(super) async fn next(
    adapter: &dyn SandboxAdapter,
    current: &SandboxHome,
) -> Result<Option<SandboxHome>, AdapterError> {
    let native = current
        .native_pool
        .as_ref()
        .ok_or_else(|| rejected("native home has no reservation"))?;
    if native.cleaned || native.cleanup.is_some() {
        return Err(rejected("native home is already retiring"));
    }
    let pools = adapter
        .native_pools_for_home(&current.request)?
        .ok_or_else(|| rejected("native pool authority is unavailable"))?;
    let mut next = current.clone();
    let reservation = next.native_pool.as_mut().unwrap();
    match &native.claim {
        None => reservation.claim = Some(pools.claim(&native.request).await?),
        Some(original) if original.sandbox_name.is_none() => {
            reservation.claim = Some(observe_assignment(pools, original).await?);
        }
        Some(original) => {
            let attachment = pools.adopt_home(original).await?;
            if attachment.request != current.request {
                return Err(rejected("native claim returned another home allocation"));
            }
            next.bind(attachment.receipt)
                .map_err(|error| rejected(&error.to_string()))?;
        }
    }
    next.validate(&layer2_core::SandboxId(current.request.sandbox_id.clone()))
        .map_err(|error| rejected(&error.to_string()))?;
    Ok(Some(next))
}

async fn observe_assignment(
    pools: &dyn NativePoolControl,
    original: &NativePoolClaim,
) -> Result<NativePoolClaim, AdapterError> {
    // The controller can assign warm capacity just after Claim creation. Observe
    // only this persisted receipt briefly before yielding to durable retry;
    // dependency and identity errors still leave immediately through `?`.
    let readiness = layer3_adapter::agent_sandbox::NativeReadinessObservation::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    tokio::time::timeout_at(deadline, async {
        loop {
            let started = tokio::time::Instant::now();
            // A wake at the deadline must not dispatch another read and clear
            // Pending evidence without any remaining observation budget.
            if started >= deadline {
                return Err(readiness.timeout());
            }
            if let Some(assigned) = readiness
                .observe(async {
                    let assigned = pools.observe_claim(original).await?;
                    Ok(assigned.sandbox_name.is_some().then_some(assigned))
                })
                .await?
            {
                return Ok(assigned);
            }
            // Read latency consumes the polling interval. Anchor each wait to
            // this read, not a ticker, so slow reads never cause catch-up bursts.
            tokio::time::sleep_until(started + Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| readiness.timeout())?
}

pub(super) async fn next_cleanup(
    adapter: &dyn SandboxAdapter,
    current: &SandboxHome,
) -> Result<Option<SandboxHome>, AdapterError> {
    let Some(native) = &current.native_pool else {
        return Ok(None);
    };
    if native.cleaned {
        return Ok(None);
    }
    let pools = adapter
        .native_pools_for_home(&current.request)?
        .ok_or_else(|| rejected("native pool authority is unavailable"))?;
    let original = native.cleanup.clone().unwrap_or_else(|| NativePoolCleanup {
        request: native.request.clone(),
        claim: native.claim.clone(),
        members: Vec::new(),
    });
    let planned = pools.plan_cleanup(&original).await?;
    let mut next = current.clone();
    let reservation = next.native_pool.as_mut().unwrap();
    if native.cleanup.as_ref() != Some(&planned) {
        reservation.cleanup = Some(planned);
    } else {
        pools.cleanup(&planned).await?;
        reservation.cleaned = true;
    }
    Ok(Some(next))
}

fn rejected(message: &str) -> AdapterError {
    AdapterError::Rejected(message.into())
}
