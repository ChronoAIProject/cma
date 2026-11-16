//! Sample only the native measurement process; Mongo RSS is outside this clock.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

fn rss_kib() -> u64 {
    if let Ok(text) = std::fs::read_to_string("/proc/self/status") {
        if let Some(line) = text.lines().find(|line| line.starts_with("VmRSS:")) {
            return line
                .split_whitespace()
                .nth(1)
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        }
    }
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
pub(super) struct Rss {
    before: u64,
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Rss {
    pub(super) fn start() -> Self {
        let before = rss_kib();
        let peak = Arc::new(AtomicU64::new(before));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (peak.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(rss_kib(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        Self {
            before,
            peak,
            stop,
            thread: Some(thread),
        }
    }
    pub(super) fn finish(mut self) -> Value {
        let after = rss_kib();
        self.peak.fetch_max(after, Ordering::Relaxed);
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
        json!({"before_kib":self.before,"sampled_peak_kib":self.peak.load(Ordering::Relaxed),
            "after_kib":after,"sample_interval_ms":200,"process":"native_test_binary",
            "mongo_rss_included":false})
    }
}
impl Drop for Rss {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
