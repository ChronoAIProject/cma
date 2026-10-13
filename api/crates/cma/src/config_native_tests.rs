use super::*;

#[test]
fn native_namespace_and_connection_do_not_replace_legacy_configuration() {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let config = configured(&[
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_NAMESPACE", "remote-cma"),
        (
            "CHRONO_AGENT_SANDBOX_API_SERVER",
            "https://remote-api.example",
        ),
        ("CHRONO_AGENT_SANDBOX_CA_FILE", "/var/run/native/ca.crt"),
        ("CHRONO_AGENT_SANDBOX_TOKEN_FILE", "/var/run/native/token"),
    ])
    .unwrap();
    assert_eq!(config.runtime.sandbox_namespace, "chrono-bot");
    assert_eq!(
        config.runtime.agent_sandbox.as_ref().unwrap().namespace,
        "remote-cma"
    );
    assert_eq!(
        config
            .runtime
            .agent_sandbox_connection
            .as_ref()
            .unwrap()
            .server,
        "https://remote-api.example"
    );
    assert!(config.runtime.retain_opensandbox);
    assert!(!config.runtime.allocate_agent_sandbox);
}

#[test]
fn native_configuration_preserves_local_defaults_and_rejects_partial_remote_credentials() {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let config = configured(&[
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
    ])
    .unwrap();
    assert_eq!(
        config.runtime.agent_sandbox.unwrap().namespace,
        "chrono-bot"
    );
    assert!(config.runtime.agent_sandbox_connection.is_none());
    let fields = [
        (
            "CHRONO_AGENT_SANDBOX_API_SERVER",
            "https://remote-api.example",
        ),
        ("CHRONO_AGENT_SANDBOX_CA_FILE", "/var/run/native/ca.crt"),
        ("CHRONO_AGENT_SANDBOX_TOKEN_FILE", "/var/run/native/token"),
    ];
    for bits in 1..7 {
        let mut subset: Vec<_> = fields
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, field)| *field)
            .collect();
        subset.extend([
            ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
            ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ]);
        assert!(configured(&subset)
            .unwrap_err()
            .contains("must be configured together"));
    }
    assert!(configured(&[
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_NAMESPACE", "bad/namespace"),
    ])
    .is_err());
}

#[test]
fn native_snapshot_override_keeps_legacy_seed_and_empty_vct_needs_storage_pair() {
    let legacy = [
        ("CHRONO_SANDBOX_HOME_STORAGE_CLASS", "local-disk"),
        ("CHRONO_SANDBOX_HOME_CAPACITY_GIB", "64"),
        ("CHRONO_SANDBOX_HOME_TEMPLATE", "local-seed"),
        ("CHRONO_SANDBOX_HOME_TEMPLATE_UID", "local-seed-uid"),
    ];
    let native = [
        ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "remote-disk"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "32"),
        ("CHRONO_AGENT_SANDBOX_HOME_TEMPLATE", "remote-seed"),
        ("CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID", "remote-seed-uid"),
    ];
    let defaults = configured(&legacy).unwrap();
    assert_eq!(
        defaults.runtime.home_policy,
        defaults.runtime.agent_sandbox_home_policy
    );
    let config = configured(&[legacy, native].concat()).unwrap();
    assert_eq!(
        config
            .runtime
            .home_policy
            .as_ref()
            .unwrap()
            .template_generation,
        "local-seed-uid"
    );
    let remote = config.runtime.agent_sandbox_home_policy.unwrap();
    assert_eq!(remote.template_generation, "remote-seed-uid");
    assert_eq!(remote.storage_class, "remote-disk");
    assert_eq!(remote.capacity_gib, 32);
    for bits in 1..15 {
        let mut values = legacy.to_vec();
        values.extend(
            native
                .iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, field)| *field),
        );
        if bits == 3 {
            let policy = configured(&values)
                .unwrap()
                .runtime
                .agent_sandbox_home_policy
                .unwrap();
            assert!(policy.is_native_vct());
            assert!(policy.template_name.is_empty());
        } else {
            assert!(configured(&values).is_err(), "subset {bits}");
        }
    }
    let mut empty = legacy.to_vec();
    empty.extend(native.iter().map(|(key, _)| (*key, "")));
    assert_eq!(
        configured(&empty)
            .unwrap()
            .runtime
            .agent_sandbox_home_policy,
        defaults.runtime.home_policy
    );
}

#[test]
fn different_native_home_policy_requires_independent_authority_when_enabled() {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let native = [
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "remote-disk"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "32"),
        ("CHRONO_AGENT_SANDBOX_HOME_TEMPLATE", "remote-seed"),
        ("CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID", "remote-seed-uid"),
    ];
    assert!(configured(&native)
        .unwrap_err()
        .contains("requires an independent"));

    let mut separate_namespace = native.to_vec();
    separate_namespace.push(("CHRONO_AGENT_SANDBOX_NAMESPACE", "native-cma"));
    assert!(configured(&separate_namespace).is_ok());

    let mut separate_connection = native.to_vec();
    separate_connection.extend([
        (
            "CHRONO_AGENT_SANDBOX_API_SERVER",
            "https://remote-api.example",
        ),
        ("CHRONO_AGENT_SANDBOX_CA_FILE", "/var/run/native/ca.crt"),
        ("CHRONO_AGENT_SANDBOX_TOKEN_FILE", "/var/run/native/token"),
    ]);
    assert!(configured(&separate_connection).is_ok());

    let mut same_policy = native.to_vec();
    same_policy.extend([
        ("CHRONO_SANDBOX_HOME_STORAGE_CLASS", "remote-disk"),
        ("CHRONO_SANDBOX_HOME_CAPACITY_GIB", "32"),
        ("CHRONO_SANDBOX_HOME_TEMPLATE", "remote-seed"),
        ("CHRONO_SANDBOX_HOME_TEMPLATE_UID", "remote-seed-uid"),
    ]);
    assert!(configured(&same_policy).is_ok());
}

#[test]
fn metadata_auth_is_explicit_and_cannot_mix_with_token_file_or_partial_target() {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let enabled = [
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
    ];
    let fields = [
        (
            "CHRONO_AGENT_SANDBOX_API_SERVER",
            "https://remote-api.example",
        ),
        ("CHRONO_AGENT_SANDBOX_CA_FILE", "/etc/singapore/ca.crt"),
        ("CHRONO_AGENT_SANDBOX_AUTH_MODE", "google-metadata"),
    ];
    let config = configured(&[enabled.as_slice(), fields.as_slice()].concat()).unwrap();
    assert_eq!(
        config.runtime.agent_sandbox_connection.unwrap().auth,
        layer3_adapter::agent_sandbox::RemoteKubernetesAuth::GoogleMetadata
    );
    for missing in 0..fields.len() {
        let mut subset = enabled.to_vec();
        subset.extend(
            fields
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != missing)
                .map(|(_, v)| *v),
        );
        assert!(configured(&subset).is_err());
    }
    let mut mixed = [enabled.as_slice(), fields.as_slice()].concat();
    mixed.push(("CHRONO_AGENT_SANDBOX_TOKEN_FILE", "/var/run/native/token"));
    assert!(configured(&mixed).unwrap_err().contains("must be unset"));
    assert!(configured(
        &[
            enabled.as_slice(),
            &[("CHRONO_AGENT_SANDBOX_AUTH_MODE", "auto")]
        ]
        .concat()
    )
    .is_err());
}

#[test]
fn disabled_native_provider_does_not_require_projected_remote_connection() {
    let projected = [
        ("CHRONO_AGENT_SANDBOX_AUTH_MODE", "google-metadata"),
        ("CHRONO_AGENT_SANDBOX_CA_FILE", "/etc/example-registry-01/ca.crt"),
    ];
    for bits in 0..4 {
        let partial: Vec<_> = projected
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, value)| *value)
            .collect();
        for explicit_disable in [false, true] {
            let mut values = partial.clone();
            if explicit_disable {
                values.push(("CHRONO_AGENT_SANDBOX_ENABLED", "false"));
            }
            let config = configured(&values).expect("disabled native target is optional");
            assert!(config.runtime.agent_sandbox.is_none());
            assert!(config.runtime.agent_sandbox_connection.is_none());
        }
    }
}

#[test]
fn native_empty_vct_mode_can_share_cluster_without_snapshot_and_rejects_mixed_mode() {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let enabled = [
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "native-disk"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "32"),
    ];
    let policy = configured(&enabled)
        .unwrap()
        .runtime
        .agent_sandbox_home_policy
        .unwrap();
    assert!(policy.is_native_vct());
    assert!(policy.template_name.is_empty() && policy.template_generation.is_empty());
    for (name, value) in [
        ("CHRONO_AGENT_SANDBOX_HOME_MODE", "unknown"),
        ("CHRONO_AGENT_SANDBOX_HOME_MODE", "legacy_snapshot"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "032"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "0"),
    ] {
        let mut values = enabled.to_vec();
        values.retain(|(key, _)| *key != name);
        values.push((name, value));
        assert!(configured(&values).is_err(), "{name}={value}");
    }
    let mixed = [
        enabled.as_slice(),
        &[
            ("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_empty_v1"),
            ("CHRONO_AGENT_SANDBOX_HOME_TEMPLATE", "forbidden"),
        ],
    ]
    .concat();
    assert!(configured(&mixed).is_err());
    let fallback = configured(&[
        ("CHRONO_SANDBOX_HOME_STORAGE_CLASS", "original-disk"),
        ("CHRONO_SANDBOX_HOME_CAPACITY_GIB", "64"),
        ("CHRONO_SANDBOX_HOME_TEMPLATE", "original-seed"),
        ("CHRONO_SANDBOX_HOME_TEMPLATE_UID", "original-uid"),
        ("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_empty_v1"),
    ])
    .unwrap();
    assert_eq!(
        fallback
            .runtime
            .agent_sandbox_home_policy
            .unwrap()
            .storage_class,
        "original-disk"
    );
    assert!(!fallback.runtime.home_policy.unwrap().is_native_vct());
}

#[test]
fn native_template_policy_is_explicit_bounded_and_does_not_block_disabled_provider() {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let enabled = [
        ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
    ];
    assert!(configured(&enabled)
        .unwrap()
        .runtime
        .agent_sandbox
        .unwrap()
        .template_network_policy
        .is_none());
    for value in ["", r#"{"ingress":[],"egress":[]}"#] {
        let mut values = enabled.to_vec();
        values.push(("CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY", value));
        let config = configured(&values).unwrap();
        assert_eq!(
            config
                .runtime
                .agent_sandbox
                .unwrap()
                .template_network_policy
                .is_some(),
            !value.is_empty()
        );
    }
    for value in [
        r#"{"ingress":[]}"#,
        r#"{"ingress":[],"egress":[],"management":"Unmanaged"}"#,
        "invalid",
    ] {
        let mut values = enabled.to_vec();
        values.push(("CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY", value));
        assert!(configured(&values).is_err());
    }
    assert!(configured(&[(
        "CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY",
        "invalid-while-disabled"
    )])
    .is_ok());
}
