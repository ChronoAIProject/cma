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
