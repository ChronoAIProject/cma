//! What every hosted kernel is started with, and what it is refused.

use super::*;
use crate::{RuntimeEnvironment, RuntimeSpec, TermSize};

/// A provisioning request carrying this session's own environment.
fn spec_with(environment: RuntimeEnvironment) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        workspace: None,
        skills: Vec::new(),
        reaches: Vec::new(),
        runtime_policy_required: false,
        platform_briefing: String::new(),
        created_by: None,
        environment,
        session_id: "sesn_x".into(),
        generation: None,
        reservation_token: None,
        size: TermSize {
            cols: 120,
            rows: 40,
        },
        capacity: None,
        timeout_seconds: None,
        subject_credential: None,
        options: Default::default(),
    }
}

/// A deployment with an upstream configured, as a real one has.
fn deployed() -> Deployment {
    Deployment {
        upstream_base_url: "https://models.example/v1".into(),
        upstream_model: "a-default-model".into(),
        upstream_wire_api: "responses".into(),
        platform_base_url: "https://platform.example".into(),
        ..Deployment::default()
    }
}

#[test]
fn no_credential_travels_as_environment() {
    // The reason the proxy exists: a terminal attachment is equivalent to
    // shell access, so anything reachable from the environment is reachable by
    // whoever is typing. This is the assertion that fails if somebody ever
    // decides passing the real one would be simpler.
    let env = deployed().base_environment(&spec_with(Default::default()));
    assert_eq!(
        env.get("NYXID_ACCESS_TOKEN").map(String::as_str),
        Some(PLACEHOLDER_CREDENTIAL)
    );
    assert!(
        env.values().all(|v| !v.contains("Bearer")),
        "no credential-shaped value may be handed to a sandbox"
    );
}

#[test]
fn trigger_sensitive_proxies_have_distinct_fixed_loopback_bindings() {
    let env = deployed().base_environment(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_UPSTREAM_BIND").map(String::as_str),
        Some("127.0.0.1:9091")
    );
    assert_eq!(
        env.get("CHRONO_PLATFORM_PROXY_BIND").map(String::as_str),
        Some("127.0.0.2:9092")
    );
    assert_eq!(
        env.get("NYXID_URL").map(String::as_str),
        Some("http://127.0.0.2:9092")
    );
}

#[test]
fn a_sessions_own_model_wins_over_the_deployments_default() {
    let mut spec = spec_with(Default::default());
    spec.options.model = Some("a-model-this-session-asked-for".into());
    let env = deployed().base_environment(&spec);
    assert_eq!(
        env.get("CHRONO_UPSTREAM_MODEL").map(String::as_str),
        Some("a-model-this-session-asked-for")
    );
}

#[test]
fn an_unconfigured_session_gets_the_deployments_model() {
    let env = deployed().base_environment(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_UPSTREAM_MODEL").map(String::as_str),
        Some("a-default-model")
    );
}

#[test]
fn the_sandbox_is_told_the_geometry_it_was_asked_for() {
    let env = deployed().base_environment(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_COLS").map(String::as_str),
        Some("120")
    );
    assert_eq!(
        env.get("CHRONO_RUNTIME_ROWS").map(String::as_str),
        Some("40")
    );
}

#[test]
fn software_the_image_does_not_carry_is_refused_by_name() {
    let err = deployed()
        .refuse_undeliverable(&RuntimeEnvironment {
            packages: vec!["a-package-this-image-lacks".into()],
            ..Default::default()
        })
        .expect_err("an undeliverable requirement must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-package-this-image-lacks"),
        "the refusal must name what is missing, got {err}"
    );
}

#[test]
fn a_skill_the_agent_cannot_load_is_refused_by_name() {
    let err = deployed()
        .refuse_undeliverable(&RuntimeEnvironment {
            skills: vec!["a-skill-this-agent-lacks".into()],
            ..Default::default()
        })
        .expect_err("an undeliverable skill must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-skill-this-agent-lacks"),
        "{err}"
    );
}

#[test]
fn what_the_image_already_carries_is_honoured() {
    // The success path, and the one that matters most: every session carries
    // this deployment's baseline, so a blanket refusal would refuse all of them.
    let d = Deployment {
        provided_packages: vec!["nyxid".into()],
        skills_root: ".codex/skills".into(),
        ..deployed()
    };
    assert!(d
        .refuse_undeliverable(&RuntimeEnvironment {
            packages: vec!["nyxid".into()],
            skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
            ..Default::default()
        })
        .is_ok());
}

#[test]
fn a_session_asking_for_nothing_is_never_refused() {
    // A deployment that states no baseline still runs ordinary sessions.
    assert!(Deployment::default()
        .refuse_undeliverable(&RuntimeEnvironment::default())
        .is_ok());
}

#[test]
fn the_agent_is_told_what_its_session_may_reach() {
    // The gap this closes: a sandbox carried a credential broker and a CLI to
    // call it with, and nothing that said what either was for. An agent left
    // to infer its own scope reasons about why it cannot act, and produces a
    // confident wrong answer.
    let mut spec = spec_with(Default::default());
    spec.reaches = vec!["api-github".into(), "chrono-llm".into()];

    let env = deployed().base_environment(&spec);

    assert_eq!(
        env.get("CHRONO_SESSION_REACHES").map(String::as_str),
        Some("api-github,chrono-llm")
    );
}

#[test]
fn reaching_nothing_is_said_rather_than_left_unsaid() {
    // "This session reaches nothing" and "nothing told me" are different
    // facts, and an agent acts differently on them. The variable is always
    // present, so an absent one can only mean a control plane that predates
    // this — never a session that was granted none.
    let env = deployed().base_environment(&spec_with(Default::default()));

    assert_eq!(
        env.get("CHRONO_SESSION_REACHES").map(String::as_str),
        Some(""),
        "a session granted nothing must still be told so"
    );
}

#[test]
fn what_a_session_reaches_carries_names_and_never_a_credential() {
    // Names are a fact about a session's configuration; a token is not. The
    // whole reason the broker sits behind a loopback proxy is that anything in
    // this map is readable by whoever holds the terminal.
    let mut spec = spec_with(Default::default());
    spec.reaches = vec!["api-github".into()];
    spec.subject_credential = Some("a-real-credential-value".into());

    let env = deployed().base_environment(&spec);

    for (name, value) in &env {
        assert!(
            !value.contains("a-real-credential-value"),
            "{name} carried the owner's credential"
        );
    }
    assert_eq!(
        env.get("CHRONO_SESSION_REACHES").map(String::as_str),
        Some("api-github")
    );
}

#[test]
fn trigger_policy_environment_contains_only_identity_requirement_and_public_keys() {
    let mut spec = spec_with(Default::default());
    spec.session_id = "sesn_trigger".into();
    spec.runtime_policy_required = true;
    let deployment = Deployment {
        runtime_policy_verification_keys: r#"{"key-current":"public-key-material"}"#.into(),
        ..deployed()
    };

    let env = deployment.base_environment(&spec);

    assert_eq!(env["CHRONO_SESSION_ID"], "sesn_trigger");
    assert_eq!(env["CHRONO_GITHUB_CAPABILITY_REQUIRED"], "true");
    assert_eq!(
        env["CHRONO_GITHUB_CAPABILITY_VERIFY_KEYS"],
        r#"{"key-current":"public-key-material"}"#
    );
    assert!(env.keys().all(|name| !name.contains("SIGNING_KEY")));
}

#[test]
fn a_policy_required_session_is_refused_before_provision_without_a_verifier() {
    let mut spec = spec_with(Default::default());
    spec.runtime_policy_required = true;

    let error = deployed()
        .refuse_missing_policy_verifier(&spec)
        .expect_err("a Trigger runtime must fail closed without public verification keys");

    assert!(matches!(error, AdapterError::Unsupported(_)));
    assert!(Deployment::default()
        .refuse_missing_policy_verifier(&spec_with(Default::default()))
        .is_ok());
}
