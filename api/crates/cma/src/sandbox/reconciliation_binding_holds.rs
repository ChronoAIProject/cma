//! Process-local scheduling only. Holds add protection; they never grant authority.
use layer2_core::{sandbox::Sandbox, SandboxId, SubjectId};
use layer3_adapter::RuntimeObservation;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct BindingHolds {
    holds: Mutex<BTreeMap<SandboxId, Hold>>,
    metrics_enabled: AtomicBool,
    diagnostics_enabled: AtomicBool,
}

struct Hold {
    attempts: u32,
    ready_at: Instant,
    reason: &'static str,
    ids: BTreeSet<String>,
    handles: BTreeSet<String>,
    diagnostic: Option<Diagnostic>,
}

struct Diagnostic {
    owner: Option<SubjectId>,
    first_held_at: Option<SystemTime>,
    retrying: bool,
    adapter_unavailable: bool,
}

#[path = "reconciliation_binding_observation.rs"]
mod observation;

impl Hold {
    fn protect(&mut self, sandbox: &Sandbox, inventory: &[RuntimeObservation]) {
        self.ids.insert(sandbox.id.to_string());
        for binding in sandbox
            .runtime
            .iter()
            .chain(sandbox.lifecycle.suspended_runtime.as_deref())
        {
            self.handles.insert(binding.runtime_id.clone());
        }
        if let Some(home) = &sandbox.lifecycle.home {
            self.ids.insert(home.request.sandbox_id.clone());
        }
        // A conflicting handle can advertise a different logical owner. Protect
        // its claim/home too, rather than interpreting the mismatch as orphanhood.
        for runtime in inventory {
            if self.handles.contains(runtime.handle.as_str()) {
                self.ids.insert(runtime.sandbox_id.clone());
            }
        }
    }
}

impl BindingHolds {
    pub(super) fn set_metrics(&self, enabled: bool) {
        self.metrics_enabled.store(enabled, Ordering::Relaxed);
    }

    pub(super) fn enable_diagnostics(&self) {
        self.diagnostics_enabled.store(true, Ordering::Relaxed);
    }

    /// Scheduling hint only: the same fresh authority checks still run on retry.
    pub(super) fn reserved_retry_due(&self, id: &SandboxId, bounded: bool) -> bool {
        self.holds.lock().unwrap().get(id).is_some_and(|hold| {
            hold.attempts > 0
                && Instant::now() >= hold.ready_at
                && (matches!(hold.reason, "deadline" | "cancelled")
                    || bounded && hold.reason == "raced")
        })
    }

    pub(super) fn begin<'a>(
        &'a self,
        sandbox: &Sandbox,
        inventory: &[RuntimeObservation],
    ) -> Option<BindingAttempt<'a>> {
        let mut holds = self.holds.lock().unwrap();
        let hold = holds.entry(sandbox.id.clone()).or_insert_with(|| Hold {
            attempts: 0,
            ready_at: Instant::now(),
            reason: "cancelled",
            ids: BTreeSet::new(),
            handles: BTreeSet::new(),
            diagnostic: self
                .diagnostics_enabled
                .load(Ordering::Relaxed)
                .then(|| Diagnostic {
                    owner: (!sandbox.owner.as_str().is_empty()
                        && sandbox.owner.as_str().len() <= 512
                        && !sandbox.owner.as_str().chars().any(char::is_control))
                    .then(|| sandbox.owner.clone()),
                    first_held_at: None,
                    retrying: false,
                    adapter_unavailable: false,
                }),
        });
        hold.protect(sandbox, inventory);
        if Instant::now() < hold.ready_at {
            return None;
        }
        hold.attempts = hold.attempts.saturating_add(1);
        if let Some(diagnostic) = &mut hold.diagnostic {
            diagnostic.retrying = true;
        }
        Some(BindingAttempt {
            holds: self,
            id: sandbox.id.clone(),
            complete: false,
        })
    }

    // Only a complete, validated global scan can retire an absent hold. Fresh
    // pool/owned/provider fences still decide every subsequent reclamation.
    pub(super) fn retain_authority(&self, authority: &BTreeMap<SandboxId, Sandbox>) {
        self.holds
            .lock()
            .unwrap()
            .retain(|id, _| authority.contains_key(id));
    }

    pub(super) fn observe_inventory(&self, sandbox: &Sandbox, inventory: &[RuntimeObservation]) {
        if let Some(hold) = self.holds.lock().unwrap().get_mut(&sandbox.id) {
            hold.protect(sandbox, inventory);
        }
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.holds
            .lock()
            .unwrap()
            .values()
            .any(|hold| hold.ids.contains(id))
    }

    pub(super) fn protects_runtime(&self, runtime: &RuntimeObservation) -> bool {
        self.holds.lock().unwrap().values().any(|hold| {
            hold.ids.contains(&runtime.sandbox_id) || hold.handles.contains(runtime.handle.as_str())
        })
    }

    pub(super) fn failed(&self, id: &SandboxId, reason: &'static str) {
        let mut holds = self.holds.lock().unwrap();
        let hold = holds
            .get_mut(id)
            .expect("attempt retains protection until success");
        let seconds = 30u64
            .saturating_mul(1 << hold.attempts.saturating_sub(1).min(4))
            .min(300);
        hold.ready_at = Instant::now() + Duration::from_secs(seconds);
        hold.reason = reason;
        if let Some(diagnostic) = &mut hold.diagnostic {
            diagnostic.first_held_at.get_or_insert_with(SystemTime::now);
            diagnostic.retrying = false;
        }
    }

    pub(super) fn report_on_drop(&self) -> ReportOnDrop<'_> {
        ReportOnDrop(self)
    }

    fn report(&self) {
        let holds = self.holds.lock().unwrap();
        for reason in [
            "cancelled",
            "raced",
            "deadline",
            "unavailable",
            "corrupt",
            "conflict",
            "refused",
        ] {
            let count = holds.values().filter(|hold| hold.reason == reason).count();
            if count > 0 {
                tracing::info!(parent: None, reason, held_bindings = count, "sandbox binding holds");
            }
        }
        tracing::info!(parent: None, held_bindings = holds.len(), "sandbox binding hold summary");
        if self.metrics_enabled.load(Ordering::Relaxed) {
            metrics::gauge!("cma_sandbox_binding_holds_local").set(holds.len() as f64);
        }
    }
}

pub(super) struct BindingAttempt<'a> {
    holds: &'a BindingHolds,
    id: SandboxId,
    complete: bool,
}

impl BindingAttempt<'_> {
    pub(super) fn observe(&self, sandbox: &Sandbox, inventory: &[RuntimeObservation]) {
        self.holds
            .holds
            .lock()
            .unwrap()
            .get_mut(&self.id)
            .unwrap()
            .protect(sandbox, inventory);
    }
    pub(super) fn complete(mut self) {
        self.holds.holds.lock().unwrap().remove(&self.id);
        self.complete = true;
    }

    /// Retain protection after a durable CAS race. This is retryable and is
    /// reported separately from cancellation of the sweep operation.
    pub(super) fn raced(mut self) {
        self.holds.failed(&self.id, "raced");
        self.complete = true;
    }

    pub(super) fn refused(mut self) {
        self.holds.failed(&self.id, "refused");
        self.complete = true;
    }

    /// Retain protection after a transient runtime failure at startup. The
    /// background sweep retries it with fresh reads under the same fences.
    pub(super) fn unavailable(mut self) {
        self.holds.failed(&self.id, "unavailable");
        if let Some(diagnostic) = self
            .holds
            .holds
            .lock()
            .unwrap()
            .get_mut(&self.id)
            .and_then(|hold| hold.diagnostic.as_mut())
        {
            diagnostic.adapter_unavailable = true;
        }
        self.complete = true;
    }
}

impl Drop for BindingAttempt<'_> {
    fn drop(&mut self) {
        if !self.complete {
            // Includes outer cancellation and timeout. No lease or pending I/O
            // survives the attempt, and the delay starts at completion/drop.
            self.holds.failed(&self.id, "cancelled");
        }
    }
}

/// Emit hold counts even when the total-work budget cancels the sweep future.
pub(super) struct ReportOnDrop<'a>(&'a BindingHolds);
impl Drop for ReportOnDrop<'_> {
    fn drop(&mut self) {
        self.0.report();
    }
}
