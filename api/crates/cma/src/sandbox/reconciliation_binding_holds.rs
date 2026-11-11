//! Process-local scheduling only. Holds add protection; they never grant authority.
use layer2_core::{sandbox::Sandbox, SandboxId};
use layer3_adapter::RuntimeObservation;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::Duration,
};
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct BindingHolds(Mutex<BTreeMap<SandboxId, Hold>>);

struct Hold {
    attempts: u32,
    ready_at: Instant,
    reason: &'static str,
    ids: BTreeSet<String>,
    handles: BTreeSet<String>,
}

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
    /// Scheduling hint only: the same fresh authority checks still run on retry.
    pub(super) fn reserved_retry_due(&self, id: &SandboxId) -> bool {
        self.0.lock().unwrap().get(id).is_some_and(|hold| {
            hold.attempts > 0
                && Instant::now() >= hold.ready_at
                && matches!(hold.reason, "deadline" | "cancelled")
        })
    }

    pub(super) fn begin<'a>(
        &'a self,
        sandbox: &Sandbox,
        inventory: &[RuntimeObservation],
    ) -> Option<BindingAttempt<'a>> {
        let mut holds = self.0.lock().unwrap();
        let hold = holds.entry(sandbox.id.clone()).or_insert_with(|| Hold {
            attempts: 0,
            ready_at: Instant::now(),
            reason: "cancelled",
            ids: BTreeSet::new(),
            handles: BTreeSet::new(),
        });
        hold.protect(sandbox, inventory);
        if Instant::now() < hold.ready_at {
            return None;
        }
        hold.attempts = hold.attempts.saturating_add(1);
        Some(BindingAttempt {
            holds: self,
            id: sandbox.id.clone(),
            complete: false,
        })
    }

    // Only a complete, validated global scan can retire an absent hold. Fresh
    // pool/owned/provider fences still decide every subsequent reclamation.
    pub(super) fn retain_authority(&self, authority: &BTreeMap<SandboxId, Sandbox>) {
        self.0
            .lock()
            .unwrap()
            .retain(|id, _| authority.contains_key(id));
    }

    pub(super) fn observe_inventory(&self, sandbox: &Sandbox, inventory: &[RuntimeObservation]) {
        if let Some(hold) = self.0.lock().unwrap().get_mut(&sandbox.id) {
            hold.protect(sandbox, inventory);
        }
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .values()
            .any(|hold| hold.ids.contains(id))
    }

    pub(super) fn protects_runtime(&self, runtime: &RuntimeObservation) -> bool {
        self.0.lock().unwrap().values().any(|hold| {
            hold.ids.contains(&runtime.sandbox_id) || hold.handles.contains(runtime.handle.as_str())
        })
    }

    pub(super) fn failed(&self, id: &SandboxId, reason: &'static str) {
        let mut holds = self.0.lock().unwrap();
        let hold = holds
            .get_mut(id)
            .expect("attempt retains protection until success");
        let seconds = 30u64
            .saturating_mul(1 << hold.attempts.saturating_sub(1).min(4))
            .min(300);
        hold.ready_at = Instant::now() + Duration::from_secs(seconds);
        hold.reason = reason;
    }

    pub(super) fn report_on_drop(&self) -> ReportOnDrop<'_> {
        ReportOnDrop(self)
    }

    fn report(&self) {
        let holds = self.0.lock().unwrap();
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
            .0
            .lock()
            .unwrap()
            .get_mut(&self.id)
            .unwrap()
            .protect(sandbox, inventory);
    }
    pub(super) fn complete(mut self) {
        self.holds.0.lock().unwrap().remove(&self.id);
        self.complete = true;
    }

    /// Retain protection after a durable CAS race. This is retryable and is
    /// reported separately from cancellation of the sweep operation.
    pub(super) fn raced(mut self) {
        self.holds.failed(&self.id, "raced");
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
