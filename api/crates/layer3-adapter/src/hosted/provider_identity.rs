use super::*;

pub(super) fn select_adoption(
    handle: &RuntimeHandle,
    expected: &ProviderIdentity,
    inventory: Vec<RuntimeObservation>,
) -> crate::Result<Option<RuntimeObservation>> {
    let observation = inventory
        .into_iter()
        .find(|observation| observation.handle == *handle);
    let Some(observation) = observation else {
        return Ok(None);
    };
    if !observation_matches(&observation, expected) {
        return Err(identity_mismatch());
    }
    if !observation.lifecycle.is_running() {
        // Exact identity is proven above. Readiness can change while a pod is
        // replaced; leave durable recovery to reconciliation instead of failing
        // the lifecycle operation permanently.
        return Err(AdapterError::Unavailable(format!(
            "provider runtime {} is not running",
            handle.0
        )));
    }
    Ok(Some(observation))
}

pub(super) fn matching_sandbox(
    request: &ProviderSandboxRequest,
    sandboxes: Vec<ProviderSandbox>,
) -> crate::Result<Option<ProviderSandbox>> {
    let identity = provider_request_identity(request)?;
    let mut candidates = Vec::new();
    for sandbox in sandboxes {
        if sandbox.metadata.get(SANDBOX_METADATA) == Some(&identity.sandbox_id)
            && sandbox.metadata.get(CONTROL_PLANE_METADATA)
                == request.metadata.get(CONTROL_PLANE_METADATA)
        {
            let candidate = provider_metadata_identity(&sandbox.metadata)?;
            if candidate.generation == identity.generation {
                candidates.push(sandbox);
            }
        }
    }
    let mut candidates = candidates.into_iter();
    let Some(sandbox) = candidates.next() else {
        return Ok(None);
    };
    if candidates.next().is_some() {
        return Err(identity_mismatch());
    }
    validate_sandbox_identity(request, sandbox).map(Some)
}

pub(super) fn validate_sandbox_identity(
    request: &ProviderSandboxRequest,
    sandbox: ProviderSandbox,
) -> crate::Result<ProviderSandbox> {
    let identity = provider_request_identity(request)?;
    if invalid_handle(&sandbox.id)
        || provider_metadata_identity(&sandbox.metadata).as_ref() != Ok(&identity)
        || sandbox.metadata.get(CONTROL_PLANE_METADATA)
            != request.metadata.get(CONTROL_PLANE_METADATA)
    {
        return Err(identity_mismatch());
    }
    Ok(sandbox)
}

pub(super) fn creator_inventory(
    created_by: &str,
    sandboxes: Vec<ProviderSandbox>,
) -> crate::Result<Vec<RuntimeObservation>> {
    // A shared provider also hosts generic environments and other services.
    // Only this controller's resources carry its runtime-contract obligations.
    // Filter ownership before decoding that contract; foreign images need no
    // CMA metadata and must never become candidates for orphan reclamation.
    let owned = sandboxes
        .into_iter()
        .filter(|sandbox| {
            sandbox
                .metadata
                .get(CONTROL_PLANE_METADATA)
                .map(String::as_str)
                == Some(created_by)
        })
        .collect();
    provider_inventory(owned)
}

pub(super) fn provider_inventory(
    sandboxes: Vec<ProviderSandbox>,
) -> crate::Result<Vec<RuntimeObservation>> {
    let mut handles = BTreeSet::new();
    let mut generations = BTreeSet::new();
    let mut observations = Vec::with_capacity(sandboxes.len());
    for sandbox in sandboxes {
        if invalid_handle(&sandbox.id) {
            return Err(identity_mismatch());
        }
        let identity = provider_metadata_identity(&sandbox.metadata)?;
        let handle = RuntimeHandle(sandbox.id);
        if !handles.insert(handle.clone())
            || !generations.insert((identity.sandbox_id.clone(), identity.generation))
        {
            return Err(identity_mismatch());
        }
        let lifecycle = match sandbox.status {
            Some(status) if valid_status(&status.state, status.reason.as_deref()) => {
                RuntimeLifecycle {
                    state: if status.state.trim().is_empty() {
                        "unknown".into()
                    } else {
                        status.state
                    },
                    reason: status.reason,
                }
            }
            None => RuntimeLifecycle {
                state: "unknown".into(),
                reason: None,
            },
            Some(_) => {
                return Err(AdapterError::Rejected(
                    "provider sandbox has an invalid lifecycle status".into(),
                ));
            }
        };
        observations.push(RuntimeObservation {
            sandbox_id: identity.sandbox_id,
            generation: identity.generation,
            handle,
            image_sha256: identity.image_sha256,
            spec_fingerprint: identity.spec_fingerprint,
            runtime_contract: identity.runtime_contract,
            lifecycle,
        });
    }
    observations.sort_by(|left, right| {
        (&left.sandbox_id, left.generation, &left.handle).cmp(&(
            &right.sandbox_id,
            right.generation,
            &right.handle,
        ))
    });
    Ok(observations)
}

pub(super) fn observation_matches(
    observation: &RuntimeObservation,
    expected: &ProviderIdentity,
) -> bool {
    observation.sandbox_id == expected.sandbox_id
        && observation.generation == expected.generation
        && observation.image_sha256 == expected.image_sha256
        && observation.spec_fingerprint == expected.spec_fingerprint
        && observation.runtime_contract == expected.runtime_contract
}

pub(super) fn invalid_handle(value: &str) -> bool {
    value.trim().is_empty()
        || value.trim() != value
        || value.len() > 512
        || value.chars().any(char::is_control)
}

pub(super) fn valid_status(state: &str, reason: Option<&str>) -> bool {
    state.len() <= 128
        && !state.chars().any(char::is_control)
        && reason.is_none_or(|reason| {
            reason.chars().count() <= 1_024
                && reason
                    .chars()
                    .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        })
}

pub(super) fn identity_mismatch() -> AdapterError {
    AdapterError::Rejected(
        "provider sandbox identity is ambiguous or belongs to another specification".into(),
    )
}
