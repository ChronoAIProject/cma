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
    /// False in the assessment-only release; GET never schedules recovery.
    pub admission_enabled: bool,
}

impl Sandboxes<'_> {
    /// Assess the exact retained runtime without recovery or lifecycle effects.
    pub async fn inspect_runtime_recovery(&self, id: &str) -> Result<RetainedRecoveryAssessment> {
        self.client
            .get(&["sandboxes", id, "runtime-recovery"], &[])
            .await
    }
}
