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
            restore_from: None,
            created_by: None,
            environment: Default::default(),
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
        restore_from: None,
        created_by: None,
        environment: Default::default(),
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

#[tokio::test]
async fn asking_for_the_owners_own_credential_is_refused_rather_than_ignored() {
    // The failure this exists to prevent is silent: nothing in the runtime
    // reads such a setting, so a session accepted with it would spend the
    // delegated credential while its owner believed they were spending their
    // own. Refusing before the provider is even called is what makes the
    // difference visible.
    use crate::RuntimeOptions;

    let a = adapter("http://127.0.0.1:1");
    let err = a
        .provision(&RuntimeSpec {
            restore_from: None,
            created_by: None,
            environment: Default::default(),
            session_id: "sesn_x".into(),
            size: TermSize { cols: 80, rows: 24 },
            subject_credential: None,
            options: RuntimeOptions {
                own_credential: true,
                ..RuntimeOptions::default()
            },
        })
        .await
        .expect_err("an unsupported credential mode must not provision");
    assert!(
        matches!(err, AdapterError::Unsupported(_)),
        "the refusal must be attributable to a missing ability, got {err:?}"
    );
    assert!(
        err.to_string().contains("own credential"),
        "the refusal must name what was refused, got {err}"
    );
}

#[tokio::test]
async fn the_delegated_credential_provisions_exactly_as_before() {
    // The success path, stated so the refusal above cannot quietly widen: the
    // default credential mode still reaches the provider, and only fails there.
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
        .expect_err("the dead provider is what fails, not the request");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[test]
fn no_credential_mode_is_exported_to_a_runtime_that_cannot_read_one() {
    // The variable existed and nothing in layer4-runtime ever read it, which
    // is what made the setting look honoured.
    use crate::RuntimeOptions;

    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&RuntimeSpec {
        restore_from: None,
        created_by: None,
        environment: Default::default(),
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        subject_credential: None,
        options: RuntimeOptions {
            own_credential: true,
            ..RuntimeOptions::default()
        },
    });
    assert!(
        !env.keys().any(|key| key.contains("OWN_CREDENTIAL")),
        "an unread variable reads as a setting that was honoured"
    );
}

#[test]
fn this_kernel_advertises_the_terminal_it_actually_drives() {
    // Honest for the pseudo-terminal path, which is all this adapter drives:
    // bytes for a screen, and a conversation the supervisor can capture. A
    // claim here that the wire cannot back would be discovered by whoever
    // tried to use it, which is the failure capabilities exist to prevent.
    let c = adapter("http://p").capabilities();
    assert!(c.terminal, "the pty path is what this adapter is");
    assert!(c.snapshot, "rotation depends on the supervisor's capture");
    assert!(
        !c.conversation && !c.steer && !c.usage,
        "nothing structured crosses this boundary yet"
    );
}

#[tokio::test]
async fn a_credential_reaches_a_runtime_over_the_link_it_already_has() {
    // The frame is unchanged, which is the point: a pod started before this
    // method existed still understands what arrives, so no live session has to
    // be replaced to keep its credential fresh.
    let a = adapter("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, mut rx) = mpsc::channel(4);
    a.links.lock().await.insert(handle.0.clone(), tx);

    a.refresh_credential(&handle, "a-fresh-credential")
        .await
        .expect("a live link carries a credential");

    match rx.recv().await.expect("the frame was sent") {
        TerminalInput::Credential(value) => assert_eq!(value, "a-fresh-credential"),
        other => panic!("expected a credential frame, got {other:?}"),
    }
}

#[tokio::test]
async fn a_credential_for_a_runtime_with_no_link_is_refused_rather_than_dropped() {
    // Silence here would leave a session that looks alive and refuses every
    // call the agent makes on its owner's behalf, with nothing said about why.
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .refresh_credential(&RuntimeHandle("never-attached".into()), "unused")
        .await
        .expect_err("there is nowhere for this to go");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn a_link_that_closed_is_forgotten_rather_than_retried_forever() {
    let a = adapter("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    a.links.lock().await.insert(handle.0.clone(), tx);

    assert!(a.refresh_credential(&handle, "unused").await.is_err());
    assert!(
        !a.links.lock().await.contains_key(&handle.0),
        "a dead link kept in the map would fail every renewal the same way forever"
    );
}

#[tokio::test]
async fn tearing_a_runtime_down_forgets_its_link() {
    // Otherwise this map grows with the remains of every session the process
    // has ever served.
    let a = adapter("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, _rx) = mpsc::channel(1);
    a.links.lock().await.insert(handle.0.clone(), tx);

    // Against a dead provider the delete fails; the link is dropped first, on
    // purpose, because the sandbox is going either way.
    let _ = a.terminate(&handle).await;
    assert!(!a.links.lock().await.contains_key(&handle.0));
}

#[tokio::test]
async fn a_kernel_with_no_conversation_says_so_rather_than_failing_obscurely() {
    // The default body from the trait. Terminal-mode codex holds no
    // conversation, and a caller has to be able to tell that from a
    // conversation that broke.
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .open_conversation(&RuntimeHandle("sbx-1".into()))
        .await
        .expect_err("this kernel has no conversation plane yet");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

/// A spec, with the session's own environment attached.
fn spec_with(environment: crate::RuntimeEnvironment) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        created_by: None,
        environment,
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        subject_credential: None,
        options: Default::default(),
    }
}

#[test]
fn a_sessions_own_variables_reach_the_sandbox() {
    // The defect this closes: a tenant set variables on a profile, the REST
    // surface accepted them, and nothing ever reached the pod.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(crate::RuntimeEnvironment {
        variables: BTreeMap::from([("MY_TOKEN_NAME".to_string(), "a-value".to_string())]),
        ..Default::default()
    }));
    assert_eq!(
        env.get("MY_TOKEN_NAME").map(String::as_str),
        Some("a-value")
    );
}

#[test]
fn a_name_the_runtime_controls_cannot_be_taken_over() {
    // These names are how the runtime finds its upstream, its proxy and its
    // credential. A caller who could overwrite one could redirect the agent's
    // traffic — and the credential attached to it — somewhere of their
    // choosing. The refusal is logged by `apply_to`.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(crate::RuntimeEnvironment {
        variables: BTreeMap::from([(
            "CHRONO_UPSTREAM_BASE_URL".to_string(),
            "http://somewhere-else.example".to_string(),
        )]),
        ..Default::default()
    }));
    assert_ne!(
        env.get("CHRONO_UPSTREAM_BASE_URL").map(String::as_str),
        Some("http://somewhere-else.example"),
        "a caller must not be able to redirect the agent's traffic"
    );
}

#[tokio::test]
async fn software_the_image_does_not_carry_is_refused_by_name() {
    // Nothing can be installed into a running sandbox, so accepting this would
    // start a session that quietly lacks the tools it was described as having.
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .provision(&spec_with(crate::RuntimeEnvironment {
            packages: vec!["a-package-this-image-lacks".into()],
            ..Default::default()
        }))
        .await
        .expect_err("an undeliverable requirement must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-package-this-image-lacks"),
        "the refusal must name what is missing, got {err}"
    );
}

#[tokio::test]
async fn a_skill_the_agent_cannot_load_is_refused_by_name() {
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .provision(&spec_with(crate::RuntimeEnvironment {
            skills: vec!["a-skill-this-agent-lacks".into()],
            ..Default::default()
        }))
        .await
        .expect_err("an undeliverable skill must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-skill-this-agent-lacks"),
        "{err}"
    );
}

#[tokio::test]
async fn what_the_image_already_carries_provisions_as_before() {
    // The success path, and the one that matters most: every session carries
    // this deployment's baseline, so a blanket refusal would have refused all
    // of them.
    let a = CodexAdapter::new(
        SandboxClient::new("http://127.0.0.1:1", "test-key"),
        CodexSandboxSpec {
            provided_packages: vec!["nyxid".into()],
            provided_skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
            ..CodexSandboxSpec::default()
        },
    );
    let err = a
        .provision(&spec_with(crate::RuntimeEnvironment {
            packages: vec!["nyxid".into()],
            skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
            ..Default::default()
        }))
        .await
        .expect_err("the dead provider is what fails, not the request");
    assert!(
        matches!(err, AdapterError::Unavailable(_)),
        "the baseline must reach the provider, got {err:?}"
    );
}

#[test]
fn the_runtime_is_told_where_this_kernel_keeps_its_conversation() {
    // The supervisor hosts any kernel and must not know this. Naming it here,
    // in the directory that owns this kernel, is what lets the same pod image
    // supervise a different one.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_SNAPSHOT_DIR").map(String::as_str),
        Some(".codex/sessions")
    );
}
