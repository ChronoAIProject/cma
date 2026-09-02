//! What this kernel is, beyond the environment its sandbox is started with.
//!
//! Everything about running in a pod — the link, the wire, the credential, the
//! lifecycle — is covered where it lives, in `hosted`. How the core's intent
//! becomes this kernel's own vocabulary is covered next door. What is left is
//! the adapter itself.

#![allow(clippy::items_after_test_module)]

use super::*;
use crate::{KernelAdapter, RuntimeHandle};

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
fn this_kernel_binds_its_production_conversation_to_shared_acp_evidence() {
    let options = crate::RuntimeOptions {
        working_directory: Some("/workspace/worktrees/chat-1".into()),
        conversation_slot: Some("sesn_chat_1".into()),
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
            Some("read-only".into()),
            None,
            false,
            options.working_directory,
            options.conversation_slot,
        )
    );
}

#[tokio::test]
async fn a_conversation_against_a_dead_provider_reports_unavailable() {
    // A conversation that could not be opened is a failure, not an absent
    // ability: this kernel does hold one, and a caller has to be able to tell
    // "could not reach it" from "never could".
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .open_conversation(
            &RuntimeHandle("sbx-1".into()),
            &Default::default(),
            tokio::sync::mpsc::channel(1).0,
        )
        .await
        .expect_err("a dead provider has no conversation to open");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}
