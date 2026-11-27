//! Bounded operator decisions share ordinary inactivity evidence and fences.
use std::{sync::Arc, time::Duration};

use super::{store::*, InactivityPauseRefusal, OperatorError};
use crate::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, Caller,
    Capability, DetailKey, PlatformSettingsService, SandboxId,
};

pub const AUTO_PAUSE_BATCH_LIMIT: u16 = 25;
pub const AUTO_PAUSE_MAX_BATCH: u16 = 100;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AutoPauseBatch {
    pub candidates: u64,
    pub refusals: Vec<AutoPauseSandboxRefusal>,
    pub scheduled: u64,
    pub already_satisfied: u64,
    pub skipped: u64,
    pub failed: u64,
    /// Empty cursor retries the first candidate after an uncertain first result.
    pub next: Option<String>,
    pub policy_revision: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AutoPauseSandboxRefusal {
    pub sandbox_id: SandboxId,
    pub reason: InactivityPauseRefusal,
}

pub struct SandboxAutoPauseService {
    settings: Arc<PlatformSettingsService>,
    maintenance: Arc<dyn SandboxMaintenanceStore>,
    activity: Arc<ActivityService>,
}

impl SandboxAutoPauseService {
    pub fn new(
        settings: Arc<PlatformSettingsService>,
        maintenance: Arc<dyn SandboxMaintenanceStore>,
        activity: Arc<ActivityService>,
    ) -> Self {
        Self {
            settings,
            maintenance,
            activity,
        }
    }

    pub async fn batch(
        &self,
        caller: &Caller,
        page: PageRequest,
        pause_now: bool,
    ) -> Result<AutoPauseBatch, OperatorError> {
        caller
            .require(Capability::ConfigurePlatform)
            .map_err(|_| OperatorError::Forbidden)?;
        if page.limit == 0 || page.limit > AUTO_PAUSE_MAX_BATCH {
            return Err(OperatorError::InvalidRequest(
                "auto-pause batch limit must be 1 through 100".into(),
            ));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let policy = tokio::time::timeout_at(deadline, self.settings.current())
            .await
            .map_err(|_| unavailable_before_batch(pause_now, "policy_read_timeout"))?
            .map_err(|_| unavailable_before_batch(pause_now, "policy_read_error"))?;
        let mut batch = AutoPauseBatch {
            policy_revision: policy.revision,
            enabled: policy.settings.inactivity.auto_pause_enabled,
            ..Default::default()
        };
        // A preview describes the enabled policy. Disabled policy never becomes
        // an implicit hypothetical or a mutating override.
        if !batch.enabled {
            return Ok(batch);
        }
        let candidates = tokio::time::timeout_at(
            deadline,
            self.maintenance.sandbox_maintenance_candidates(&page),
        )
        .await
        .map_err(|_| unavailable_before_batch(pause_now, "discovery_timeout"))?
        .map_err(|_| unavailable_before_batch(pause_now, "discovery_error"))?;
        if candidates.items.len() > usize::from(page.limit) {
            return Err(unavailable_before_batch(
                pause_now,
                "invalid_discovery_page",
            ));
        }
        let mut previous = page.cursor.as_deref();
        for id in &candidates.items {
            if previous.is_some_and(|value| value >= id.as_str()) {
                return Err(unavailable_before_batch(
                    pause_now,
                    "invalid_discovery_page",
                ));
            }
            previous = Some(id.as_str());
        }
        if candidates
            .next
            .as_deref()
            .is_some_and(|next| previous != Some(next) || candidates.items.is_empty())
        {
            return Err(unavailable_before_batch(
                pause_now,
                "invalid_discovery_page",
            ));
        }
        let mut cursor = page.cursor.clone();
        for id in candidates.items {
            if tokio::time::Instant::now() >= deadline {
                batch.next = Some(cursor.unwrap_or_default());
                metrics::counter!("cma_auto_pause_operator_batches_total", "mode" => if pause_now { "pause" } else { "preview" }, "outcome" => "partial").increment(1);
                return Ok(batch);
            }
            let request = SandboxInactivityPauseRequest {
                sandbox_id: id.clone(),
                requested_at: crate::now_rfc3339(),
                pause_after_seconds: policy.settings.inactivity.pause_after_seconds,
                auto_pause_enabled: true,
                expected_policy_revision: Some(policy.revision),
                cause: if pause_now {
                    InactivityPauseCause::OperatorInactivity
                } else {
                    InactivityPauseCause::Inactivity
                },
            };
            let attempt = async {
                let intent = if pause_now {
                    // Acknowledged durable actor intent is a prerequisite to
                    // dispatch. Losing the later outcome cannot erase authority.
                    Some(self.audit(caller, &request, "requested", None).await?)
                } else {
                    None
                };
                let result = if pause_now {
                    self.maintenance
                        .schedule_inactivity_pause(request.clone())
                        .await
                        .map(|outcome| InactivityPausePreview {
                            outcome,
                            refusal: None,
                        })
                } else {
                    self.maintenance
                        .preview_inactivity_pause_with_reason(request.clone())
                        .await
                };
                if let Some(intent) = intent {
                    let status = match &result {
                        Ok(result) => outcome_code(result.outcome),
                        Err(_) => "uncertain",
                    };
                    self.audit(caller, &request, status, Some(intent)).await?;
                }
                Ok::<_, OperatorError>(result)
            };
            let result = tokio::time::timeout_at(deadline, attempt).await;
            match result {
                Ok(Ok(Ok(result))) => match result.outcome {
                    SandboxMaintenanceOutcome::Scheduled if pause_now => batch.scheduled += 1,
                    SandboxMaintenanceOutcome::Scheduled => batch.candidates += 1,
                    SandboxMaintenanceOutcome::AlreadyScheduled => batch.already_satisfied += 1,
                    SandboxMaintenanceOutcome::NotEligible => {
                        batch.skipped += 1;
                        if !pause_now {
                            batch.refusals.push(AutoPauseSandboxRefusal {
                                sandbox_id: id.clone(),
                                reason: result
                                    .refusal
                                    .unwrap_or(InactivityPauseRefusal::EvidenceUnavailable),
                            });
                        }
                    }
                },
                failure => {
                    batch.failed += 1;
                    if !pause_now {
                        let reason = match failure {
                            Ok(Ok(Err(
                                SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict,
                            ))) => InactivityPauseRefusal::InvalidStoredEvidence,
                            _ => InactivityPauseRefusal::EvidenceUnavailable,
                        };
                        batch.refusals.push(AutoPauseSandboxRefusal {
                            sandbox_id: id,
                            reason,
                        });
                    }
                    batch.next = Some(cursor.unwrap_or_default());
                    metrics::counter!("cma_auto_pause_operator_batches_total", "mode" => if pause_now { "pause" } else { "preview" }, "outcome" => "partial").increment(1);
                    return Ok(batch);
                }
            }
            cursor = Some(id.to_string());
        }
        batch.next = candidates.next;
        metrics::counter!("cma_auto_pause_operator_batches_total", "mode" => if pause_now { "pause" } else { "preview" }, "outcome" => "complete").increment(1);
        Ok(batch)
    }

    async fn audit(
        &self,
        caller: &Caller,
        request: &SandboxInactivityPauseRequest,
        status: &str,
        intent: Option<String>,
    ) -> Result<String, OperatorError> {
        let mut record = ActivityRecord::new(
            ActivityAction::Configure,
            if status == "uncertain" {
                ActivityOutcome::Failed("auto_pause_outcome_unknown".into())
            } else {
                ActivityOutcome::Succeeded
            },
        )
        .by(caller.subject.clone())
        .on(ActivityTarget::Resource(format!(
            "sandbox:{}",
            request.sandbox_id
        )));
        // Join an interrupted attempt to its exact lifecycle admission/event
        // timestamp during reconciliation, even after the process restarts.
        record.at = request.requested_at.clone();
        record.detail.record(DetailKey::Setting, "auto_pause");
        record.detail.record(DetailKey::Status, status);
        record.detail.record(
            DetailKey::Revision,
            request
                .expected_policy_revision
                .unwrap_or_default()
                .to_string(),
        );
        record
            .detail
            .record(DetailKey::Limit, request.pause_after_seconds.to_string());
        if let Some(intent) = intent {
            record.detail.record(DetailKey::CorrelationId, intent);
        }
        self.activity
            .record(&record)
            .await
            .map_err(|_| unavailable())?;
        Ok(record.id.0)
    }
}

fn unavailable_before_batch(pause_now: bool, reason: &'static str) -> OperatorError {
    metrics::counter!(
        "cma_auto_pause_operator_unavailable_total",
        "mode" => if pause_now { "pause" } else { "preview" },
        "reason" => reason
    )
    .increment(1);
    unavailable()
}

fn unavailable() -> OperatorError {
    OperatorError::Unavailable("auto-pause evidence or audit unavailable".into())
}
fn outcome_code(outcome: SandboxMaintenanceOutcome) -> &'static str {
    match outcome {
        SandboxMaintenanceOutcome::Scheduled => "scheduled",
        SandboxMaintenanceOutcome::AlreadyScheduled => "already_satisfied",
        SandboxMaintenanceOutcome::NotEligible => "not_eligible",
    }
}

#[cfg(test)]
mod tests;
