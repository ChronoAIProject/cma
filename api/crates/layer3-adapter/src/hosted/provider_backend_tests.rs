use std::collections::BTreeMap;

use super::*;
use crate::provider::{ProviderImage, ProviderSandboxStatus};

fn request(sandbox_id: &str, fingerprint: &str) -> ProviderSandboxRequest {
    ProviderSandboxRequest {
        image: ProviderImage {
            uri: "runtime:test".into(),
        },
        entrypoint: vec!["/runtime".into()],
        env: BTreeMap::new(),
        resource_limits: BTreeMap::new(),
        timeout: None,
        metadata: BTreeMap::from([
            (SANDBOX_METADATA.into(), sandbox_id.into()),
            (SPEC_FINGERPRINT_METADATA.into(), fingerprint.into()),
        ]),
    }
}

fn sandbox(runtime_id: &str, sandbox_id: &str, fingerprint: &str) -> ProviderSandbox {
    ProviderSandbox {
        id: runtime_id.into(),
        status: Some(ProviderSandboxStatus {
            state: "Running".into(),
            reason: None,
        }),
        metadata: BTreeMap::from([
            (SANDBOX_METADATA.into(), sandbox_id.into()),
            (SPEC_FINGERPRINT_METADATA.into(), fingerprint.into()),
        ]),
    }
}

#[test]
fn provider_reconciliation_accepts_one_exact_identity() {
    let matched = matching_sandbox(
        &request("sbx_target", "a"),
        vec![
            sandbox("runtime-other", "sbx_other", "b"),
            sandbox("runtime-target", "sbx_target", "a"),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(matched.id, "runtime-target");
}

#[test]
fn provider_reconciliation_refuses_changed_or_duplicate_identity() {
    let changed = matching_sandbox(
        &request("sbx_target", "expected"),
        vec![sandbox("runtime-target", "sbx_target", "changed")],
    )
    .unwrap_err();
    assert!(matches!(changed, AdapterError::Rejected(detail)
        if detail.contains("another specification")));

    let duplicate = matching_sandbox(
        &request("sbx_target", "expected"),
        vec![
            sandbox("runtime-a", "sbx_target", "expected"),
            sandbox("runtime-b", "sbx_target", "expected"),
        ],
    )
    .unwrap_err();
    assert!(matches!(duplicate, AdapterError::Rejected(detail)
        if detail.contains("ambiguous")));
}

#[test]
fn provider_create_response_requires_the_exact_identity() {
    let expected = request("sbx_target", "expected");
    let exact = validate_sandbox_identity(
        &expected,
        sandbox("runtime-target", "sbx_target", "expected"),
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
        sandbox("runtime-target", "sbx_target", "changed"),
    )
    .unwrap_err();
    let blank =
        validate_sandbox_identity(&expected, sandbox("   ", "sbx_target", "expected")).unwrap_err();
    assert!(matches!(missing, AdapterError::Rejected(detail)
        if detail.contains("another specification")));
    assert!(matches!(changed, AdapterError::Rejected(detail)
        if detail.contains("another specification")));
    assert!(matches!(blank, AdapterError::Rejected(detail)
        if detail.contains("another specification")));
}
