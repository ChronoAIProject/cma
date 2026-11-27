use super::*;

#[test]
fn pool_images_must_explicitly_support_both_runtime_and_assignment_contracts() {
    let mut image = crate::RuntimeImage {
        uri: "registry.example/runtime@sha256:fixture".into(),
        labels: BTreeMap::from([(crate::RUNTIME_CONTRACT_LABEL.into(), "3".into())]),
    };
    assert!(admit_pool_image(&image).is_err());
    image
        .labels
        .insert(RUNTIME_BOOTSTRAP_LABEL.into(), "2".into());
    assert!(admit_pool_image(&image).is_err());
    image
        .labels
        .insert(RUNTIME_BOOTSTRAP_LABEL.into(), "1".into());
    admit_pool_image(&image).unwrap();
    image
        .labels
        .insert(crate::RUNTIME_CONTRACT_LABEL.into(), "2".into());
    assert!(admit_pool_image(&image).is_err());
}

fn request() -> RuntimeBootstrapRequest {
    RuntimeBootstrapRequest {
        version: RUNTIME_BOOTSTRAP_VERSION,
        staging_startup_failure: None,
        staging_startup_confirmed: false,
        pod_uid: "original-pod".into(),
        environment: BTreeMap::from([
            ("CHRONO_SANDBOX_ID".into(), "sbx_original".into()),
            ("CHRONO_RUNTIME_GENERATION".into(), "1".into()),
            ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
            ("CHRONO_RUNTIME_CONTROL_TOKEN".into(), "b".repeat(64)),
        ]),
    }
}

#[test]
fn assignment_and_receipt_pin_pod_identity_and_all_configuration_without_debug_disclosure() {
    let original = request();
    let receipt = RuntimeBootstrapReceipt::for_request(&original).unwrap();
    receipt.validate_for(&original).unwrap();
    assert_eq!(
        serde_json::from_slice::<RuntimeBootstrapRequest>(&serde_json::to_vec(&original).unwrap())
            .unwrap(),
        original
    );
    assert!(!format!("{original:?}").contains(&"b".repeat(64)));
    for field in [
        "CHRONO_SANDBOX_ID",
        "CHRONO_RUNTIME_HOME_IDENTITY",
        "CHRONO_RUNTIME_CONTROL_TOKEN",
        "CHRONO_RUNTIME_GENERATION",
    ] {
        let mut changed = original.clone();
        changed
            .environment
            .get_mut(field)
            .unwrap()
            .replace_range(..1, "2");
        assert!(receipt.validate_for(&changed).is_err(), "{field}");
    }
    let mut changed = original.clone();
    changed.pod_uid = "replacement-pod".into();
    assert!(receipt.validate_for(&changed).is_err());
}

#[test]
fn assignment_refuses_missing_authority_reserved_bootstrap_overrides_and_oversized_input() {
    for field in [
        "CHRONO_SANDBOX_ID",
        "CHRONO_RUNTIME_HOME_IDENTITY",
        "CHRONO_RUNTIME_CONTROL_TOKEN",
        "CHRONO_RUNTIME_GENERATION",
    ] {
        let mut changed = request();
        changed.environment.remove(field);
        assert!(changed.validate().is_err(), "{field}");
    }
    for (name, value) in [
        ("CHRONO_RUNTIME_GENERATION", "0"),
        ("CHRONO_RUNTIME_GENERATION", "01"),
        ("CHRONO_RUNTIME_GENERATION", "-1"),
        ("CHRONO_RUNTIME_POD_UID", "spoofed"),
        ("CHRONO_RUNTIME_BOOTSTRAP_SOCKET", "/tmp/other"),
        ("1INVALID", "invalid"),
        ("NULL_BYTE", "a\0b"),
    ] {
        let mut changed = request();
        changed.environment.insert(name.into(), value.into());
        assert!(changed.validate().is_err(), "{name}");
    }
    let mut changed = request();
    for index in 0..8 {
        changed
            .environment
            .insert(format!("LARGE_{index}"), "x".repeat(16 * 1024));
    }
    assert!(changed.validate().is_err());
    for value in ["", "../pod", "other:pod", "-pod", "pod-"] {
        let mut changed = request();
        changed.pod_uid = value.into();
        assert!(changed.validate().is_err());
    }
}
