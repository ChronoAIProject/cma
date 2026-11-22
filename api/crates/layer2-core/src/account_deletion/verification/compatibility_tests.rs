use super::{tests::*, *};
use serde_json::json;

#[test]
fn version_one_binding_and_observation_have_an_exact_wire_shape() {
    let (plan, observations) = fixture();
    let binding = json!({
        "deletion_id": "acd_synthetic",
        "subject_digest": digest("synthetic subject"),
        "requested_at": "2026-11-22T00:00:00Z",
        "contract_digest": digest("contract"),
        "catalogue_digest": digest("catalogue"),
        "policy_digest": digest("policy"),
        "checkpoint": {
            "inventory_generation": 1,
            "effects_digest": digest("settled effects"),
            "effects_completed_at": "2026-11-22T00:01:00Z",
            "writer_drain_digest": digest("qualified writer drain"),
            "writers_closed_at": "2026-11-22T00:00:30Z"
        }
    });
    assert_eq!(serde_json::to_value(&plan.binding).unwrap(), binding);
    assert_eq!(
        serde_json::from_value::<VerificationBinding>(binding).unwrap(),
        plan.binding
    );
    let scope = json!({
        "class": "records", "namespace": "source_0", "authority_scope": digest("authority 0")
    });
    assert_eq!(
        serde_json::to_value(&plan.requirements[0]).unwrap(),
        json!({
            "scope": scope, "disposition": { "kind": "erase" }
        })
    );
    let observation = json!({
        "scope": scope,
        "observation": {
            "kind": "observed_zero",
            "scan": {
                "started_at": "2026-11-22T00:02:00Z",
                "finished_at": "2026-11-22T00:03:00Z",
                "pages": [{
                    "cursor": null, "next_cursor": null, "remaining": 0,
                    "evidence_digest": digest("complete source scan")
                }]
            }
        }
    });
    assert_eq!(
        serde_json::to_value(&observations.observations[0]).unwrap(),
        observation
    );
    assert_eq!(
        serde_json::from_value::<ScopeObservation>(observation).unwrap(),
        observations.observations[0]
    );
    let plan: CoveragePlan = serde_json::from_value(serde_json::to_value(plan).unwrap()).unwrap();
    let observations: CoverageObservations =
        serde_json::from_value(serde_json::to_value(observations).unwrap()).unwrap();
    plan.validate(&observations, NOW).unwrap();
}

#[test]
fn strict_reader_rejects_unknown_fields_classes_and_observation_formats() {
    let (_, observations) = fixture();
    let original = serde_json::to_value(&observations).unwrap();
    for path in [
        "",
        "/binding",
        "/binding/checkpoint",
        "/observations/0",
        "/observations/0/scope",
        "/observations/0/observation",
        "/observations/0/observation/scan",
        "/observations/0/observation/scan/pages/0",
    ] {
        let mut changed = original.clone();
        changed
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("future_field".into(), json!(true));
        assert!(
            serde_json::from_value::<CoverageObservations>(changed).is_err(),
            "{path}"
        );
    }
    for (path, value) in [
        ("/observations/0/scope/class", "unclassified"),
        ("/observations/0/observation/kind", "assumed_zero"),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).unwrap() = json!(value);
        assert!(serde_json::from_value::<CoverageObservations>(changed).is_err());
    }
}

#[test]
fn future_versions_are_refused_even_when_both_sides_agree() {
    let (mut plan, mut observations) = fixture();
    for (expected, observed) in [(2, 1), (1, 2), (2, 2), (0, 0)] {
        plan.version = expected;
        observations.version = observed;
        assert_eq!(
            plan.validate(&observations, NOW).unwrap_err(),
            CoverageError::UnsupportedVersion
        );
    }
}

#[test]
fn legacy_zero_projection_cannot_be_read_as_a_coverage_proof() {
    let legacy = json!({
        "records": {}, "objects": 0, "homes": [],
        "keys": { "remaining": 0, "empty_at": "2026-11-22T00:00:00Z" }
    });
    let report: crate::account_deletion::ResidueReport =
        serde_json::from_value(legacy.clone()).unwrap();
    assert!(report.is_zero());
    assert!(serde_json::from_value::<CoverageObservations>(legacy).is_err());
    let (_, observations) = fixture();
    assert!(
        serde_json::from_value::<crate::account_deletion::ResidueReport>(
            serde_json::to_value(observations).unwrap()
        )
        .is_err()
    );
}

#[test]
fn evidence_fingerprints_cannot_be_missing_empty_truncated_or_noncanonical() {
    for value in [
        "".to_owned(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        assert!(serde_json::from_value::<ErasureDigest>(json!(value)).is_err());
    }
    let (_, observations) = fixture();
    let original = serde_json::to_value(&observations).unwrap();
    for field in [
        "subject_digest",
        "contract_digest",
        "catalogue_digest",
        "policy_digest",
    ] {
        let mut changed = original.clone();
        changed["binding"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<CoverageObservations>(changed).is_err());
    }
    for field in ["effects_digest", "writer_drain_digest"] {
        let mut changed = original.clone();
        changed["binding"]["checkpoint"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<CoverageObservations>(changed).is_err());
    }
}
