//! What a sandbox for this kernel is started with.
//!
//! Split from the adapter's own tests so both stay within the project's
//! file-size limit, on the same division the code has: that file is about a
//! runtime's lifecycle, and this is about what one is given.

#![allow(clippy::items_after_test_module)]

use super::CodexSandboxSpec;
use crate::codex::CodexAdapter;
use crate::sandbox::SandboxClient;
use std::collections::BTreeMap;

use crate::{AdapterError, KernelAdapter, RuntimeSpec, TermSize};

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

fn adapter(base: &str) -> CodexAdapter {
    CodexAdapter::new(
        SandboxClient::new(base, "test-key"),
        CodexSandboxSpec::default(),
    )
}

#[tokio::test]
async fn permission_intent_reaches_this_kernel_through_the_file_it_reads() {
    // The core says how much rope to give the agent; this layer is the one
    // place that knows what this kernel calls those settings. A wrong mapping
    // silently grants more than the person asked for.
    //
    // It travels in the configuration this kernel reads rather than as
    // environment, because that is where this kernel looks — and the file is
    // composed here rather than in the supervisor, which hosts kernels that
    // read no file at all.
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
        let env = a.environment_for(&spec);
        let config = env
            .get(crate::layer4::CONFIG_BODY_VAR)
            .expect("this kernel is handed a configuration");
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
fn the_supervisor_is_told_where_this_kernels_file_goes() {
    // It writes what it is handed and reads none of it, which is what lets the
    // same pod image host a kernel that reads no file at all.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get(crate::layer4::CONFIG_PATH_VAR).map(String::as_str),
        Some(".codex/config.toml")
    );
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
fn this_kernel_advertises_only_what_was_proven_of_it() {
    // Every one of these was checked against the pinned kernel version before
    // it was claimed — transcripts on issue #76 — rather than taken from its
    // documentation. A claim the wire cannot back would be discovered by
    // whoever tried to use it, which is the failure capabilities exist to
    // prevent.
    let c = adapter("http://p").capabilities();
    assert!(c.terminal, "the pty path is unchanged");
    assert!(c.conversation, "the bridge speaks the contract");
    assert!(
        c.snapshot,
        "a rollout carried into a fresh home resumes by id"
    );
    assert!(c.steer, "input mid-turn continues the same turn");
    assert!(c.usage, "token counts arrive with every turn");
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
async fn this_kernel_applies_the_rule_about_what_cannot_be_delivered() {
    // The rule itself belongs to every hosted kernel and is covered where it
    // lives. What is checked here is that this adapter applies it at all —
    // before the provider is called, so an undeliverable request never starts
    // a session that quietly lacks what it was described as having.
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
async fn what_the_image_already_carries_provisions_as_before() {
    // The success path, and the one that matters most: every session carries
    // this deployment's baseline, so a blanket refusal would have refused all
    // of them.
    let a = CodexAdapter::new(
        SandboxClient::new("http://127.0.0.1:1", "test-key"),
        CodexSandboxSpec {
            deployment: crate::hosted::Deployment {
                provided_packages: vec!["nyxid".into()],
                provided_skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
                ..CodexSandboxSpec::default().deployment
            },
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

#[test]
fn a_conversation_session_runs_the_bridge_rather_than_the_terminal_application() {
    // The same kernel, driven through a program that speaks the contract
    // rather than painting a screen. Starting the wrong one would give a
    // conversation session a full-screen application on a pipe, which either
    // refuses to render or produces output nothing can read.
    let a = adapter("http://127.0.0.1:1");
    let mut spec = spec_with(Default::default());
    spec.options.mode = crate::RuntimeMode::Conversation;
    let env = a.environment_for(&spec);
    assert_eq!(
        env.get("CHRONO_RUNTIME_COMMAND").map(String::as_str),
        Some("codex-acp")
    );
    assert_eq!(
        env.get("CHRONO_RUNTIME_RESUME_ARGS").map(String::as_str),
        Some(""),
        "the bridge is told which conversation to continue over the protocol, not by a subcommand"
    );
}

#[test]
fn a_terminal_session_runs_exactly_what_it_always_did() {
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_COMMAND").map(String::as_str),
        Some("codex")
    );
    assert_eq!(
        env.get("CHRONO_RUNTIME_RESUME_ARGS").map(String::as_str),
        Some("resume")
    );
}

#[test]
fn either_plane_insists_a_carried_conversation_names_itself() {
    // Asked for "the latest" this kernel selects among conversations recorded
    // in the current working directory, and one carried from another pod was
    // recorded somewhere else — so it finds nothing, starts fresh, and reports
    // success. Silent loss, on both planes.
    let a = adapter("http://127.0.0.1:1");
    for mode in [
        crate::RuntimeMode::Terminal,
        crate::RuntimeMode::Conversation,
    ] {
        let mut spec = spec_with(Default::default());
        spec.options.mode = mode;
        assert_eq!(
            a.environment_for(&spec)
                .get("CHRONO_RUNTIME_SNAPSHOT_NAMED")
                .map(String::as_str),
            Some("true"),
            "{mode:?} must insist on being told which conversation to continue"
        );
    }
}
