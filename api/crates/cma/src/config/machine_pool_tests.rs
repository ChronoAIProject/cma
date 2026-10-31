use super::*;

#[test]
fn machine_pool_requires_its_own_image_and_native_home() {
    assert_eq!(
        configured(&[])
            .unwrap()
            .runtime
            .native_machine_warm_pool_target,
        0
    );
    assert!(
        configured(&[("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET", "1")])
            .unwrap_err()
            .contains("requires a machine image")
    );
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let image = format!("registry.example/desktop@sha256:{}", "a".repeat(64));
    let native = [
        ("CHRONO_SANDBOX_PROVIDER", "agent-sandbox"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "remote-disk"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "32"),
        ("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_full_home_v2"),
        ("CHRONO_MACHINE_SANDBOX_IMAGE", image.as_str()),
    ];
    assert_eq!(
        configured(&native)
            .unwrap()
            .runtime
            .native_machine_warm_pool_target,
        1
    );
    for value in ["0", "2", "1000"] {
        let mut fields = native.to_vec();
        fields.push(("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET", value));
        assert_eq!(
            configured(&fields)
                .unwrap()
                .runtime
                .native_machine_warm_pool_target
                .to_string(),
            value
        );
    }
    for value in ["", "-1", "1001", "true", "1.0"] {
        let mut fields = native.to_vec();
        fields.push(("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET", value));
        assert!(configured(&fields)
            .unwrap_err()
            .contains("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET"));
    }
    let mut fields = native.to_vec();
    fields.push(("CHRONO_SANDBOX_IMAGE", image.as_str()));
    assert!(configured(&fields).unwrap_err().contains("must differ"));
}

#[test]
fn prepared_machines_require_native_full_home_and_bounded_capacity() {
    assert_eq!(
        configured(&[])
            .unwrap()
            .runtime
            .native_machine_prepared_target,
        0
    );
    assert!(
        configured(&[("CHRONO_NATIVE_MACHINE_PREPARED_TARGET", "1")])
            .unwrap_err()
            .contains("requires a machine image")
    );
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let image = format!("registry.example/desktop@sha256:{}", "a".repeat(64));
    let native = [
        ("CHRONO_SANDBOX_PROVIDER", "agent-sandbox"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "remote-disk"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "32"),
        ("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_full_home_v2"),
        ("CHRONO_MACHINE_SANDBOX_IMAGE", image.as_str()),
    ];
    for value in ["0", "1", "2"] {
        let mut fields = native.to_vec();
        fields.push(("CHRONO_NATIVE_MACHINE_PREPARED_TARGET", value));
        assert_eq!(
            configured(&fields)
                .unwrap()
                .runtime
                .native_machine_prepared_target
                .to_string(),
            value
        );
    }
    for value in ["", "-1", "3", "true", "1.0"] {
        let mut fields = native.to_vec();
        fields.push(("CHRONO_NATIVE_MACHINE_PREPARED_TARGET", value));
        assert!(configured(&fields)
            .unwrap_err()
            .contains("CHRONO_NATIVE_MACHINE_PREPARED_TARGET"));
    }
    let mut fields = native.to_vec();
    fields.retain(|(name, _)| *name != "CHRONO_AGENT_SANDBOX_HOME_MODE");
    fields.push(("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_empty_v1"));
    fields.push(("CHRONO_NATIVE_MACHINE_PREPARED_TARGET", "1"));
    assert!(configured(&fields)
        .unwrap_err()
        .contains("full native home"));
}
