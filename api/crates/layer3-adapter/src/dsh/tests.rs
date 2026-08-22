//! What this kernel is, and what it refuses to pretend it can do.
//!
//! Every refusal here exists because the alternative is a session that accepts
//! an operation and silently does nothing — or, in one case, one that blocks
//! forever on a question nobody can see.

#![allow(clippy::items_after_test_module)]

use super::*;
use crate::{RuntimeEnvironment, RuntimeMode, RuntimeOptions, TermSize};

fn adapter(base: &str) -> DshAdapter {
    DshAdapter::new(
        SandboxClient::new(base, "test-key"),
        DshSandboxSpec::default(),
    )
}

/// A conversation session that this kernel would accept.
fn conversation() -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        skills: Vec::new(),
        reaches: Vec::new(),
        platform_briefing: String::new(),
        created_by: None,
        environment: RuntimeEnvironment::default(),
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        capacity: None,
        subject_credential: None,
        options: RuntimeOptions {
            mode: RuntimeMode::Conversation,
            permission: RuntimePermission::Autonomous,
            ..RuntimeOptions::default()
        },
    }
}

#[test]
fn the_kernel_is_named_for_diagnostics_only() {
    assert_eq!(adapter("http://p").kernel_name(), "dsh");
}

#[test]
fn this_kernel_advertises_only_what_was_proven_of_it() {
    // Each was checked by driving real turns through the profile this
    // deployment bakes. The absences are argued in the module header, and
    // every one of them was found by trying rather than by reading.
    let c = adapter("http://p").capabilities();
    assert!(c.conversation, "its wire streams a session log");
    assert!(c.usage, "an assembled message carries its own accounting");
    assert!(!c.terminal, "the harness ships no terminal surface");
    assert!(
        !c.interrupt,
        "its wire has no cancel, and a signal did not stop a turn"
    );
    assert!(
        !c.snapshot,
        "its only session call creates; nothing re-serves a carried log"
    );
    assert!(
        !c.steer,
        "a prompt sent mid-turn is queued for the next one"
    );
}

#[tokio::test]
async fn a_guarded_session_is_refused_rather_than_left_to_block() {
    // The sharpest refusal here. This agent stops and asks when its policy says
    // to, and the wire it is driven over has no way to carry the question — so
    // a guarded session would wait forever on something nobody can see or
    // answer. Told at creation, its owner can choose a kernel that can be
    // guarded.
    let mut spec = conversation();
    spec.options.permission = RuntimePermission::Guarded;
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("a session that cannot be guarded must not start guarded");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("guarded"),
        "the refusal must name what cannot be done, got {err}"
    );
}

#[tokio::test]
async fn an_autonomous_session_reaches_the_provider() {
    // The success path, stated so the refusal above cannot quietly widen.
    // Port 1 is reserved and never listening, so the dead provider is what
    // fails rather than the request.
    let err = adapter("http://127.0.0.1:1")
        .provision(&conversation())
        .await
        .expect_err("the dead provider is what fails, not the request");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn a_terminal_session_is_refused_rather_than_started_empty() {
    let mut spec = conversation();
    spec.options.mode = RuntimeMode::Terminal;
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("this harness ships no terminal");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

#[tokio::test]
async fn a_carried_conversation_is_refused_rather_than_silently_dropped() {
    // Reaching here means rotation asked despite the capability. Failing is
    // what keeps the session it came from running instead of replacing it with
    // one whose first turn ends in a persistence collision.
    let mut spec = conversation();
    spec.restore_from = Some(b"a conversation from another pod".to_vec());
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("a conversation that cannot be resumed must not look resumed");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(err.to_string().contains("resume"), "got {err}");
}

#[tokio::test]
async fn a_session_asking_for_its_owners_own_credential_is_refused() {
    let mut spec = conversation();
    spec.options.own_credential = true;
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("a session that cannot be what was asked for must not start");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

#[tokio::test]
async fn this_kernel_applies_the_rule_about_what_cannot_be_delivered() {
    let mut spec = conversation();
    spec.environment.skills = vec!["a-skill-this-agent-lacks".into()];
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("an undeliverable skill must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-skill-this-agent-lacks"),
        "got {err}"
    );
}

#[tokio::test]
async fn attaching_a_terminal_reports_the_absence_rather_than_a_failure() {
    let err = adapter("http://127.0.0.1:1")
        .attach_terminal(&RuntimeHandle("sbx-1".into()))
        .await
        .expect_err("this kernel has no terminal plane");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

#[tokio::test]
async fn a_snapshot_reports_the_absence_rather_than_an_empty_conversation() {
    // `None` would read as "this session has nothing to carry", which is what
    // rotation acts on. It has to read as "this kernel cannot say".
    let err = adapter("http://127.0.0.1:1")
        .snapshot(&RuntimeHandle("sbx-1".into()))
        .await
        .expect_err("an absence must not read as an empty conversation");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

#[tokio::test]
async fn a_conversation_against_a_dead_provider_reports_unavailable() {
    let err = adapter("http://127.0.0.1:1")
        .open_conversation(
            &RuntimeHandle("sbx-1".into()),
            &Default::default(),
            tokio::sync::mpsc::channel(1).0,
        )
        .await
        .expect_err("a dead provider has no conversation to open");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}
