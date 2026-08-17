//! What this kernel's sandbox is started with.
//!
//! The profile baked into the image reads these names at boot, so a wrong one
//! is a session that runs on the vendor's defaults — a different endpoint, a
//! different model, and telemetry to somebody else.

use std::collections::BTreeMap;

use super::{DshSandboxSpec, SESSIONS_DIR};
use crate::dsh::DshAdapter;
use crate::hosted::PLACEHOLDER_CREDENTIAL;
use crate::sandbox::SandboxClient;
use crate::{
    RuntimeEnvironment, RuntimeMode, RuntimeOptions, RuntimePermission, RuntimeSpec, TermSize,
};

fn adapter(base: &str) -> DshAdapter {
    DshAdapter::new(
        SandboxClient::new(base, "test-key"),
        DshSandboxSpec::default(),
    )
}

/// A conversation session, which is the only kind this kernel serves.
fn spec_with(environment: RuntimeEnvironment) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        created_by: None,
        environment,
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        subject_credential: None,
        options: RuntimeOptions {
            mode: RuntimeMode::Conversation,
            ..RuntimeOptions::default()
        },
    }
}

#[test]
fn the_pod_boots_the_profile_this_image_bakes() {
    // Not the launcher's default, which is the vendor's: telemetry to their
    // endpoint, a route to their API, and a watcher on the directory the agent
    // is editing.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_COMMAND").map(String::as_str),
        Some("dsh --profile chrono")
    );
}

#[test]
fn the_agent_is_pointed_at_the_supervisors_proxy_and_not_at_the_model() {
    // The reason the proxy exists: the real credential never enters the
    // sandbox, so nothing reachable by whoever is driving the agent holds it.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_DSH_PROXY_URL").map(String::as_str),
        Some("http://127.0.0.1:9091")
    );
    assert_eq!(
        env.get("CHRONO_UPSTREAM_PLACEHOLDER_KEY")
            .map(String::as_str),
        Some(PLACEHOLDER_CREDENTIAL)
    );
}

#[test]
fn telemetry_is_turned_off_rather_than_left_to_a_default() {
    // A tenant's session activity is not a third party's to receive, and this
    // is the one setting where a silent regression sends it.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("DSH_TELEMETRY_MODE").map(String::as_str),
        Some("DISABLED")
    );
}

#[test]
fn each_level_of_authority_has_this_kernels_own_name_for_it() {
    let a = adapter("http://p");
    for (permission, expected) in [
        (RuntimePermission::Autonomous, "workspace-write"),
        (RuntimePermission::Unrestricted, "danger-full-access"),
    ] {
        let mut spec = spec_with(Default::default());
        spec.options.permission = permission;
        assert_eq!(
            a.environment_for(&spec)
                .get("DSH_PERMISSION_MODE")
                .map(String::as_str),
            Some(expected),
            "for {permission:?}"
        );
    }
}

#[test]
fn the_runtime_is_told_where_this_kernel_keeps_its_conversations() {
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_SNAPSHOT_DIR").map(String::as_str),
        Some(SESSIONS_DIR)
    );
}

#[test]
fn a_conversation_is_never_claimed_to_be_resumable_by_name() {
    // This kernel cannot re-serve a carried conversation at all, so asking the
    // pod to name one would be asking for something that cannot happen.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_ne!(
        env.get("CHRONO_RUNTIME_SNAPSHOT_NAMED").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        env.get("CHRONO_RUNTIME_RESUME_ARGS").map(String::as_str),
        Some("")
    );
}

#[test]
fn a_sessions_own_variables_reach_the_sandbox() {
    let env = adapter("http://p").environment_for(&spec_with(RuntimeEnvironment {
        variables: BTreeMap::from([("MY_TOKEN_NAME".to_string(), "a-value".to_string())]),
        ..Default::default()
    }));
    assert_eq!(
        env.get("MY_TOKEN_NAME").map(String::as_str),
        Some("a-value")
    );
}

#[test]
fn a_name_this_kernel_controls_cannot_be_taken_over() {
    // A caller who could overwrite one of these could redirect the agent's
    // traffic, or widen how far it may reach inside its sandbox.
    let env = adapter("http://p").environment_for(&spec_with(RuntimeEnvironment {
        variables: BTreeMap::from([
            (
                "CHRONO_DSH_PROXY_URL".to_string(),
                "http://somewhere-else.example".to_string(),
            ),
            (
                "DSH_PERMISSION_MODE".to_string(),
                "danger-full-access".to_string(),
            ),
        ]),
        ..Default::default()
    }));
    assert_eq!(
        env.get("CHRONO_DSH_PROXY_URL").map(String::as_str),
        Some("http://127.0.0.1:9091")
    );
    assert_eq!(
        env.get("DSH_PERMISSION_MODE").map(String::as_str),
        Some("workspace-write"),
        "a caller must not be able to widen what the agent may reach"
    );
}

#[test]
fn the_route_the_profile_declares_is_the_route_a_session_asks_for() {
    // Two constants in two files that must agree, so they are one value. If
    // they diverge every turn fails with "no adapter registered for provider".
    assert_eq!(DshSandboxSpec::default().provider, "chrono");
}
