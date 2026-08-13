//! Tests for the kernel adapter.
//!
//! Kept beside the module rather than inside it so that file stays within the
//! project's size limit while the behaviour remains covered.

#![allow(clippy::items_after_test_module)]

use super::*;

fn adapter(base: &str) -> CodexAdapter {
    CodexAdapter::new(
        SandboxClient::new(base, "test-key"),
        CodexSandboxSpec::default(),
    )
}

#[test]
fn the_kernel_is_named_for_diagnostics_only() {
    assert_eq!(adapter("http://p").kernel_name(), "codex");
}

#[test]
fn a_proxied_endpoint_becomes_a_terminal_url() {
    let a = adapter("http://provider:80");
    assert_eq!(
        a.terminal_url("provider/v1/sandboxes/sbx-1/proxy/9090", "sbx-1"),
        "ws://provider/v1/sandboxes/sbx-1/proxy/9090/runtime/sbx-1/terminal"
    );
}

#[test]
fn a_secure_provider_yields_a_secure_socket() {
    let a = adapter("https://provider");
    assert!(a
        .terminal_url("provider/v1/sandboxes/s/proxy/9090", "s")
        .starts_with("wss://"));
}

#[test]
fn an_endpoint_that_already_has_a_scheme_is_not_doubled() {
    let a = adapter("http://provider");
    assert_eq!(
        a.terminal_url("http://provider/proxy/9090/", "s"),
        "ws://provider/proxy/9090/runtime/s/terminal"
    );
}

#[test]
fn the_default_spec_requests_a_real_workload_size() {
    let s = CodexSandboxSpec::default();
    assert_eq!(s.resource_limits.get("cpu").map(String::as_str), Some("2"));
    assert_eq!(
        s.resource_limits.get("memory").map(String::as_str),
        Some("4Gi")
    );
}

#[test]
fn sandboxes_do_not_expire_on_a_timer() {
    // A session is someone's working terminal. Expiring it on a timer ends
    // live work mid-turn, and a timer cannot tell an idle session from one
    // whose owner is reading the screen. Only the owner ends a session.
    assert!(
        CodexSandboxSpec::default().timeout_seconds.is_none(),
        "a default lifetime would terminate active sessions"
    );
}

#[tokio::test]
async fn provisioning_against_a_dead_provider_reports_unavailable() {
    // Port 1 is reserved and never listening, so this exercises the failure
    // path deterministically without a fixture server.
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .provision(&RuntimeSpec {
            session_id: "sesn_x".into(),
            size: TermSize { cols: 80, rows: 24 },
            subject_credential: None,
            options: Default::default(),
        })
        .await
        .expect_err("must fail against a dead provider");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn attaching_against_a_dead_provider_reports_unavailable() {
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .attach_terminal(&RuntimeHandle("sbx-1".into()))
        .await
        .expect_err("must fail against a dead provider");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn terminating_an_unknown_sandbox_is_not_an_error() {
    // The provider answers 404 for a sandbox already gone, and tearing down
    // something that no longer exists is a success, not a failure.
    let a = adapter("http://127.0.0.1:1");
    // Against a dead provider this is unreachable rather than 404, which is
    // still an error; the 404 tolerance is covered in the client's tests.
    assert!(a.terminate(&RuntimeHandle("gone".into())).await.is_err());
}

#[tokio::test]
async fn permission_intent_is_rendered_into_this_kernels_own_names() {
    // The core says how much rope to give the agent; this layer is the one
    // place that knows what this kernel calls those settings. A wrong
    // mapping here silently grants more than the person asked for.
    use crate::{RuntimeOptions, RuntimePermission};

    let cases = [
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
    ];
    for (permission, approval, sandbox) in cases {
        let a = adapter("http://127.0.0.1:1");
        let spec = RuntimeSpec {
            session_id: "sesn_x".into(),
            size: TermSize { cols: 80, rows: 24 },
            subject_credential: None,
            options: RuntimeOptions {
                permission,
                ..RuntimeOptions::default()
            },
        };
        // Provisioning fails against a dead provider, which is fine: the
        // environment is built before the call, and that is what is under
        // test here.
        let _ = a.provision(&spec).await;
        let env = a.environment_for(&spec);
        assert_eq!(
            env.get("CHRONO_AGENT_APPROVAL").map(String::as_str),
            Some(approval)
        );
        assert_eq!(
            env.get("CHRONO_AGENT_SANDBOX").map(String::as_str),
            Some(sandbox)
        );
    }
}

#[tokio::test]
async fn a_session_model_overrides_the_deployment_default() {
    use crate::RuntimeOptions;
    let a = adapter("http://127.0.0.1:1");
    let spec = RuntimeSpec {
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        subject_credential: None,
        options: RuntimeOptions {
            model: Some("a-chosen-model".into()),
            ..RuntimeOptions::default()
        },
    };
    let env = a.environment_for(&spec);
    assert_eq!(
        env.get("CHRONO_UPSTREAM_MODEL").map(String::as_str),
        Some("a-chosen-model")
    );
}
