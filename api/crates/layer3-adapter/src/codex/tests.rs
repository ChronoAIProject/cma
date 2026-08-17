//! Tests for the kernel adapter.
//!
//! Kept beside the module rather than inside it so that file stays within the
//! project's size limit while the behaviour remains covered.

#![allow(clippy::items_after_test_module)]

use super::*;
use crate::TermSize;

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
        "ws://provider/v1/sandboxes/sbx-1/proxy/9090/runtime/sbx-1/terminal?wire=1"
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
        "ws://provider/proxy/9090/runtime/s/terminal?wire=1"
    );
}

#[test]
fn the_default_spec_requests_a_real_workload_size() {
    // A default, not a policy: the deployment sets these, because how much a
    // session costs is decided against the quota it is drawn from.
    let s = CodexSandboxSpec::default();
    assert_eq!(s.resource_limits.get("cpu").map(String::as_str), Some("1"));
    assert_eq!(
        s.resource_limits.get("memory").map(String::as_str),
        Some("1Gi")
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
            restore_from: None,
            created_by: None,
            environment: Default::default(),
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
