//! What every hosted kernel is started with, and what it is refused.

use super::*;
use crate::{RuntimeEnvironment, RuntimeSpec, TermSize};

/// A provisioning request carrying this session's own environment.
fn spec_with(environment: RuntimeEnvironment) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        created_by: None,
        environment,
        session_id: "sesn_x".into(),
        size: TermSize {
            cols: 120,
            rows: 40,
        },
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
        provided_skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
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
