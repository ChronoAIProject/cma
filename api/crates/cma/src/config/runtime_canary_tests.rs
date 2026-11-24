use serde_json::{json, Value};

use super::{configured, TemporaryEntry};
use crate::config::runtime::RuntimeConfig;

const CATALOG: &str = "CHRONO_RUNTIME_CANARY_RELEASES";

fn release(key: &str, lane: &str, name: &str, digit: &str, desktop: &str) -> Value {
    let source = "f".repeat(40);
    json!({
        "release_key": key,
        "lane": lane,
        "platform": "linux/amd64",
        "image": {
            "uri": format!("{name}@sha256:{}", digit.repeat(64)),
            "labels": {
                "org.opencontainers.image.revision": source,
                "dev.chrono.cma.runtime-contract": "3",
                "dev.chrono.cma.runtime-bootstrap": "1",
                "dev.chrono.cma.home-layout": "2",
                "dev.chrono.cma.runtime-desktop": desktop
            }
        },
        "capacity": {"cpu_millis": 1000, "memory_mib": 2048},
        "config_digest": format!("sha256:{}", "d".repeat(64)),
        "source_revision": source,
        "publication_run": 101,
        "publication_evidence_sha256": "e".repeat(64)
    })
}

fn agent() -> Value {
    release(
        "approved-agent",
        "agent",
        "registry.example/fixture/runtime",
        "a",
        "0",
    )
}

fn native_runtime(catalog: Option<&str>) -> Result<RuntimeConfig, String> {
    native_runtime_with(catalog, &[])
}

fn native_runtime_with(
    catalog: Option<&str>,
    overrides: &[(&str, &str)],
) -> Result<RuntimeConfig, String> {
    let key = TemporaryEntry::file(b"native-control-key-fixture-padding-0000");
    let machine = format!(
        "registry.example/fixture/global-desktop@sha256:{}",
        "1".repeat(64)
    );
    let mut env = vec![
        ("CHRONO_SANDBOX_PROVIDER", "agent-sandbox"),
        ("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", key.path_str()),
        ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "remote-disk"),
        ("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_full_home_v2"),
        ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "32"),
        ("CHRONO_MACHINE_SANDBOX_IMAGE", machine.as_str()),
        ("CHRONO_NATIVE_WARM_POOL_TARGET", "7"),
        ("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET", "4"),
        ("CHRONO_NATIVE_PREPARED_TARGET", "2"),
        ("CHRONO_NATIVE_MACHINE_PREPARED_TARGET", "1"),
    ];
    env.extend(catalog.map(|value| (CATALOG, value)));
    env.extend_from_slice(overrides);
    configured(&env).map(|config| config.runtime)
}

pub(crate) fn desktop_canary_config(cpu: u32, memory: u32, enabled: bool) -> RuntimeConfig {
    let mut candidate = release(
        "debian-candidate",
        "debian_desktop",
        "registry.example/fixture/debian",
        "b",
        "1",
    );
    candidate["capacity"] = json!({"cpu_millis": cpu, "memory_mib": memory});
    native_runtime_with(
        Some(&json!([candidate]).to_string()),
        &[
            (
                "CHRONO_RUNTIME_CANARY_WRITE_ENABLED",
                if enabled { "true" } else { "false" },
            ),
            ("CHRONO_RUNTIME_CANARY_OWNERS", "[\"owner\"]"),
        ],
    )
    .unwrap()
}

pub(crate) fn retained_runtime_config(image: &str, owner: &str) -> RuntimeConfig {
    let mut candidate = agent();
    candidate["image"]["uri"] = json!(image);
    let owners = json!([owner]).to_string();
    native_runtime_with(
        Some(&json!([candidate]).to_string()),
        &[
            ("CHRONO_RETAINED_RUNTIME_TRANSITIONS_ENABLED", "true"),
            ("CHRONO_RETAINED_RUNTIME_TRANSITION_OWNERS", &owners),
        ],
    )
    .unwrap()
}

#[test]
fn catalog_is_empty_by_default_and_does_not_select_releases_or_spares() {
    let absent = native_runtime(None).unwrap();
    assert_eq!(absent, native_runtime(Some("[]")).unwrap());
    assert!(format!("{absent:?}").contains("runtime_canary_release_count: 0"));

    let desktop = release(
        "approved-desktop",
        "debian_desktop",
        "registry.example:5000/team__fixture/debian--desktop",
        "b",
        "1",
    );
    let populated = native_runtime(Some(&json!([agent(), desktop]).to_string())).unwrap();
    assert_eq!(populated.sandbox_image, absent.sandbox_image);
    assert_eq!(
        populated.machine_sandbox_image,
        absent.machine_sandbox_image
    );
    assert_eq!(
        populated.native_warm_pool_target,
        absent.native_warm_pool_target
    );
    assert_eq!(
        populated.native_machine_warm_pool_target,
        absent.native_machine_warm_pool_target
    );
    assert_eq!(
        populated.native_prepared_target,
        absent.native_prepared_target
    );
    assert_eq!(
        populated.native_machine_prepared_target,
        absent.native_machine_prepared_target
    );
    let debug = format!("{populated:?}");
    assert!(debug.contains("runtime_canary_release_count: 2"));
    assert!(!debug.contains("approved-desktop"));
    assert!(!debug.contains("fixture/debian"));
}

#[test]
fn catalog_requires_native_full_home_and_bounded_input() {
    let entries = json!([agent()]).to_string();
    assert!(configured(&[(CATALOG, &entries)])
        .unwrap_err()
        .contains("native full-home"));
    assert!(native_runtime(Some(&" ".repeat(32 * 1024 + 1)))
        .unwrap_err()
        .contains("byte limit"));
    assert!(native_runtime(Some(&json!(vec![agent(); 9]).to_string()))
        .unwrap_err()
        .contains("too many releases"));
    for malformed in ["", "null", "{}", "[true]"] {
        assert!(native_runtime(Some(malformed))
            .unwrap_err()
            .contains(CATALOG));
    }
}

#[test]
fn catalog_rejects_duplicate_keys_and_underlying_image_digests() {
    for alias in [
        release(
            "approved-agent",
            "agent",
            "registry.example/fixture/other",
            "b",
            "0",
        ),
        release(
            "approved-other",
            "agent",
            "registry.example/fixture/other",
            "a",
            "0",
        ),
    ] {
        assert!(native_runtime(Some(&json!([agent(), alias]).to_string()))
            .unwrap_err()
            .contains("must be unique"));
    }
}

#[test]
fn catalog_rejects_tagged_or_ambiguous_image_references() {
    for name in [
        "registry.example/fixture/runtime:stable",
        "registry.example:/fixture/runtime",
        "registry.example:0/fixture/runtime",
        "registry.example:abcd/fixture/runtime",
        "fixture/runtime",
        "registry.example/../runtime",
        "registry.example/fixture/a..b",
        "registry.example/fixture/a___b",
        "registry.example/fixture/a._b",
        "registry.example/fixture/a-_b",
        "registry.example/fixture/a-",
    ] {
        let item = release("approved-agent", "agent", name, "a", "0");
        let error = native_runtime(Some(&json!([item]).to_string())).unwrap_err();
        assert!(error.contains("only a digest"), "{name}: {error}");
    }
}

#[test]
fn catalog_enforces_the_distribution_repository_path_length() {
    for (length, valid) in [(255, true), (256, false)] {
        let path = format!(
            "registry.example/fixture/{}",
            "a".repeat(length - "fixture/".len())
        );
        let item = release("approved-agent", "agent", &path, "a", "0");
        assert_eq!(
            native_runtime(Some(&json!([item]).to_string())).is_ok(),
            valid,
            "repository path of {length} bytes"
        );
    }
}

#[test]
fn catalog_rejects_invalid_keys_platform_and_publication_fields() {
    for (field, bad) in [
        ("release_key", json!("Not a release key")),
        ("platform", json!("linux/arm64")),
        ("source_revision", json!("f".repeat(39))),
        ("source_revision", json!("0".repeat(40))),
        ("config_digest", json!("sha256:invalid")),
        ("publication_run", json!(0)),
        ("publication_evidence_sha256", json!("missing")),
        ("unexpected", json!("unrecognized")),
    ] {
        let mut item = agent();
        item[field] = bad;
        let error = native_runtime(Some(&json!([item]).to_string())).unwrap_err();
        assert!(error.contains(CATALOG), "{field}: {error}");
        assert!(!error.contains("unrecognized"), "{error}");
    }
}

#[test]
fn catalog_checks_runtime_contract_bootstrap_home_and_desktop_lane() {
    for label in [
        "dev.chrono.cma.runtime-contract",
        "dev.chrono.cma.runtime-bootstrap",
        "dev.chrono.cma.home-layout",
        "dev.chrono.cma.runtime-desktop",
    ] {
        let mut item = agent();
        item["image"]["labels"]
            .as_object_mut()
            .unwrap()
            .remove(label);
        assert!(
            native_runtime(Some(&json!([item]).to_string()))
                .unwrap_err()
                .contains(CATALOG),
            "{label}"
        );
    }
    let mut wrong_lane = agent();
    wrong_lane["lane"] = json!("debian_desktop");
    assert!(native_runtime(Some(&json!([wrong_lane]).to_string()))
        .unwrap_err()
        .contains("canary lane"));
    let mut wrong_desktop = release(
        "approved-desktop",
        "debian_desktop",
        "registry.example/fixture/desktop",
        "b",
        "1",
    );
    wrong_desktop["image"]["labels"]["dev.chrono.cma.desktop-environment"] = json!("omarchy");
    assert!(native_runtime(Some(&json!([wrong_desktop]).to_string()))
        .unwrap_err()
        .contains("canary lane"));
}
