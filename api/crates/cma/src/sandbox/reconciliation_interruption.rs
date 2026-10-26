//! Optional interruption diagnostics do not block retained-home recovery.
use layer2_core::{
    sandbox::{Sandbox, SandboxInterruptionCause},
    seconds_from_rfc3339,
};
use layer3_adapter::{RuntimeHandle, RuntimeTermination, SandboxAdapter};
use std::time::{Duration, SystemTime};

pub(super) async fn observe(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    now: SystemTime,
) -> SandboxInterruptionCause {
    let Some(binding) = &sandbox.runtime else {
        return SandboxInterruptionCause::RuntimeUnavailable;
    };
    match tokio::time::timeout(
        Duration::from_secs(2),
        adapter.runtime_termination(&RuntimeHandle(binding.runtime_id.clone())),
    )
    .await
    {
        Ok(Ok(Some(evidence))) => {
            classify(&evidence, &binding.bound_at, &layer2_core::rfc3339_at(now))
        }
        Ok(Err(error)) => {
            tracing::warn!(sandbox_id = %sandbox.id, %error, "runtime interruption evidence unavailable");
            SandboxInterruptionCause::RuntimeUnavailable
        }
        _ => SandboxInterruptionCause::RuntimeUnavailable,
    }
}

fn classify(evidence: &RuntimeTermination, bound_at: &str, now: &str) -> SandboxInterruptionCause {
    let times = seconds_from_rfc3339(&evidence.finished_at)
        .zip(seconds_from_rfc3339(bound_at))
        .zip(seconds_from_rfc3339(now));
    if evidence.out_of_memory
        && times.is_some_and(|((finished, bound), now)| finished >= bound && finished <= now)
    {
        SandboxInterruptionCause::OutOfMemory
    } else {
        SandboxInterruptionCause::RuntimeUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_unknown_and_future_terminations_do_not_mislabel_a_new_interruption() {
        let bound = "2026-10-16T09:00:00Z";
        let now = "2026-10-16T09:01:00Z";
        for (finished, explicit, expected) in [
            (
                "2026-10-16T09:00:20Z",
                true,
                SandboxInterruptionCause::OutOfMemory,
            ),
            (
                "2026-10-16T08:59:59Z",
                true,
                SandboxInterruptionCause::RuntimeUnavailable,
            ),
            (
                "2026-10-16T09:01:01Z",
                true,
                SandboxInterruptionCause::RuntimeUnavailable,
            ),
            (
                "2026-10-16T09:00:20Z",
                false,
                SandboxInterruptionCause::RuntimeUnavailable,
            ),
            (
                "invalid",
                true,
                SandboxInterruptionCause::RuntimeUnavailable,
            ),
        ] {
            assert_eq!(
                classify(
                    &RuntimeTermination {
                        out_of_memory: explicit,
                        finished_at: finished.into()
                    },
                    bound,
                    now
                ),
                expected
            );
        }
    }
}
