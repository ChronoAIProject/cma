//! What this kernel is configured with, and what must never appear in it.

use super::*;
use crate::{RuntimeOptions, TermSize};

/// A provisioning request with these agent choices.
fn spec_with(permission: RuntimePermission, effort: Option<&str>) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        skills: Vec::new(),
        reaches: Vec::new(),
        platform_briefing: String::new(),
        created_by: None,
        environment: Default::default(),
        session_id: "sesn_x".into(),
        generation: None,
        reservation_token: None,
        size: TermSize { cols: 80, rows: 24 },
        capacity: None,
        subject_credential: None,
        options: RuntimeOptions {
            permission,
            reasoning_effort: effort.map(str::to_string),
            ..RuntimeOptions::default()
        },
    }
}

fn rendered(permission: RuntimePermission) -> String {
    render(
        &spec_with(permission, Some("high")),
        "http://127.0.0.1:9091",
        "a-model",
        "responses",
    )
}

#[test]
fn no_credential_appears_in_what_the_agent_can_read() {
    // The reason the proxy exists at all. A terminal attachment is equivalent
    // to shell access, so anything in this file is readable by whoever is
    // typing — and the file names an environment variable rather than carrying
    // a value even for the placeholder.
    let config = rendered(RuntimePermission::Autonomous);
    assert!(config.contains("env_key = \"CHRONO_UPSTREAM_PLACEHOLDER_KEY\""));
    assert!(
        !config.contains("Bearer") && !config.contains("local-proxy-no-credential-needed"),
        "not even the inert one is written into the file, got {config}"
    );
}

#[test]
fn the_agent_is_pointed_at_the_loopback_proxy() {
    let config = rendered(RuntimePermission::Guarded);
    assert!(config.contains("base_url = \"http://127.0.0.1:9091\""));
}

#[test]
fn a_trailing_slash_on_the_proxy_url_does_not_reach_the_file() {
    // The upstream is a brokered service that appends whatever path arrives,
    // and a doubled separator produces a 404 mid-turn rather than at startup.
    let config = render(
        &spec_with(RuntimePermission::Autonomous, None),
        "http://127.0.0.1:9091/v1/",
        "a-model",
        "responses",
    );
    assert!(
        config.contains("base_url = \"http://127.0.0.1:9091/v1\""),
        "got {config}"
    );
}

#[test]
fn each_level_of_authority_becomes_this_kernels_own_pair() {
    // Two settings rather than one, and both must move together: an approval
    // policy without a matching sandbox mode is an agent that asks permission
    // for things it could not have done anyway, or does not ask for things it
    // can.
    for (permission, approval, sandbox) in [
        (RuntimePermission::Guarded, "on-request", "read-only"),
        (
            RuntimePermission::Autonomous,
            "on-failure",
            "workspace-write",
        ),
        (
            RuntimePermission::Unrestricted,
            "never",
            "danger-full-access",
        ),
    ] {
        let config = rendered(permission);
        assert!(
            config.contains(&format!("approval_policy = \"{approval}\"")),
            "{permission:?} got {config}"
        );
        assert!(
            config.contains(&format!("sandbox_mode = \"{sandbox}\"")),
            "{permission:?} got {config}"
        );
    }
}

#[test]
fn a_session_that_asked_for_no_effort_gets_the_cautious_default() {
    // Matching the core's own. A runtime started without this must not think
    // harder — or less — than one configured explicitly.
    let config = render(
        &spec_with(RuntimePermission::Guarded, None),
        "http://127.0.0.1:9091",
        "a-model",
        "responses",
    );
    assert!(
        config.contains("model_reasoning_effort = \"medium\""),
        "got {config}"
    );
}

#[test]
fn the_variable_the_file_names_is_the_one_the_environment_sets() {
    // Two places that must agree, so they are one value. If they diverge the
    // agent reads an unset variable and refuses to start.
    let (name, value) = placeholder();
    assert!(rendered(RuntimePermission::Guarded).contains(&format!("env_key = \"{name}\"")));
    assert!(!value.is_empty(), "a kernel refuses to start without one");
}

/// A request that spends its owner's own account with the model vendor.
fn own_account_spec() -> RuntimeSpec {
    let mut spec = spec_with(RuntimePermission::Autonomous, Some("high"));
    spec.options.own_credential = true;
    spec
}

#[test]
fn an_own_account_session_is_pointed_at_no_proxy_of_ours() {
    // The property the whole feature rests on. With our provider named here
    // the agent would sign in to the owner's account and then never spend it:
    // the session would run on the platform's credential while its owner
    // believed otherwise, which is the exact substitution `CredentialMode::Own`
    // exists to prevent.
    let config = render(
        &own_account_spec(),
        "http://127.0.0.1:9091",
        "a-model",
        "responses",
    );
    assert!(
        !config.contains("127.0.0.1:9091"),
        "an own-account session must not be pointed at our proxy, got {config}"
    );
    assert!(
        !config.contains("model_provider"),
        "it selects the kernel's own provider by naming none, got {config}"
    );
    assert!(
        !config.contains("env_key"),
        "and reads no credential from the environment, got {config}"
    );
}

#[test]
fn an_own_account_session_still_says_what_the_agent_may_do() {
    // Whose account pays says nothing about what the agent may do unattended.
    // Losing these on the own-account path would silently hand somebody a more
    // permissive session than the one they configured.
    let config = render(
        &own_account_spec(),
        "http://127.0.0.1:9091",
        "a-model",
        "responses",
    );
    assert!(config.contains("approval_policy = \"on-failure\""));
    assert!(config.contains("sandbox_mode = \"workspace-write\""));
    assert!(config.contains("model_reasoning_effort = \"high\""));
}

#[test]
fn no_credential_appears_in_an_own_account_config_either() {
    // A terminal attachment is shell access, so anything in this file is
    // readable by whoever is typing. The owner's own credential is written by
    // the agent itself, inside the sandbox, and must not be here.
    let config = render(
        &own_account_spec(),
        "http://127.0.0.1:9091",
        "a-model",
        "responses",
    );
    for forbidden in ["Bearer", "sk-", "auth.json", "api_key", "OPENAI_API_KEY"] {
        assert!(
            !config.contains(forbidden),
            "{forbidden} must not appear: {config}"
        );
    }
}
