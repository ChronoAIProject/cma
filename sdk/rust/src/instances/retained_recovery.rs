use super::Sandboxes;
use crate::Result;

string_enum! {
    /// Fixed read-only observations, never recovery permission.
    pub enum RetainedRecoveryDisposition {
        ActiveLifecycle => "active_lifecycle", RecordedBootPresent => "recorded_boot_present",
        ExactBootReplacement => "exact_boot_replacement", ExactSuspendedRuntime => "exact_suspended_runtime",
        EvidenceUnavailable => "evidence_unavailable", AuthorityChanged => "authority_changed",
        TerminalFailure => "terminal_failure", ChildActivity => "child_activity",
        UnsupportedImageAuthority => "unsupported_image_authority",
        ProviderEvidenceRefused => "provider_evidence_refused", IncompleteHistory => "incomplete_history"
    }
}

/// An owner-visible assessment; no proof token or native identity is exposed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RetainedRecoveryAssessment {
    /// The fixed classification of the latest complete observation.
    pub disposition: RetainedRecoveryDisposition,
    /// Explicit owner admission is enabled; GET never schedules recovery.
    pub admission_enabled: bool,
}

/// Explicit consent bound to the exact Failed-parent revision.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RecoverRetainedRequest {
    /// Revision the owner reviewed before confirming recovery.
    pub expected_revision: u64,
    /// Must be true; the server derives the exact retained image itself.
    pub confirm_same_image_recovery: bool,
}

/// Immutable admission receipt. Lifecycle progress remains on the same Sandbox.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RetainedReopenReceipt {
    /// Durable lifecycle authority accepted by the transaction.
    pub operation_id: String,
    /// Sandbox revision at admission.
    pub revision: u64,
    /// Server admission timestamp.
    pub accepted_at: String,
}

impl Sandboxes<'_> {
    /// Reopen original conversations after explicit owner consent and fresh proof.
    pub async fn recover_retained(
        &self,
        id: &str,
        body: &RecoverRetainedRequest,
        key: &crate::IdempotencyKey,
    ) -> Result<RetainedReopenReceipt> {
        self.client
            .command(
                reqwest::Method::POST,
                &["sandboxes", id, "runtime-recovery"],
                Some(body),
                key,
                reqwest::StatusCode::ACCEPTED,
            )
            .await
    }

    /// Assess the exact retained runtime without recovery or lifecycle effects.
    pub async fn inspect_runtime_recovery(&self, id: &str) -> Result<RetainedRecoveryAssessment> {
        self.client
            .get(&["sandboxes", id, "runtime-recovery"], &[])
            .await
    }
}
