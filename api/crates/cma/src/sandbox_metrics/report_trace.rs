//! One content-free generation breakdown, including the stage cancelled by a deadline.
use layer2_core::sandbox_metrics::Result;
use sha2::{Digest, Sha256};
use std::{future::Future, time::Instant};

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Inventory,
    Allocation,
    Measured,
    Source,
    Write,
    Authority,
}
impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Inventory => "inventory",
            Self::Allocation => "allocation",
            Self::Measured => "measured",
            Self::Source => "source",
            Self::Write => "write",
            Self::Authority => "authority",
        }
    }
}
pub(super) fn report_hash(id: &str) -> String {
    format!("{:x}", Sha256::digest(id.as_bytes()))
}
pub(super) struct ReportTrace {
    hash: String,
    started: Instant,
    active: Option<(Stage, Instant)>,
    elapsed_us: [u64; 6],
    calls: [u64; 6],
    outcome: &'static str,
}
impl ReportTrace {
    pub fn new(id: &str) -> Self {
        Self {
            hash: report_hash(id),
            started: Instant::now(),
            active: None,
            elapsed_us: [0; 6],
            calls: [0; 6],
            outcome: "cancelled",
        }
    }
    pub async fn stage<T>(
        &mut self,
        stage: Stage,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let started = Instant::now();
        self.active = Some((stage, started));
        self.calls[stage as usize] += 1;
        let result = future.await;
        self.elapsed_us[stage as usize] += started.elapsed().as_micros() as u64;
        self.active = None;
        if result.is_err() {
            self.outcome = "failed";
        }
        result
    }
    pub fn finish(&mut self, success: bool) {
        self.outcome = if success { "complete" } else { "failed" };
    }
}
impl Drop for ReportTrace {
    fn drop(&mut self) {
        if let Some((stage, started)) = self.active {
            self.elapsed_us[stage as usize] += started.elapsed().as_micros() as u64;
        }
        let total_us = self.started.elapsed().as_micros() as u64;
        // JSON formatters may attach the current span even to a parentless
        // event. Enter an empty root so callers cannot leak payload fields.
        let _root = tracing::info_span!(parent: None, "usage_report_diagnostic").entered();
        tracing::info!(parent:None,event="usage_report_generation_stages",report_hash=%self.hash,
            elapsed_ms=total_us/1000,other_us=total_us.saturating_sub(self.elapsed_us.iter().sum()),outcome=self.outcome,
            active_stage=self.active.map_or("none",|(s,_)|s.label()),
            inventory_us=self.elapsed_us[0],allocation_us=self.elapsed_us[1],measured_us=self.elapsed_us[2],
            source_us=self.elapsed_us[3],write_us=self.elapsed_us[4],authority_us=self.elapsed_us[5],
            inventory_calls=self.calls[0],allocation_calls=self.calls[1],measured_calls=self.calls[2],
            source_calls=self.calls[3],write_calls=self.calls[4],authority_calls=self.calls[5],
            "Sandbox report generation stages");
    }
}

#[cfg(test)]
#[path = "report_trace_tests.rs"]
mod tests;
