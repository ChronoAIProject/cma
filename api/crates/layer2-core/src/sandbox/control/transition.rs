use crate::sandbox::store::{
    apply_sandbox_lifecycle_command, SandboxLifecycleCommand, SandboxLifecycleCommandApplication,
    SandboxLifecycleCommandDisposition,
};
use crate::sandbox::{Sandbox, SandboxEventKind, SandboxReviveCause, SandboxStatus};

use super::{
    SandboxControl, SandboxControlDisposition, SandboxControlError, SandboxControlFailure,
    SandboxControlRecord, SandboxControlStatus,
};

pub enum SandboxControlPlan {
    Transition {
        sandbox: Box<Sandbox>,
        event: SandboxEventKind,
    },
    NoChange {
        disposition: SandboxControlDisposition,
    },
    Reject(SandboxControlFailure),
}

/// Pure planner shared by every store; an authorization proof is still required
/// when the store commits this plan, so supplying a candidate is not authority.
pub fn plan_sandbox_control(
    record: &SandboxControlRecord,
    current: &Sandbox,
    at: &str,
    managed_runtime_version: Option<&str>,
) -> Result<SandboxControlPlan, SandboxControlError> {
    record.validate()?;
    current
        .validate()
        .map_err(|_| SandboxControlError::Corrupt)?;
    if !record.queued() {
        return Err(SandboxControlError::Conflict);
    }
    if record.expired(at)? {
        return Ok(SandboxControlPlan::Reject(
            SandboxControlFailure::QueueExpired,
        ));
    }
    if current.id != record.envelope.sandbox_id
        || current.created_at != record.envelope.sandbox_created_at
        || current.revision != record.envelope.expected_revision
    {
        return Ok(SandboxControlPlan::Reject(
            SandboxControlFailure::TargetChanged,
        ));
    }
    let mut command = match record.envelope.command {
        SandboxControl::Pause => SandboxLifecycleCommand::pause(
            current.id.clone(),
            current.owner.clone(),
            at.into(),
            None,
        ),
        SandboxControl::Revive => SandboxLifecycleCommand::revive(
            current.id.clone(),
            current.owner.clone(),
            at.into(),
            SandboxReviveCause::ExplicitRequest,
        ),
        SandboxControl::Retire => {
            if matches!(
                current.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            ) {
                return Ok(SandboxControlPlan::NoChange {
                    disposition: SandboxControlDisposition::AlreadySatisfied,
                });
            }
            let mut next = current.clone();
            next.begin_retirement(at.into())
                .map_err(|_| SandboxControlError::Conflict)?;
            next.validate().map_err(|_| SandboxControlError::Corrupt)?;
            let event = SandboxEventKind::StatusChanged {
                from: current.status,
                to: SandboxStatus::Retiring,
            };
            return Ok(SandboxControlPlan::Transition {
                sandbox: Box::new(next),
                event,
            });
        }
    };
    command.managed_runtime_version = managed_runtime_version.map(str::to_owned);
    Ok(
        match apply_sandbox_lifecycle_command(current.clone(), &command)
            .map_err(|_| SandboxControlError::Corrupt)?
        {
            SandboxLifecycleCommandApplication::Commit { sandbox, event, .. } => {
                SandboxControlPlan::Transition {
                    sandbox: Box::new(sandbox),
                    event,
                }
            }
            SandboxLifecycleCommandApplication::Accepted(outcome) => SandboxControlPlan::NoChange {
                disposition: match outcome.disposition {
                    SandboxLifecycleCommandDisposition::AlreadyScheduled => {
                        SandboxControlDisposition::AlreadyScheduled
                    }
                    SandboxLifecycleCommandDisposition::AlreadySatisfied => {
                        SandboxControlDisposition::AlreadySatisfied
                    }
                    SandboxLifecycleCommandDisposition::Scheduled => {
                        return Err(SandboxControlError::Corrupt)
                    }
                },
            },
            SandboxLifecycleCommandApplication::NotEligible(_) => {
                SandboxControlPlan::Reject(SandboxControlFailure::NotEligible)
            }
        },
    )
}

impl SandboxControlRecord {
    pub fn rejected(
        &self,
        reason: SandboxControlFailure,
        at: &str,
    ) -> Result<Self, SandboxControlError> {
        self.validate()?;
        if !self.queued() {
            return Err(SandboxControlError::Conflict);
        }
        let mut next = self.clone();
        next.revision = 2;
        next.status = SandboxControlStatus::Rejected {
            at: at.into(),
            reason,
        };
        next.validate()?;
        Ok(next)
    }
}
