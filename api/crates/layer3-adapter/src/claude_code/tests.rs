//! What this kernel is, and what it refuses to pretend it can do.
//!
//! The refusals carry as much weight as the abilities. Each one exists because
//! the alternative is a session that accepts an operation and silently does
//! nothing with it — the failure the capability model exists to prevent.

#![allow(clippy::items_after_test_module)]

use super::*;
use crate::{RuntimeEnvironment, RuntimeMode, RuntimeOptions, TermSize};

fn adapter(base: &str) -> ClaudeCodeAdapter {
    ClaudeCodeAdapter::new(
        SandboxClient::new(base, "test-key"),
        ClaudeCodeSandboxSpec::default(),
    )
}

/// A conversation session, which is the only kind this kernel serves.
fn conversation() -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        workspace: None,
        skills: Vec::new(),
        reaches: Vec::new(),
        runtime_policy_required: false,
        sponsored_model_required: false,
        workspace_source_required: false,
        conversation_slots_required: None,
        platform_briefing: String::new(),
        created_by: None,
        environment: RuntimeEnvironment::default(),
        session_id: "sesn_x".into(),
        chat_host: None,
        generation: None,
        reservation_token: None,
        size: TermSize { cols: 80, rows: 24 },
        capacity: None,
        timeout_seconds: None,
        subject_credential: None,
        options: RuntimeOptions {
            mode: RuntimeMode::Conversation,
            ..RuntimeOptions::default()
        },
    }
}

#[test]
fn the_kernel_is_named_for_diagnostics_only() {
    assert_eq!(adapter("http://p").kernel_name(), "claude_code");
}

#[test]
fn this_kernel_binds_its_production_conversation_to_shared_acp_evidence() {
    let options = RuntimeOptions {
        working_directory: Some("/workspace/worktrees/chat-2".into()),
        conversation_slot: Some("sesn_chat_2".into()),
        ..Default::default()
    };
    let binding = adapter("http://p").conversation_binding(&options);
    assert_eq!(
        binding.evidence_contract(),
        crate::hosted::ConversationEvidenceContract::AcpV1
    );
    assert_eq!(
        binding,
        crate::hosted::AcpConversationBinding::new(
            Some("default".into()),
            None,
            false,
            options.working_directory,
            options.conversation_slot,
        )
    );
}

#[test]
fn this_kernel_advertises_only_what_was_proven_of_it() {
    // The stub this replaced advertised nothing, which was honest. What
    // replaces it must be honest in the other direction too: each absence
    // below is argued in the module header, and claiming any of them would
    // move the discovery from the moment of asking to the moment of use.
    let c = adapter("http://p").capabilities();
    assert!(c.conversation, "the bridge speaks the contract");
    assert!(!c.terminal, "the terminal plane is a change of its own");
    assert!(
        !c.snapshot,
        "a carried conversation lands where this kernel does not look"
    );
    assert!(!c.steer, "the bridge accepts nothing mid-turn");
    assert!(!c.usage, "the bridge reports no token counts");
    assert!(c.resources, "the hosted runtime reports its resource use");
}

#[tokio::test]
async fn a_terminal_session_is_refused_rather_than_started_empty() {
    // A sandbox brought up for a terminal session would be a pod nobody can
    // attach to — billed, running, and useless.
    let mut spec = conversation();
    spec.options.mode = RuntimeMode::Terminal;
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("this kernel serves no terminal sessions");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

#[tokio::test]
async fn a_carried_conversation_is_refused_rather_than_silently_dropped() {
    // Reaching here means rotation asked despite the capability. Failing is
    // what keeps the session it came from running instead of replacing it with
    // an empty one that reports success.
    let mut spec = conversation();
    spec.restore_from = Some(b"a conversation from another pod".to_vec());
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("a conversation that cannot be resumed must not look resumed");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("resume"),
        "the refusal must say what could not be done, got {err}"
    );
}

#[tokio::test]
async fn a_session_asking_for_its_owners_own_credential_is_refused() {
    // Nothing authenticates the agent against the owner's vendor account, so a
    // session that asked and was accepted would run on the delegated
    // credential — the owner's bill and entitlements — believing otherwise.
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
    // The rule itself belongs to every hosted kernel and is covered where it
    // lives. What is checked here is that this adapter applies it at all.
    let mut spec = conversation();
    spec.environment.packages = vec!["a-package-this-image-lacks".into()];
    let err = adapter("http://127.0.0.1:1")
        .provision(&spec)
        .await
        .expect_err("an undeliverable requirement must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-package-this-image-lacks"),
        "the refusal must name what is missing, got {err}"
    );
}

#[tokio::test]
async fn an_ordinary_conversation_session_reaches_the_provider() {
    // The success path, stated so the refusals above cannot quietly widen: an
    // ordinary request gets past every check and fails only at the dead
    // provider. Port 1 is reserved and never listening.
    let err = adapter("http://127.0.0.1:1")
        .provision(&conversation())
        .await
        .expect_err("the dead provider is what fails, not the request");
    assert!(
        matches!(err, AdapterError::Unavailable(_)),
        "an ordinary session must reach the provider, got {err:?}"
    );
}

#[tokio::test]
async fn attaching_a_terminal_reports_the_absence_rather_than_a_failure() {
    // The contract's default, and the honest answer: a caller has to be able
    // to tell "this kernel never could" from "it could not this time".
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
    // This kernel does hold a conversation, so a caller has to be able to tell
    // "could not reach it" from "never could".
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
