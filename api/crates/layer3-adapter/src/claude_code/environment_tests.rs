//! What this kernel's sandbox is started with.
//!
//! A wrong rendering here grants more than the person asked for, or points the
//! agent's traffic somewhere it should not go, so each one is pinned.

use std::collections::BTreeMap;

use super::ClaudeCodeSandboxSpec;
use crate::claude_code::ClaudeCodeAdapter;
use crate::hosted::PLACEHOLDER_CREDENTIAL;
use crate::sandbox::SandboxClient;
use crate::{RuntimeEnvironment, RuntimeMode, RuntimeOptions, RuntimeSpec, TermSize};

fn adapter(base: &str) -> ClaudeCodeAdapter {
    ClaudeCodeAdapter::new(
        SandboxClient::new(base, "test-key"),
        ClaudeCodeSandboxSpec::default(),
    )
}

/// A conversation session, which is the only kind this kernel serves.
fn spec_with(environment: RuntimeEnvironment) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        skills: Vec::new(),
        reaches: Vec::new(),
        platform_briefing: String::new(),
        created_by: None,
        environment,
        session_id: "sesn_x".into(),
        generation: None,
        size: TermSize { cols: 80, rows: 24 },
        capacity: None,
        subject_credential: None,
        options: RuntimeOptions {
            mode: RuntimeMode::Conversation,
            ..RuntimeOptions::default()
        },
    }
}

#[test]
fn the_pod_runs_this_kernels_bridge() {
    // There is no second program to choose between: this kernel serves no
    // terminal sessions here, so the bridge is what a pod always starts.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_COMMAND").map(String::as_str),
        Some("claude-code-acp")
    );
    assert_eq!(
        env.get("CHRONO_RUNTIME_RESUME_ARGS").map(String::as_str),
        Some(""),
        "a bridge told by protocol which conversation to continue takes no verb"
    );
}

#[test]
fn the_agent_is_pointed_at_the_supervisors_proxy_and_not_at_the_model() {
    // The reason the proxy exists: the real credential never enters the
    // sandbox, so nothing reachable by whoever is driving the agent holds it.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some("http://127.0.0.1:9091")
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
        Some(PLACEHOLDER_CREDENTIAL)
    );
}

#[test]
fn a_session_model_overrides_the_deployment_default() {
    let mut spec = spec_with(Default::default());
    spec.options.model = Some("a-model-this-session-asked-for".into());
    let env = adapter("http://p").environment_for(&spec);
    assert_eq!(
        env.get("ANTHROPIC_MODEL").map(String::as_str),
        Some("a-model-this-session-asked-for")
    );
    assert_eq!(
        env.get("CHRONO_UPSTREAM_MODEL").map(String::as_str),
        Some("a-model-this-session-asked-for"),
        "the proxy and the agent must agree on which model is being called"
    );
}

#[test]
fn the_runtime_is_told_where_this_kernel_keeps_its_conversations() {
    // The supervisor hosts any kernel and must not know this. Naming it here,
    // in the directory that owns this kernel, is what lets the same pod image
    // supervise a different one.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_SNAPSHOT_DIR").map(String::as_str),
        Some(".claude/projects")
    );
}

#[test]
fn a_conversation_is_never_claimed_to_be_resumable_by_name() {
    // The kernel files a conversation under a directory named for the working
    // directory it was started in, and the supervisor lays a carried one down
    // under a name of its own. Telling the pod to resume by identifier would
    // have it find nothing, start fresh, and report success.
    let env = adapter("http://p").environment_for(&spec_with(Default::default()));
    assert_ne!(
        env.get("CHRONO_RUNTIME_SNAPSHOT_NAMED").map(String::as_str),
        Some("true"),
        "a resume this kernel cannot perform must not be asked for"
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
    // These names are how the agent finds its endpoint and what it presents to
    // it. A caller who could overwrite one could redirect the agent's traffic —
    // and the credential attached to it — somewhere of their choosing. The
    // refusal is logged by `apply_to`.
    let env = adapter("http://p").environment_for(&spec_with(RuntimeEnvironment {
        variables: BTreeMap::from([(
            "ANTHROPIC_BASE_URL".to_string(),
            "http://somewhere-else.example".to_string(),
        )]),
        ..Default::default()
    }));
    assert_eq!(
        env.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some("http://127.0.0.1:9091"),
        "a caller must not be able to redirect the agent's traffic"
    );
}

#[test]
fn no_credential_mode_is_exported_to_a_runtime_that_cannot_read_one() {
    // An unread variable reads as a setting that was honoured. This kernel
    // refuses the request outright instead, which is visible.
    let mut spec = spec_with(Default::default());
    spec.options.own_credential = true;
    let env = adapter("http://p").environment_for(&spec);
    assert!(!env.keys().any(|key| key.contains("OWN_CREDENTIAL")));
}
