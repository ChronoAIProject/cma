//! Fixed, content-free categories for provider identity proof failures.
use super::super::deployment::ProviderIdentity;
use crate::{AdapterError, RuntimeHandle, RuntimeObservation};

pub(super) fn exact_identity_predicate(
    observation: &RuntimeObservation,
    handle: &RuntimeHandle,
    expected: &ProviderIdentity,
) -> Option<&'static str> {
    [
        (observation.handle == *handle, "handle"),
        (observation.sandbox_id == expected.sandbox_id, "sandbox"),
        (observation.generation == expected.generation, "generation"),
        (observation.image_sha256 == expected.image_sha256, "image"),
        (
            observation.spec_fingerprint == expected.spec_fingerprint,
            "spec",
        ),
        (
            observation.runtime_contract == expected.runtime_contract,
            "contract",
        ),
        (observation.lifecycle.is_running(), "lifecycle"),
    ]
    .into_iter()
    .find_map(|(matches, predicate)| (!matches).then_some(predicate))
}

pub(super) fn identity_rejection(predicate: &'static str) -> AdapterError {
    tracing::warn!(predicate, "provider runtime identity proof failed");
    AdapterError::Rejected("provider did not prove the exact running runtime identity".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected() -> ProviderIdentity {
        ProviderIdentity {
            sandbox_id: "sandbox".into(),
            generation: 3,
            image_sha256: "a".repeat(43),
            spec_fingerprint: "b".repeat(43),
            runtime_contract: "contract-3".into(),
        }
    }

    #[test]
    fn identity_predicate_reports_only_fixed_categories() {
        let expected = expected();
        let handle = RuntimeHandle::new("runtime");
        let observation = RuntimeObservation {
            sandbox_id: expected.sandbox_id.clone(),
            generation: expected.generation,
            handle: handle.clone(),
            image_sha256: expected.image_sha256.clone(),
            spec_fingerprint: expected.spec_fingerprint.clone(),
            runtime_contract: expected.runtime_contract.clone(),
            lifecycle: crate::RuntimeLifecycle {
                state: "running".into(),
                reason: None,
            },
        };
        assert_eq!(
            exact_identity_predicate(&observation, &handle, &expected),
            None
        );
        let mut changed = observation;
        changed.generation = 4;
        assert_eq!(
            exact_identity_predicate(&changed, &handle, &expected),
            Some("generation")
        );
    }
}
