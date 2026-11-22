use super::*;

mod policy;
mod scans;

pub(super) const NOW: &str = "2026-11-22T01:00:00Z";

pub(super) fn digest(text: &str) -> ErasureDigest {
    crate::document_artifacts::digest(text.as_bytes())
        .try_into()
        .unwrap()
}

pub(super) fn scan() -> CompleteScan {
    CompleteScan {
        started_at: "2026-11-22T00:02:00Z".into(),
        finished_at: "2026-11-22T00:03:00Z".into(),
        pages: vec![ScanPage {
            cursor: None,
            next_cursor: None,
            remaining: 0,
            evidence_digest: digest("complete source scan"),
        }],
    }
}

pub(super) fn fixture() -> (CoveragePlan, CoverageObservations) {
    let plan = CoveragePlan {
        version: ERASURE_COVERAGE_VERSION,
        binding: VerificationBinding {
            deletion_id: AccountDeletionId("acd_synthetic".into()),
            subject_digest: digest("synthetic subject"),
            requested_at: "2026-11-22T00:00:00Z".into(),
            contract_digest: digest("contract"),
            catalogue_digest: digest("catalogue"),
            policy_digest: digest("policy"),
            checkpoint: ErasureEffectCheckpoint {
                inventory_generation: 1,
                effects_digest: digest("settled effects"),
                effects_completed_at: "2026-11-22T00:01:00Z".into(),
                writer_drain_digest: digest("qualified writer drain"),
                writers_closed_at: "2026-11-22T00:00:30Z".into(),
            },
        },
        requirements: REQUIRED_RESIDUE_CLASSES
            .into_iter()
            .enumerate()
            .map(|(index, class)| CoverageRequirement {
                scope: ResidueScope {
                    class,
                    namespace: format!("source_{index}"),
                    authority_scope: digest(&format!("authority {index}")),
                },
                disposition: RequiredDisposition::Erase,
            })
            .collect(),
    };
    let observations = CoverageObservations {
        version: ERASURE_COVERAGE_VERSION,
        binding: plan.binding.clone(),
        observations: plan
            .requirements
            .iter()
            .map(|entry| ScopeObservation {
                scope: entry.scope.clone(),
                observation: ResidueObservation::ObservedZero { scan: scan() },
            })
            .collect(),
    };
    (plan, observations)
}

#[test]
fn accepts_complete_coverage_in_any_order_without_minting_a_completion_receipt() {
    let (plan, mut observations) = fixture();
    observations.observations.reverse();
    let checked = plan.validate(&observations, NOW).unwrap();
    assert_eq!(checked.observations(), &observations);
}

#[test]
fn empty_catalogue_or_omitted_class_never_proves_zero() {
    let (mut plan, mut observations) = fixture();
    for class in REQUIRED_RESIDUE_CLASSES {
        let mut missing = plan.clone();
        missing
            .requirements
            .retain(|entry| entry.scope.class != class);
        assert_eq!(
            missing.validate(&observations, NOW).unwrap_err(),
            CoverageError::MissingClass(class)
        );
    }
    plan.requirements.clear();
    observations.observations.clear();
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::MissingClass(ResidueClass::Records)
    );
}

#[test]
fn every_namespace_and_provider_scope_must_be_observed_exactly_once() {
    let (mut plan, mut observations) = fixture();
    let mut second = plan.requirements[0].clone();
    second.scope.authority_scope = digest("another provider scope");
    plan.requirements.push(second.clone());
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::MissingScope
    );
    observations.observations.push(ScopeObservation {
        scope: second.scope,
        observation: ResidueObservation::ObservedZero { scan: scan() },
    });
    plan.validate(&observations, NOW).unwrap();
    for index in 0..observations.observations.len() {
        let mut missing = observations.clone();
        missing.observations.remove(index);
        assert_eq!(
            plan.validate(&missing, NOW).unwrap_err(),
            CoverageError::MissingScope
        );
    }
    observations
        .observations
        .push(observations.observations[0].clone());
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::DuplicateScope
    );
    observations.observations.pop();
    observations.observations[0].scope.namespace = "unclassified".into();
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::UnclassifiedScope
    );
    plan.requirements.push(plan.requirements[0].clone());
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::DuplicateScope
    );
}

#[test]
fn changed_identity_policy_inventory_effect_or_drain_rejects_old_observations() {
    let (plan, observations) = fixture();
    let original = serde_json::to_value(&plan).unwrap();
    for (path, replacement) in [
        ("/binding/deletion_id", serde_json::json!("acd_another")),
        (
            "/binding/subject_digest",
            serde_json::json!(digest("another subject")),
        ),
        (
            "/binding/requested_at",
            serde_json::json!("2026-11-21T00:00:00Z"),
        ),
        (
            "/binding/contract_digest",
            serde_json::json!(digest("another contract")),
        ),
        (
            "/binding/catalogue_digest",
            serde_json::json!(digest("another catalogue")),
        ),
        (
            "/binding/policy_digest",
            serde_json::json!(digest("another policy")),
        ),
        (
            "/binding/checkpoint/inventory_generation",
            serde_json::json!(2),
        ),
        (
            "/binding/checkpoint/effects_digest",
            serde_json::json!(digest("new effect")),
        ),
        (
            "/binding/checkpoint/effects_completed_at",
            serde_json::json!("2026-11-22T00:04:00Z"),
        ),
        (
            "/binding/checkpoint/writer_drain_digest",
            serde_json::json!(digest("new drain")),
        ),
        (
            "/binding/checkpoint/writers_closed_at",
            serde_json::json!("2026-11-22T00:04:00Z"),
        ),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(path).unwrap() = replacement;
        let changed: CoveragePlan = serde_json::from_value(value).unwrap();
        assert_eq!(
            changed.validate(&observations, NOW).unwrap_err(),
            CoverageError::BindingChanged,
            "{path}"
        );
    }
}

#[test]
fn malformed_plan_is_rejected_even_if_observations_copy_its_binding() {
    let (plan, observations) = fixture();
    for id in ["", "contains spaces", "contains/slash"] {
        let mut plan = plan.clone();
        plan.binding.deletion_id.0 = id.into();
        let mut observations = observations.clone();
        observations.binding = plan.binding.clone();
        assert_eq!(
            plan.validate(&observations, NOW).unwrap_err(),
            CoverageError::InvalidBinding
        );
    }
    for namespace in ["", "UPPERCASE", "has space", "secret/value"] {
        let mut plan = plan.clone();
        plan.requirements[0].scope.namespace = namespace.into();
        assert_eq!(
            plan.validate(&observations, NOW).unwrap_err(),
            CoverageError::InvalidScope
        );
    }
    let mut plan = plan;
    plan.binding.checkpoint.inventory_generation = 0;
    let mut observations = observations;
    observations.binding = plan.binding.clone();
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::InvalidBinding
    );
}

#[test]
fn malformed_or_preeffect_timestamps_cannot_be_repaired_by_zero_counts() {
    let (plan, mut observations) = fixture();
    for path in [
        "/binding/requested_at",
        "/binding/checkpoint/effects_completed_at",
        "/binding/checkpoint/writers_closed_at",
    ] {
        for stamp in ["invalid", "1960-01-01T00:00:00Z", "2026-11-23T00:00:00Z"] {
            let mut value = serde_json::to_value(&plan).unwrap();
            *value.pointer_mut(path).unwrap() = serde_json::json!(stamp);
            let changed: CoveragePlan = serde_json::from_value(value).unwrap();
            observations.binding = changed.binding.clone();
            assert_eq!(
                changed.validate(&observations, NOW).unwrap_err(),
                CoverageError::InvalidTime
            );
        }
    }
}

#[test]
fn coverage_capacity_is_checked_before_duplicate_or_missing_class_validation() {
    let (mut plan, observations) = fixture();
    plan.requirements = vec![plan.requirements[0].clone(); MAX_ERASURE_COVERAGE_SCOPES + 1];
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::CapacityExceeded
    );
    let (plan, mut observations) = fixture();
    observations.observations =
        vec![observations.observations[0].clone(); MAX_ERASURE_COVERAGE_SCOPES + 1];
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::CapacityExceeded
    );
}
