//! Test-only measurements shared by the baseline and instrumented workload.
#[derive(Default)]
pub(super) struct Samples {
    latencies_ms: Vec<f64>,
    cpu_seconds: f64,
}

impl Samples {
    pub(super) fn record(&mut self, latencies_ms: Vec<f64>, cpu_seconds: f64) {
        self.latencies_ms.extend(latencies_ms);
        self.cpu_seconds += cpu_seconds;
    }

    pub(super) fn summary(&self) -> serde_json::Value {
        if self.latencies_ms.is_empty() {
            return serde_json::Value::Null;
        }
        let mut latencies = self.latencies_ms.clone();
        latencies.sort_by(f64::total_cmp);
        let count = latencies.len();
        serde_json::json!({
            "turns": count,
            "p50_ms": latencies[count.div_ceil(2) - 1],
            "p99_ms": latencies[(count * 99).div_ceil(100) - 1],
            "cpu_ms_per_turn": self.cpu_seconds * 1000.0 / count as f64,
        })
    }
}

#[cfg(unix)]
pub(super) fn cpu_seconds() -> f64 {
    // Process CPU includes the local HTTP peer and both writer threads. Sampling
    // ends after queued records arrive, before their JSON/privacy assertions.
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: the process CPU clock writes a fully initialized timespec.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) },
        0
    );
    time.tv_sec as f64 + time.tv_nsec as f64 / 1_000_000_000.0
}

#[cfg(not(unix))]
pub(super) fn cpu_seconds() -> f64 {
    f64::NAN
}
