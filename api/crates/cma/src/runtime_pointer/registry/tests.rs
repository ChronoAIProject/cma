use super::*;

fn config(
    flag: &str,
    token: Option<&str>,
    tag: &str,
    machine: bool,
) -> Result<Option<RuntimePointerConfig>, String> {
    let agent_ref = format!("{HOST}/{AGENT}:{tag}");
    let machine_ref = format!("{HOST}/{MACHINE}:{tag}");
    RuntimePointerConfig::from_lookup(
        &mut |key| {
            Ok(match key {
                "CHRONO_RUNTIME_POINTER_ENABLED" => Some(flag),
                "CHRONO_RUNTIME_REGISTRY_TOKEN" => token,
                "CHRONO_RUNTIME_POINTER" => Some(agent_ref.as_str()),
                "CHRONO_MACHINE_RUNTIME_POINTER" => machine.then_some(machine_ref.as_str()),
                "CHRONO_RUNTIME_POINTER_CUTOVER_EPOCH" => None,
                "CHRONO_ENVIRONMENT" => Some("staging"),
                _ => panic!("unexpected configuration key {key}"),
            }
            .map(str::to_owned))
        },
        machine,
    )
}

#[test]
fn configured_writers_can_bootstrap_while_following_is_frozen() {
    let mut queried = Vec::new();
    let absent = RuntimePointerConfig::from_lookup(
        &mut |key| {
            queried.push(key.to_owned());
            Ok(None)
        },
        false,
    )
    .unwrap();
    assert!(absent.is_none());
    assert!(!queried
        .iter()
        .any(|key| key == "CHRONO_RUNTIME_REGISTRY_TOKEN"));
    assert!(!config("false", Some("synthetic"), "staging-current", true)
        .unwrap()
        .unwrap()
        .can_follow());
    assert!(!config("true", None, "staging-current", true)
        .unwrap()
        .unwrap()
        .can_follow());
    assert!(config("true", Some("synthetic"), "staging-current", true)
        .unwrap()
        .unwrap()
        .can_follow());
}

#[test]
fn configuration_refuses_noncanonical_environment_and_switch() {
    assert!(config("true", Some("synthetic"), "production-current", false).is_err());
    assert!(config("1", Some("synthetic"), "staging-current", false).is_err());
    assert!(config("true", Some("invalid\ttoken"), "staging-current", false).is_err());
}

#[test]
fn first_publication_epoch_is_optional_but_must_be_canonical() {
    for (value, valid) in [
        (None, true),
        (Some("00000000-0000-0000-0000-000000000001"), true),
        (Some("00000000-0000-0000-0000-000000000000"), false),
        (Some("1"), false),
    ] {
        let config = RuntimePointerConfig::from_lookup(
            &mut |key| {
                Ok(match key {
                    "CHRONO_RUNTIME_POINTER" => Some(format!("{HOST}/{AGENT}:staging-current")),
                    "CHRONO_ENVIRONMENT" => Some("staging".into()),
                    "CHRONO_RUNTIME_POINTER_CUTOVER_EPOCH" => value.map(str::to_owned),
                    _ => None,
                })
            },
            false,
        );
        assert_eq!(config.is_ok(), valid);
        if let Ok(Some(config)) = config {
            assert_eq!(config.cutover_epoch().is_some(), value.is_some());
        }
    }
}

#[test]
fn immutable_descriptor_requires_a_canonical_sha256() {
    let digest = fingerprint(b"fixture");
    assert!(valid_digest(&digest));
    assert!(!valid_digest(&digest.to_uppercase()));
    assert!(!valid_digest("sha256:short"));
}
