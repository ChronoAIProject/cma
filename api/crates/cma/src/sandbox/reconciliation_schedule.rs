//! Binding refresh and retained maintenance share one serialized work budget.
use super::*;

impl SandboxReconciler {
    pub(crate) async fn run_due(&self, now: SystemTime) -> Result<bool, SandboxStoreError> {
        let mut progress = self.progress.lock().await;
        let Progress {
            sweep,
            binding_refresh,
            refresh_required,
            telemetry,
        } = progress.get_or_insert_with(Progress::default);
        // A retained tail gets a fresh binding pass before its next opportunity.
        // Its own cursor and uncertain cleanup item are never reset or skipped.
        if *refresh_required && binding_refresh.is_none() {
            *binding_refresh = Some(Sweep::default());
        }
        // A cancelled binding already has protection/backoff from its drop guard.
        sweep.pending_binding = None;
        if let Some(refresh) = binding_refresh.as_mut() {
            refresh.pending_binding = None;
        }
        let _holds_report = self.binding_holds.report_on_drop();
        let mut attempt = telemetry.attempt();
        let started = tokio::time::Instant::now();
        let complete = loop {
            let refreshing = binding_refresh.is_some();
            let current = binding_refresh.as_mut().unwrap_or(&mut *sweep);
            if !refreshing && !current.is_binding_phase() {
                // Set before awaiting, so errors, deadlines and outer cancellation
                // cannot strand future binding work behind this retained item.
                *refresh_required = true;
            }
            attempt.start(current.phase);
            match tokio::time::timeout(OPERATION, current.step(self, now)).await {
                Ok(Ok(items)) => attempt.record(items),
                Ok(Err(error)) if current.pending_binding.is_some() => {
                    let reason = match error {
                        SandboxStoreError::Unavailable(_) => "unavailable",
                        SandboxStoreError::Corrupt(_) => "corrupt",
                        SandboxStoreError::Conflict => "conflict",
                        _ => "refused",
                    };
                    self.binding_holds
                        .failed(&current.pending_binding.take().unwrap(), reason);
                    attempt.record(0);
                }
                Err(_) if current.pending_binding.is_some() => {
                    self.binding_holds
                        .failed(&current.pending_binding.take().unwrap(), "deadline");
                    attempt.record(0);
                }
                Ok(Err(error)) => {
                    attempt.finish("unavailable");
                    return Err(error);
                }
                Err(_) => {
                    attempt.finish("deadline");
                    return Err(SandboxStoreError::Unavailable(
                        "sandbox maintenance operation deadline exceeded".into(),
                    ));
                }
            }
            if refreshing && current.phase == Phase::Runtimes {
                // A refresh stops after all bindings. It never performs cleanup.
                // If this consumes the slice, the next invocation gives the
                // retained tail its opportunity instead of refreshing again.
                *binding_refresh = None;
                *refresh_required = false;
            }
            if binding_refresh.is_none() && sweep.phase == Phase::Complete {
                break true;
            }
            if started.elapsed() >= SLICE {
                break false;
            }
            tokio::task::yield_now().await;
        };
        attempt.finish(if complete { "complete" } else { "yielded" });
        drop(attempt);
        if complete {
            *progress = None;
        }
        Ok(complete)
    }
}
