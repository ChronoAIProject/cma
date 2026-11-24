use super::*;
use layer2_core::binding_diagnostics::{
    AdapterFailureClass, BindingHold, BindingHoldSample, HoldPhase, HoldReason,
    MAX_HOLDS_PER_REPORTER,
};

impl crate::binding_diagnostics::LocalSource for BindingHolds {
    fn sample(&self) -> BindingHoldSample {
        let holds = self.holds.lock().unwrap();
        let wall = SystemTime::now();
        let now = Instant::now();
        let mut sample = BindingHoldSample {
            observed_at: layer2_core::rfc3339_at(wall),
            truncated: holds.len() > MAX_HOLDS_PER_REPORTER,
            holds: Vec::new(),
        };
        for (id, hold) in holds.iter().take(MAX_HOLDS_PER_REPORTER) {
            let Some(diagnostic) = &hold.diagnostic else {
                continue;
            };
            let Some(first_held_at) = diagnostic.first_held_at else {
                continue;
            };
            let Some(owner) = &diagnostic.owner else {
                sample.truncated = true;
                continue;
            };
            if id.as_str().is_empty()
                || id.as_str().len() > 256
                || id.as_str().chars().any(char::is_control)
            {
                sample.truncated = true;
                continue;
            }
            let reason = match hold.reason {
                "unavailable" => HoldReason::Unavailable,
                "raced" => HoldReason::Raced,
                "cancelled" => HoldReason::Cancelled,
                "deadline" => HoldReason::Deadline,
                "corrupt" => HoldReason::Corrupt,
                "conflict" => HoldReason::Conflict,
                "refused" => HoldReason::Refused,
                _ => {
                    sample.truncated = true;
                    continue;
                }
            };
            sample.holds.push(BindingHold {
                sandbox_id: id.clone(),
                owner: owner.clone(),
                reason,
                phase: if diagnostic.retrying {
                    HoldPhase::Retrying
                } else {
                    HoldPhase::Deferred
                },
                first_held_at: layer2_core::rfc3339_at(first_held_at),
                attempts: hold.attempts,
                retry_not_before: if diagnostic.retrying {
                    None
                } else {
                    wall.checked_add(hold.ready_at.saturating_duration_since(now))
                        .map(layer2_core::rfc3339_at)
                },
                adapter_error_class: diagnostic
                    .adapter_unavailable
                    .then_some(AdapterFailureClass::Unavailable),
            });
        }
        sample
    }
}
