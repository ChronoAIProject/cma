//! Binding refresh and retained maintenance share one serialized work budget.
use super::*;

impl SandboxReconciler {
    pub(crate) async fn run_due(&self, now: SystemTime) -> Result<bool, SandboxStoreError> {
        // Reserved binding work shares the outer opportunity with the prelude.
        let reserved_deadline = self
            .runtime_loss_bounded_proof_enabled
            .then(|| tokio::time::Instant::now() + Duration::from_secs(25));
        let mut progress = self.progress.lock().await;
        // Independent of sweep progress: a failing earlier phase must not hold
        // back platform spend transitions (contract §5.3).
        self.platform_spend_stop(now).await;
        self.reconcile_pending_homes(now).await;
        // The progress lock serializes both lanes, including outer cancellation.
        if self.failed_runtime_recovery.is_some() {
            let mut recovery = self.recovery_schedule.lock().await;
            // A platform spend stop defers automatic recovery, as it defers
            // managed runtime updates (decision 26): recovery reopens a failed
            // Sandbox as Active through an image rotation that starts a
            // runtime. Candidates keep their eligibility and recover after
            // the lift; the deferred turn goes to the ordinary sweep.
            if recovery.take_turn() && !self.platform_spend_stop(now).await {
                recovery.run(self).await?;
                return Ok(false);
            }
        }
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
        let mut dispatched = false;
        let complete = loop {
            let refreshing = binding_refresh.is_some();
            let current = binding_refresh.as_mut().unwrap_or(&mut *sweep);
            let reserved = current.reserved_binding_due(self);
            // Reserve before cursor advancement or I/O. Even a fast preceding
            // step uses this slice; a slow retry starts in the next empty one.
            if reserved && dispatched {
                break false;
            }
            let deadline = if reserved {
                // The bounded path leaves five seconds before the outer timeout.
                let deadline = reserved_deadline.unwrap_or(started + BINDING_OPPORTUNITY);
                if reserved_deadline.is_some() && tokio::time::Instant::now() >= deadline {
                    break false;
                }
                deadline
            } else {
                tokio::time::Instant::now() + OPERATION
            };
            if !refreshing && !current.is_binding_phase() {
                // Set before awaiting, so errors, deadlines and outer cancellation
                // cannot strand future binding work behind this retained item.
                *refresh_required = true;
            }
            attempt.start(current.phase);
            dispatched = true;
            let binding_step = current.phase == Phase::Bindings;
            let operation = tokio::time::timeout_at(deadline, current.step(self, now));
            let result = if binding_step {
                layer3_adapter::binding_diagnostics::scope(deadline, operation).await
            } else {
                operation.await
            };
            match result {
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
            if reserved || started.elapsed() >= SLICE {
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
