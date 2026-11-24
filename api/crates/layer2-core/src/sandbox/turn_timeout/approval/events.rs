use super::*;
use crate::sandbox::turn_timeout::{TimeoutDisposition, TurnTimeout, TurnTimeoutCause};
use crate::sandbox::{AgentLogKind, ExternalApprovalResolution};

#[derive(PartialEq, Eq)]
struct View {
    id: String,
    provider: String,
    start: i64,
    deadline: i64,
    resolution: Option<ExternalApprovalResolution>,
}

impl TurnTimeout {
    /// Public revisions order committed CMA observations. Provider revisions
    /// remain independently checked by the private receipt reducer.
    pub fn approval_events_since(
        &self,
        previous: &Self,
    ) -> Result<Vec<AgentLogKind>, TurnTimeoutError> {
        self.validate()?;
        previous.validate()?;
        if self.binding != previous.binding
            || self.action_id != previous.action_id
            || self.revision < previous.revision
            || (self.revision == previous.revision && self != previous)
        {
            return Err(TurnTimeoutError::Conflict);
        }
        let before = views(previous)?;
        let mut events = Vec::new();
        for view in views(self)? {
            if before.iter().any(|old| old == &view) {
                continue;
            }
            let turn_sequence = self.binding.turn_sequence;
            events.push(match view.resolution {
                Some(resolution) => AgentLogKind::ExternalApprovalResolved {
                    approval_id: view.id,
                    provider: view.provider,
                    turn_sequence,
                    revision: self.revision,
                    resolution,
                },
                None => AgentLogKind::ExternalApprovalRequested {
                    approval_id: view.id,
                    provider: view.provider,
                    turn_sequence,
                    revision: self.revision,
                    waiting_since: timestamp(view.start)?,
                    deadline_at: timestamp(view.deadline)?,
                },
            });
        }
        Ok(events)
    }

    pub fn is_waiting_for_external_approval(&self) -> bool {
        self.is_open()
            && self
                .approval
                .as_ref()
                .is_some_and(ApprovalBudget::is_waiting)
    }
}

fn views(policy: &TurnTimeout) -> Result<Vec<View>, TurnTimeoutError> {
    let Some(budget) = &policy.approval else {
        return Ok(Vec::new());
    };
    budget
        .operations
        .iter()
        .filter_map(|operation| {
            let start = operation.started_at_ms?;
            let identity = (&policy.binding, &operation.binding);
            Some(fingerprint(&identity).map(|id| {
                let mut view = View {
                    id: format!("external_{id}"),
                    provider: operation.binding.provider.clone(),
                    start,
                    deadline: start
                        .saturating_add(budget.limits.episode_ms)
                        .min(
                            operation
                                .expires_at_ms
                                .expect("validated approval interval"),
                        )
                        .min(budget.budget_exhaustion_at().unwrap_or(i64::MAX)),
                    resolution: operation
                        .resolution
                        .map(|resolution| match resolution {
                            ApprovalResolution::Approved => ExternalApprovalResolution::Approved,
                            ApprovalResolution::Denied => ExternalApprovalResolution::Denied,
                            ApprovalResolution::Expired => ExternalApprovalResolution::Expired,
                            ApprovalResolution::Disconnected => {
                                ExternalApprovalResolution::Disconnected
                            }
                        })
                        .or(match policy.disposition {
                            TimeoutDisposition::Terminal { .. } => Some(match policy.cause {
                                TurnTimeoutCause::ServiceDeadline
                                | TurnTimeoutCause::ApprovalBudget => {
                                    ExternalApprovalResolution::TimedOut
                                }
                                TurnTimeoutCause::ApprovalDenied
                                | TurnTimeoutCause::ApprovalExpired
                                | TurnTimeoutCause::ApprovalDisconnected => {
                                    ExternalApprovalResolution::Cancelled
                                }
                            }),
                            TimeoutDisposition::Superseded => {
                                Some(ExternalApprovalResolution::Cancelled)
                            }
                            _ => None,
                        }),
                };
                if view.resolution.is_some() {
                    view.start = 0;
                    view.deadline = 0;
                }
                view
            }))
        })
        .collect()
}

fn timestamp(ms: i64) -> Result<String, TurnTimeoutError> {
    let format = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
    )
    .map_err(|_| TurnTimeoutError::Invalid)?;
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .map_err(|_| TurnTimeoutError::Invalid)?
        .format(&format)
        .map_err(|_| TurnTimeoutError::Invalid)
}

#[cfg(test)]
mod tests;
