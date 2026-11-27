use super::*;

fn runtime_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("CHRONO_SANDBOX_ID".into(), "sandbox-test".into()),
        ("CHRONO_RUNTIME_HOME".into(), "/home/cma".into()),
        ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
    ])
}

#[test]
fn authenticated_standard_runtime_defaults_to_stale_turn_seals() {
    let flag = crate::bridge::STALE_TURN_SEAL_FLAG;
    for (authenticated, machine, override_value, expected) in [
        (true, false, None, Some("1")),
        (true, false, Some("0"), Some("0")),
        (true, true, None, None),
        (false, false, None, None),
    ] {
        let mut environment = runtime_environment();
        if machine {
            environment.insert("CMA_MACHINE_INCLUDED".into(), "1".into());
        }
        if let Some(value) = override_value {
            environment.insert(flag.into(), value.into());
        }
        let mut config = RuntimeConfig::from_values(environment).unwrap();
        config.configure_stale_turn_seal(authenticated);
        assert_eq!(config.process_env.get(flag).map(String::as_str), expected);
        assert!(!config.install_env.contains_key(flag));
    }
}

#[test]
fn assigned_tmpdir_controls_checkpoint_and_installation_storage() {
    let mut environment = runtime_environment();
    environment.insert("TMPDIR".into(), "/var/lib/cma-transfer".into());
    let config = RuntimeConfig::from_values(environment).unwrap();
    assert_eq!(
        config.checkpoint_root,
        Path::new("/var/lib/cma-transfer/cma-checkpoints")
    );
    assert_eq!(config.process_env["TMPDIR"], "/var/lib/cma-transfer");
    assert_eq!(config.install_env["TMPDIR"], "/var/lib/cma-transfer");
}

#[test]
fn machine_marker_is_explicit_and_fail_closed() {
    let mut environment = runtime_environment();
    assert!(
        !RuntimeConfig::from_values(environment.clone())
            .unwrap()
            .machine_included
    );
    environment.insert("CMA_MACHINE_INCLUDED".into(), "1".into());
    let machine = RuntimeConfig::from_values(environment.clone()).unwrap();
    assert!(machine.machine_included);
    assert_eq!(
        machine.process_env.get("DISPLAY").map(String::as_str),
        Some(":99")
    );
    environment.insert("CMA_MACHINE_INCLUDED".into(), "yes".into());
    assert!(RuntimeConfig::from_values(environment).is_err());
}

#[test]
fn absent_tmpdir_preserves_legacy_temporary_directory() {
    let mut environment = runtime_environment();
    for persistent in [true, false] {
        if !persistent {
            environment.remove("CHRONO_RUNTIME_HOME_IDENTITY");
        }
        let config = RuntimeConfig::from_values(environment.clone()).unwrap();
        assert_eq!(
            config.checkpoint_root,
            std::env::temp_dir().join("cma-checkpoints")
        );
    }
}

#[test]
fn nonpersistent_runtime_preserves_custom_home_local_tmpdir() {
    let mut environment = runtime_environment();
    environment.remove("CHRONO_RUNTIME_HOME_IDENTITY");
    environment.insert("TMPDIR".into(), "/home/cma/tmp".into());
    let config = RuntimeConfig::from_values(environment).unwrap();
    assert_eq!(
        config.checkpoint_root,
        Path::new("/home/cma/tmp/cma-checkpoints")
    );
    assert_eq!(config.install_env["TMPDIR"], "/home/cma/tmp");
}

#[test]
fn invalid_assigned_tmpdir_is_rejected_without_ambient_fallback() {
    for temporary in [
        "",
        "relative",
        "/",
        "/tmp/../transfer",
        "/tmp/./transfer",
        "/home/cma",
        "/home/cma/tmp",
    ] {
        let mut environment = runtime_environment();
        environment.insert("TMPDIR".into(), temporary.into());
        assert!(RuntimeConfig::from_values(environment)
            .unwrap_err()
            .contains("TMPDIR"));
    }
}

#[test]
fn assigned_storage_keeps_foreign_installation_journals_rejected() {
    let temporary = std::env::temp_dir().join(format!(
        "cma-assigned-install-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut environment = runtime_environment();
    environment.insert("TMPDIR".into(), temporary.to_string_lossy().into_owned());
    let config = RuntimeConfig::from_values(environment).unwrap();
    let foreign = RuntimeConfig {
        sandbox_id: "another-sandbox".into(),
        ..config.clone()
    };
    let request = layer3_adapter::InstallRequest::new("foreign-install", vec![]);
    let record = crate::install_state::InstallRecord::started(&foreign, &request);
    assert!(crate::install_state::write_initial(&foreign, &record).unwrap());
    let path = temporary.join("cma-checkpoints/runtime-state/install-v2.json");
    let original = std::fs::read(&path).unwrap();
    assert_eq!(
        crate::install_state::recover(&config).unwrap_err().code,
        "install_state_invalid"
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
    std::fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn reserved_variables_are_not_install_environment() {
    assert!(reserved("CHRONO_SANDBOX_REACHES"));
    assert!(reserved("NYXID_URL"));
    assert!(!reserved("RUST_LOG"));
}

#[test]
fn credential_proxies_require_nonzero_loopback_bindings() {
    for bind in ["0.0.0.0:9091", "192.0.2.1:9091", "127.0.0.1:0"] {
        let env = BTreeMap::from([
            ("UPSTREAM".into(), "https://example.test".into()),
            ("BIND".into(), bind.into()),
        ]);
        assert!(proxy_from_env(&env, "BIND", "127.0.0.1:9091", "UPSTREAM", "test").is_err());
    }

    let env = BTreeMap::from([
        ("UPSTREAM".into(), "https://example.test".into()),
        ("BIND".into(), "127.0.0.2:9091".into()),
    ]);
    assert_eq!(
        proxy_from_env(&env, "BIND", "127.0.0.1:9091", "UPSTREAM", "test")
            .unwrap()
            .unwrap()
            .bind,
        "127.0.0.2:9091".parse().unwrap()
    );
}
