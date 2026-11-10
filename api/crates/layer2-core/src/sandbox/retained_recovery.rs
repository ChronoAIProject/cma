//! Read-only assessment of an exact retained image; assessment never admits work.
use std::{collections::BTreeSet, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};

use super::{
    store::{SandboxStore, SandboxStoreError},
    Agent, AgentStatus, RuntimeBinding, Sandbox, SandboxStatus, SandboxWorkStep, Workspace,
};
use crate::{Caller, Capability, SandboxId};

/// Closed diagnostic vocabulary. No provider text or identity becomes a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedRecoveryDisposition {
    /// Durable lifecycle state only; this is not a native health claim.
    ActiveLifecycle,
    RecordedBootPresent,
    ExactBootReplacement,
    ExactSuspendedRuntime,
    EvidenceUnavailable,
    AuthorityChanged,
    TerminalFailure,
    ChildActivity,
    UnsupportedImageAuthority,
    ProviderEvidenceRefused,
    IncompleteHistory,
}

impl RetainedRecoveryDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveLifecycle => "active_lifecycle",
            Self::RecordedBootPresent => "recorded_boot_present",
            Self::ExactBootReplacement => "exact_boot_replacement",
            Self::ExactSuspendedRuntime => "exact_suspended_runtime",
            Self::EvidenceUnavailable => "evidence_unavailable",
            Self::AuthorityChanged => "authority_changed",
            Self::TerminalFailure => "terminal_failure",
            Self::ChildActivity => "child_activity",
            Self::UnsupportedImageAuthority => "unsupported_image_authority",
            Self::ProviderEvidenceRefused => "provider_evidence_refused",
            Self::IncompleteHistory => "incomplete_history",
        }
    }
}

/// The store must read complete child sets and validate every complete Agent log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedRecoverySnapshot {
    pub sandbox: Sandbox,
    pub agents: Vec<Agent>,
    pub workspaces: Vec<Workspace>,
}

/// Derived exclusively from the retained record, never from a requested image.
pub struct RetainedRecoverySource<'a> {
    pub binding: &'a RuntimeBinding,
    pub generation: u64,
    pub expected_boot: &'a str,
    pub home_identity: String,
}

impl RetainedRecoverySnapshot {
    pub fn source(&self) -> Result<RetainedRecoverySource<'_>, RetainedRecoveryDisposition> {
        use RetainedRecoveryDisposition as D;
        let sandbox = &self.sandbox;
        sandbox.validate().map_err(|_| D::AuthorityChanged)?;
        if sandbox.status == SandboxStatus::Active {
            return Err(D::ActiveLifecycle);
        }
        if sandbox.status != SandboxStatus::Failed
            || !sandbox.failure.as_ref().is_some_and(|failure| {
                matches!(failure.class.as_str(), "runtime_lost" | "restore_failed")
            })
        {
            return Err(D::TerminalFailure);
        }
        if sandbox.work.lease.is_some()
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.lifecycle.runtime_provision.is_some()
            || sandbox.lifecycle.unpublished_recovery.is_some()
        {
            return Err(D::AuthorityChanged);
        }
        let binding = sandbox
            .runtime
            .as_ref()
            .ok_or(D::UnsupportedImageAuthority)?;
        // An immutable exact image and the original retained storage graph are
        // necessary even for inspection. Managed update selection stays separate.
        let (_, digest) = binding
            .runtime_version
            .rsplit_once("@sha256:")
            .ok_or(D::UnsupportedImageAuthority)?;
        if !hex(digest, 64)
            || sandbox
                .profile
                .content
                .runtime_image
                .as_ref()
                .is_some_and(|image| image != &binding.runtime_version)
            || sandbox.runtime_update_request().is_some_and(|request| {
                request.completed_at.is_none() && request.target_version != binding.runtime_version
            })
        {
            return Err(D::UnsupportedImageAuthority);
        }
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .filter(|home| !home.released && home.native_pool.is_some())
            .ok_or(D::UnsupportedImageAuthority)?;
        let attachment = home
            .attachment()
            .filter(|a| a.request.policy.is_native_vct())
            .ok_or(D::UnsupportedImageAuthority)?;
        let generation = binding
            .provision_generation
            .filter(|value| *value > 0)
            .ok_or(D::AuthorityChanged)?;
        let boot = binding
            .boot_id
            .as_deref()
            .filter(|boot| hex(boot, 32))
            .ok_or(D::AuthorityChanged)?;
        if generation != sandbox.lifecycle.runtime_generation_high_water
            || sandbox.credential.current.is_none()
        {
            return Err(D::AuthorityChanged);
        }
        self.validate_children(generation)?;
        Ok(RetainedRecoverySource {
            binding,
            generation,
            expected_boot: boot,
            home_identity: attachment.identity().map_err(|_| D::AuthorityChanged)?,
        })
    }

    fn validate_children(&self, generation: u64) -> Result<(), RetainedRecoveryDisposition> {
        use RetainedRecoveryDisposition as D;
        let mut ids = BTreeSet::new();
        for agent in &self.agents {
            agent
                .validate_conversation()
                .map_err(|_| D::IncompleteHistory)?;
            if !ids.insert(&agent.id)
                || agent.owner != self.sandbox.owner
                || agent.sandbox_id != self.sandbox.id
            {
                return Err(D::AuthorityChanged);
            }
            if agent.status == AgentStatus::Failed {
                return Err(D::TerminalFailure);
            }
            // Live-child settlement is a distinct admission. A read-only
            // assessment never retires a turn or clears dispatch authority.
            if agent.work.step.is_pending()
                || agent.work.lease.is_some()
                || !agent.work.cleanup_debt.is_empty()
                || agent.requested_stop_reason.is_some()
                || agent.conversation.active_turn.is_some()
                || !agent.conversation.queued_messages.is_empty()
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(|op| op.blocks_dispatch())
                || agent
                    .conversation
                    .first_message
                    .as_ref()
                    .is_some_and(|first| !first.is_terminal())
                || (agent.status.is_live()
                    && (agent.status != AgentStatus::Paused
                        || agent
                            .conversation
                            .conversation_id
                            .as_ref()
                            .is_none_or(|id| id.trim().is_empty())
                        || agent
                            .conversation
                            .runtime_generation
                            .is_none_or(|old| old > generation)))
            {
                return Err(D::ChildActivity);
            }
        }
        let mut ids = BTreeSet::new();
        for workspace in &self.workspaces {
            workspace
                .validate_for_admission(&self.sandbox, self.sandbox.status)
                .map_err(|_| D::AuthorityChanged)?;
            if !ids.insert(&workspace.id)
                || workspace.owner != self.sandbox.owner
                || workspace.sandbox_id != self.sandbox.id
            {
                return Err(D::AuthorityChanged);
            }
            if workspace.status == super::WorkspaceStatus::Failed {
                return Err(D::TerminalFailure);
            }
            if workspace.work.lease.is_some() || workspace.work.step.is_pending() {
                return Err(D::ChildActivity);
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait RetainedRecoveryInspector: Send + Sync {
    /// Read-only fresh native/home/history inspection. The result is a fact,
    /// not a reusable proof token, lifecycle receipt, or permission to suspend.
    async fn inspect(&self, snapshot: &RetainedRecoverySnapshot) -> RetainedRecoveryDisposition;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetainedRecoveryAssessment {
    pub disposition: RetainedRecoveryDisposition,
    pub admission_enabled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RetainedRecoveryError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("recovery evidence unavailable")]
    Unavailable,
}

pub struct RetainedRecoveryService {
    store: Arc<dyn SandboxStore>,
    inspector: Arc<dyn RetainedRecoveryInspector>,
    permits: tokio::sync::Semaphore,
}

struct AssessmentObservation {
    started: tokio::time::Instant,
    outcome: &'static str,
    completed: bool,
}

impl Drop for AssessmentObservation {
    fn drop(&mut self) {
        tracing::info!(parent: None, outcome = self.outcome,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            completed = self.completed, admission_enabled = false,
            "retained recovery assessment");
    }
}

impl RetainedRecoveryService {
    pub fn new(
        store: Arc<dyn SandboxStore>,
        inspector: Arc<dyn RetainedRecoveryInspector>,
    ) -> Self {
        Self {
            store,
            inspector,
            permits: tokio::sync::Semaphore::new(1),
        }
    }

    pub async fn assess(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<RetainedRecoveryAssessment, RetainedRecoveryError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(RetainedRecoveryError::Forbidden);
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| RetainedRecoveryError::Unavailable)?;
        let mut observation = AssessmentObservation {
            started: tokio::time::Instant::now(),
            outcome: "cancelled",
            completed: false,
        };
        let disposition = tokio::time::timeout(Duration::from_secs(20), async {
            // Bound the initial authorization read too. No child/log/native
            // evidence is loaded before ownership has been established.
            self.store
                .get_sandbox(id)
                .await
                .map_err(store_error)?
                .filter(|sandbox| sandbox.owner == caller.subject)
                .ok_or(RetainedRecoveryError::NotFound)?;
            let snapshot = self
                .store
                .retained_runtime_recovery_snapshot(id)
                .await
                .map_err(store_error)?;
            if snapshot.sandbox.owner != caller.subject {
                return Err(RetainedRecoveryError::NotFound);
            }
            let disposition = match snapshot.source() {
                Ok(_) => self.inspector.inspect(&snapshot).await,
                Err(reason) => reason,
            };
            let current = self
                .store
                .retained_runtime_recovery_snapshot(id)
                .await
                .map_err(store_error)?;
            if current.sandbox.owner != caller.subject {
                return Err(RetainedRecoveryError::NotFound);
            }
            Ok(if snapshot == current {
                disposition
            } else {
                RetainedRecoveryDisposition::AuthorityChanged
            })
        })
        .await
        .unwrap_or(Ok(RetainedRecoveryDisposition::EvidenceUnavailable));
        observation.completed = true;
        observation.outcome = match &disposition {
            Ok(disposition) => disposition.as_str(),
            Err(RetainedRecoveryError::Forbidden) => "forbidden",
            Err(RetainedRecoveryError::NotFound) => "not_found",
            Err(RetainedRecoveryError::Unavailable) => "evidence_unavailable",
        };
        Ok(RetainedRecoveryAssessment {
            disposition: disposition?,
            admission_enabled: false,
        })
    }
}

fn store_error(error: SandboxStoreError) -> RetainedRecoveryError {
    match error {
        SandboxStoreError::NotFound => RetainedRecoveryError::NotFound,
        _ => RetainedRecoveryError::Unavailable,
    }
}

fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
#[path = "retained_recovery_tests.rs"]
mod tests;
