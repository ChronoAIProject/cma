//! Fixed categories only: never attach Sandbox IDs, errors or caller spans.
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Inventory,
    Authority,
    Bindings,
    Runtimes,
    ClaimInventory,
    Claims,
    HomeInventory,
    Homes,
    RecoveryAuthority,
    Recovery,
    Policy,
    Candidates,
    Profiles,
    RuntimeUpdates,
    IdlePause,
    Complete,
}

impl Phase {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Inventory => "runtime_inventory",
            Self::Authority => "authority",
            Self::Bindings => "bindings",
            Self::Runtimes => "runtime_cleanup",
            Self::ClaimInventory => "claim_inventory",
            Self::Claims => "claim_cleanup",
            Self::HomeInventory => "home_inventory",
            Self::Homes => "home_cleanup",
            Self::RecoveryAuthority => "recovery_authority",
            Self::Recovery => "failed_recovery",
            Self::Policy => "policy",
            Self::Candidates => "maintenance_candidates",
            Self::Profiles => "profiles",
            Self::RuntimeUpdates => "runtime_updates",
            Self::IdlePause => "idle_pause",
            Self::Complete => "complete",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    items: u64,
    elapsed: Duration,
    operations: u64,
}

pub(super) struct Telemetry {
    started: Instant,
    attempts: u64,
    items: u64,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            attempts: 0,
            items: 0,
        }
    }
}

impl Telemetry {
    pub(super) fn attempt(&mut self) -> Attempt<'_> {
        self.attempts += 1;
        Attempt {
            sweep: self,
            started: Instant::now(),
            phase: Phase::Inventory,
            operation_started: None,
            samples: [Sample::default(); Phase::Complete as usize + 1],
            reason: "cancelled",
        }
    }
}

pub(super) struct Attempt<'a> {
    sweep: &'a mut Telemetry,
    started: Instant,
    phase: Phase,
    operation_started: Option<Instant>,
    samples: [Sample; Phase::Complete as usize + 1],
    reason: &'static str,
}

impl Attempt<'_> {
    pub(super) fn start(&mut self, phase: Phase) {
        self.phase = phase;
        self.operation_started = Some(Instant::now());
    }

    pub(super) fn record(&mut self, items: u64) {
        if let Some(started) = self.operation_started.take() {
            let sample = &mut self.samples[self.phase as usize];
            sample.elapsed += started.elapsed();
            sample.operations += 1;
            sample.items += items;
            self.sweep.items += items;
        }
    }

    pub(super) fn finish(&mut self, reason: &'static str) {
        self.reason = reason;
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        // Also observe time spent in a cancelled/failed operation, without
        // claiming its uncertain effects as processed items.
        self.record(0);
        for phase in [
            Phase::Inventory,
            Phase::Authority,
            Phase::Bindings,
            Phase::Runtimes,
            Phase::ClaimInventory,
            Phase::Claims,
            Phase::HomeInventory,
            Phase::Homes,
            Phase::RecoveryAuthority,
            Phase::Recovery,
            Phase::Policy,
            Phase::Candidates,
            Phase::Profiles,
            Phase::RuntimeUpdates,
            Phase::IdlePause,
        ] {
            let sample = self.samples[phase as usize];
            if sample.operations > 0 {
                tracing::info!(parent: None,
                    phase = phase.name(), items_processed = sample.items,
                    operations = sample.operations, elapsed_ms = sample.elapsed.as_millis() as u64,
                    "sandbox maintenance phase progress"
                );
            }
        }
        tracing::info!(parent: None,
            phase = self.phase.name(), reason = self.reason,
            items_processed = self.samples.iter().map(|sample| sample.items).sum::<u64>(),
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            sweep_elapsed_ms = self.sweep.started.elapsed().as_millis() as u64,
            attempt = self.sweep.attempts,
            "sandbox maintenance attempt completed"
        );
        if self.reason == "complete" {
            tracing::info!(parent: None,
                elapsed_ms = self.sweep.started.elapsed().as_millis() as u64,
                attempts = self.sweep.attempts, items_processed = self.sweep.items,
                "sandbox maintenance sweep completed"
            );
        }
    }
}
