//! Bounded, process-local retry handles captured only from actual child cleanup.
use layer2_core::sandbox::{Sandbox, SandboxStatus, SandboxWorkStep};
use layer3_adapter::receiver_failure::ReceiverFailureProbe;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct RetirementControlFailures(Mutex<BTreeMap<String, FailedAttempt>>);
struct FailedAttempt {
    parent: Sandbox,
    probe: Arc<dyn ReceiverFailureProbe>,
    at: Instant,
}
impl RetirementControlFailures {
    pub(crate) fn record(&self, parent: &Sandbox, probe: Arc<dyn ReceiverFailureProbe>) {
        if parent.status != SandboxStatus::Retiring
            || parent.work.step != SandboxWorkStep::StopAgents
        {
            return;
        }
        let mut entries = self.0.lock().expect("retirement observations");
        entries.retain(|_, value| value.at.elapsed() < Duration::from_secs(120));
        if entries.len() < 1024 || entries.contains_key(parent.id.as_str()) {
            entries.insert(
                parent.id.to_string(),
                FailedAttempt {
                    parent: parent.clone(),
                    probe,
                    at: Instant::now(),
                },
            );
        }
    }
    pub(crate) async fn failed_now(&self, parent: &Sandbox) -> bool {
        let probe = {
            let entries = self.0.lock().expect("retirement observations");
            entries
                .get(parent.id.as_str())
                .filter(|entry| {
                    entry.at.elapsed() < Duration::from_secs(120)
                        && entry.parent.revision <= parent.revision
                        && entry.parent.status == parent.status
                        && entry.parent.work.step == parent.work.step
                        && entry.parent.runtime == parent.runtime
                        && entry.parent.lifecycle == parent.lifecycle
                        && entry.parent.owner == parent.owner
                })
                .map(|entry| entry.probe.clone())
        };
        // Scheduling claims/retries may advance the revision since child cleanup.
        // The old observation is never proof: repeat its exact challenge within
        // the current finalizer attempt, then revalidate this parent revision.
        match probe {
            Some(probe) => probe.still_unavailable().await,
            None => false,
        }
    }
}
