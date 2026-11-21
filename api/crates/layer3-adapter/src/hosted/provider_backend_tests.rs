use std::collections::BTreeMap;

use super::*;
use crate::hosted::deployment::{
    image_sha256, RUNTIME_CONTRACT_METADATA, RUNTIME_GENERATION_METADATA, RUNTIME_IMAGE_METADATA,
    SPEC_FINGERPRINT_METADATA,
};
use crate::provider::{ProviderImage, ProviderSandboxStatus};
use crate::RUNTIME_CONTRACT_VERSION;

fn fingerprint(byte: char) -> String {
    byte.to_string().repeat(43)
}

fn metadata(sandbox_id: &str, generation: u64, fingerprint: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (SANDBOX_METADATA.into(), sandbox_id.into()),
        (RUNTIME_GENERATION_METADATA.into(), generation.to_string()),
        (RUNTIME_IMAGE_METADATA.into(), image_sha256("runtime:test")),
        (SPEC_FINGERPRINT_METADATA.into(), fingerprint.into()),
        (
            RUNTIME_CONTRACT_METADATA.into(),
            RUNTIME_CONTRACT_VERSION.into(),
        ),
    ])
}

fn request(sandbox_id: &str, generation: u64, fingerprint: &str) -> ProviderSandboxRequest {
    ProviderSandboxRequest {
        funded_until: None,
        image: ProviderImage {
            uri: "runtime:test".into(),
        },
        entrypoint: vec!["/runtime".into()],
        env: BTreeMap::new(),
        resource_limits: BTreeMap::new(),
        timeout: None,
        metadata: metadata(sandbox_id, generation, fingerprint),
        volumes: Vec::new(),
    }
}

fn sandbox(
    runtime_id: &str,
    sandbox_id: &str,
    generation: u64,
    fingerprint: &str,
) -> ProviderSandbox {
    ProviderSandbox {
        id: runtime_id.into(),
        status: Some(ProviderSandboxStatus {
            state: "Running".into(),
            reason: None,
        }),
        metadata: metadata(sandbox_id, generation, fingerprint),
    }
}

#[test]
fn provider_reconciliation_accepts_one_exact_generation() {
    let target = fingerprint('a');
    let matched = matching_sandbox(
        &request("sbx_target", 2, &target),
        vec![
            sandbox("runtime-old", "sbx_target", 1, &fingerprint('b')),
            sandbox("runtime-target", "sbx_target", 2, &target),
            sandbox("runtime-other", "sbx_other", 2, &fingerprint('c')),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(matched.id, "runtime-target");
}

#[test]
fn provider_reconciliation_refuses_changed_or_duplicate_generation() {
    let expected = fingerprint('a');
    let changed = matching_sandbox(
        &request("sbx_target", 2, &expected),
        vec![sandbox(
            "runtime-target",
            "sbx_target",
            2,
            &fingerprint('b'),
        )],
    )
    .unwrap_err();
    assert!(matches!(changed, AdapterError::Rejected(detail)
        if detail.contains("another specification")));

    let duplicate = matching_sandbox(
        &request("sbx_target", 2, &expected),
        vec![
            sandbox("runtime-a", "sbx_target", 2, &expected),
            sandbox("runtime-b", "sbx_target", 2, &expected),
        ],
    )
    .unwrap_err();
    assert!(matches!(duplicate, AdapterError::Rejected(detail)
        if detail.contains("ambiguous")));
}

#[test]
fn provider_create_response_requires_the_exact_identity() {
    let expected_fingerprint = fingerprint('a');
    let expected = request("sbx_target", 3, &expected_fingerprint);
    let exact = validate_sandbox_identity(
        &expected,
        sandbox("runtime-target", "sbx_target", 3, &expected_fingerprint),
    )
    .unwrap();
    assert_eq!(exact.id, "runtime-target");

    let missing = validate_sandbox_identity(
        &expected,
        ProviderSandbox {
            id: "runtime-target".into(),
            status: None,
            metadata: BTreeMap::new(),
        },
    )
    .unwrap_err();
    let changed = validate_sandbox_identity(
        &expected,
        sandbox("runtime-target", "sbx_target", 4, &expected_fingerprint),
    )
    .unwrap_err();
    let blank = validate_sandbox_identity(
        &expected,
        sandbox("   ", "sbx_target", 3, &expected_fingerprint),
    )
    .unwrap_err();
    for error in [missing, changed, blank] {
        assert!(matches!(error, AdapterError::Rejected(detail)
            if detail.contains("another specification") || detail.contains("ambiguous")));
    }
}

#[test]
fn complete_inventory_is_sorted_and_preserves_unknown_lifecycle() {
    let mut pending = sandbox("runtime-b", "sbx_b", 4, &fingerprint('b'));
    pending.status = None;
    let inventory = provider_inventory(vec![
        pending,
        sandbox("runtime-a", "sbx_a", 2, &fingerprint('a')),
    ])
    .unwrap();

    assert_eq!(inventory.len(), 2);
    assert_eq!(inventory[0].sandbox_id, "sbx_a");
    assert_eq!(inventory[0].generation, 2);
    assert!(inventory[0].lifecycle.is_running());
    assert_eq!(inventory[1].lifecycle.state, "unknown");
}

#[test]
fn shared_provider_inventory_parses_only_this_controllers_runtimes() {
    let mut owned = sandbox("runtime-owned", "sbx-owned", 1, &fingerprint('a'));
    owned
        .metadata
        .insert(CONTROL_PLANE_METADATA.into(), "legacy-cma".into());
    let generic = ProviderSandbox {
        id: "generic-python".into(),
        status: None,
        metadata: BTreeMap::from([(CONTROL_PLANE_METADATA.into(), "sandbox-manager".into())]),
    };
    let unlabelled = ProviderSandbox {
        id: "unrelated".into(),
        status: None,
        metadata: BTreeMap::new(),
    };
    // The foreign runtime deliberately lacks every CMA runtime-contract field.
    let inventory = creator_inventory(
        "legacy-cma",
        vec![generic.clone(), owned.clone(), unlabelled],
    )
    .unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].handle.as_str(), "runtime-owned");
    assert!(creator_inventory("sandbox-manager", vec![generic]).is_err());
    owned.metadata.remove(RUNTIME_GENERATION_METADATA);
    assert!(creator_inventory("legacy-cma", vec![owned]).is_err());
}

#[test]
fn another_creators_matching_sandbox_id_cannot_be_adopted_or_bound() {
    let fingerprint = fingerprint('a');
    let mut expected = request("sbx", 1, &fingerprint);
    expected
        .metadata
        .insert(CONTROL_PLANE_METADATA.into(), "legacy-cma".into());
    let mut foreign = sandbox("foreign-runtime", "sbx", 1, &fingerprint);
    foreign
        .metadata
        .insert(CONTROL_PLANE_METADATA.into(), "sandbox-manager".into());
    assert!(matching_sandbox(&expected, vec![foreign.clone()])
        .unwrap()
        .is_none());
    assert!(validate_sandbox_identity(&expected, foreign).is_err());
}

#[test]
fn inventory_fails_closed_on_unlabelled_malformed_or_duplicate_rows() {
    let unlabelled = ProviderSandbox {
        id: "foreign".into(),
        status: None,
        metadata: BTreeMap::new(),
    };
    assert!(matches!(
        provider_inventory(vec![unlabelled]),
        Err(AdapterError::Rejected(_))
    ));

    let mut malformed = sandbox("runtime-a", "sbx", 1, &fingerprint('a'));
    malformed
        .metadata
        .insert(RUNTIME_GENERATION_METADATA.into(), "01".into());
    assert!(matches!(
        provider_inventory(vec![malformed]),
        Err(AdapterError::Rejected(_))
    ));

    let duplicate_generation = vec![
        sandbox("runtime-a", "sbx", 1, &fingerprint('a')),
        sandbox("runtime-b", "sbx", 1, &fingerprint('b')),
    ];
    assert!(matches!(
        provider_inventory(duplicate_generation),
        Err(AdapterError::Rejected(_))
    ));

    let duplicate_handle = vec![
        sandbox("runtime-a", "sbx-a", 1, &fingerprint('a')),
        sandbox("runtime-a", "sbx-b", 1, &fingerprint('b')),
    ];
    assert!(matches!(
        provider_inventory(duplicate_handle),
        Err(AdapterError::Rejected(_))
    ));
}

#[test]
fn exact_adoption_distinguishes_absence_identity_mismatch_and_non_running_state() {
    let expected_fingerprint = fingerprint('a');
    let request = request("sbx", 2, &expected_fingerprint);
    let expected = provider_request_identity(&request).unwrap();
    let handle = RuntimeHandle::new("runtime-a");
    let running = provider_inventory(vec![sandbox(
        handle.as_str(),
        "sbx",
        2,
        &expected_fingerprint,
    )])
    .unwrap();
    assert_eq!(
        select_adoption(&handle, &expected, running)
            .unwrap()
            .unwrap()
            .handle,
        handle
    );

    assert!(
        select_adoption(&RuntimeHandle::new("absent"), &expected, Vec::new())
            .unwrap()
            .is_none()
    );

    let mismatched = provider_inventory(vec![sandbox(
        handle.as_str(),
        "sbx",
        3,
        &expected_fingerprint,
    )])
    .unwrap();
    assert!(matches!(
        select_adoption(&handle, &expected, mismatched),
        Err(AdapterError::Rejected(_))
    ));

    for state in ["Pending", "Stopped", "Failed"] {
        let mut unavailable = sandbox(handle.as_str(), "sbx", 2, &expected_fingerprint);
        unavailable.status.as_mut().unwrap().state = state.into();
        assert!(matches!(
            select_adoption(&handle, &expected, provider_inventory(vec![unavailable]).unwrap()),
            Err(AdapterError::Unavailable(detail)) if detail.contains("not running")
        ));
        let mut mismatched = sandbox(handle.as_str(), "sbx", 3, &expected_fingerprint);
        mismatched.status.as_mut().unwrap().state = state.into();
        assert!(matches!(
            select_adoption(
                &handle,
                &expected,
                provider_inventory(vec![mismatched]).unwrap()
            ),
            Err(AdapterError::Rejected(_))
        ));
    }
}
