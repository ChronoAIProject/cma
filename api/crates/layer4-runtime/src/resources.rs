//! This pod's own resource counters, read from kernel interfaces.
//!
//! Samples contain integers only. Every figure that cannot be read is absent,
//! never zero, so an unavailable kernel interface cannot look like an idle
//! sandbox.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

/// Where a sampler reads from.
///
/// Held as fields rather than hard-coded so a test can point the same code at
/// a fixture tree. Nothing here is configuration: these are kernel interfaces,
/// and a deployment does not get to move them.
pub struct Roots {
    pub proc: PathBuf,
    pub cgroup_mount: PathBuf,
    pub net: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
            cgroup_mount: PathBuf::from("/sys/fs/cgroup"),
            net: PathBuf::from("/sys/class/net"),
        }
    }
}

/// One reading from this pod's cgroup and network namespace.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFrame {
    /// Monotonic milliseconds since this supervisor started.
    pub uptime_ms: u64,
    /// The raw cumulative processor counter from `cpu.stat`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    /// The processor ceiling in thousandths of one CPU.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_millis_limit: Option<u32>,
    /// Bytes currently charged to this cgroup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_used: Option<u64>,
    /// The cgroup's memory ceiling in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_limit: Option<u64>,
    /// The highest memory usage this cgroup has recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_peak: Option<u64>,
    /// The memory PSI `full avg10` percentage in tenths of a percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_permille: Option<u16>,
    /// Times memory usage reached the configured ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_hits: Option<u64>,
    /// Times an allocation reached cgroup OOM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocation_failures: Option<u64>,
    /// Processes killed by an OOM killer in this cgroup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kills: Option<u64>,
    /// Bytes received across readable non-loopback interfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    /// Bytes sent across readable non-loopback interfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
}

/// A stateless reader for this container's kernel counters.
pub struct Sampler {
    roots: Roots,
    /// This container's cgroup directory, where one was resolvable.
    cgroup: Option<PathBuf>,
    /// When this sampler was built, which is when the supervisor started.
    started_at: Instant,
}

impl Sampler {
    /// Resolve this pod's cgroup once and retain the monotonic start time.
    pub fn new(roots: Roots, started_at: Instant) -> Self {
        let cgroup = resolve_cgroup(&roots);
        match cgroup.as_ref() {
            Some(path) => {
                tracing::info!(cgroup = %path.display(), "resource sampler resolved this pod's cgroup")
            }
            None => {
                tracing::warn!("this kernel exposes no cgroup v2 hierarchy to this pod");
                tracing::info!("resource sampler started without a resolved cgroup");
            }
        }
        Self {
            roots,
            cgroup,
            started_at,
        }
    }

    /// Read one independent snapshot without deriving rates or retaining it.
    pub fn sample(&self, now: Instant) -> ResourceFrame {
        let uptime_ms = u64::try_from(now.saturating_duration_since(self.started_at).as_millis())
            .unwrap_or(u64::MAX);
        let (received_bytes, sent_bytes) = read_network(&self.roots.net);
        let mut frame = ResourceFrame {
            uptime_ms,
            cpu_usage_usec: None,
            cpu_millis_limit: None,
            memory_bytes_used: None,
            memory_bytes_limit: None,
            memory_bytes_peak: None,
            stall_permille: None,
            ceiling_hits: None,
            allocation_failures: None,
            kills: None,
            received_bytes,
            sent_bytes,
        };

        let Some(cgroup) = self.cgroup.as_ref() else {
            return frame;
        };
        frame.memory_bytes_used = read_u64(&cgroup.join("memory.current"));
        frame.memory_bytes_limit = read_limit(&cgroup.join("memory.max"));
        frame.memory_bytes_peak = read_u64(&cgroup.join("memory.peak"));
        frame.stall_permille = read_pressure(&cgroup.join("memory.pressure"));

        let events = read_keyed(&cgroup.join("memory.events"));
        frame.ceiling_hits = events.get("max").copied();
        frame.allocation_failures = events.get("oom").copied();
        frame.kills = events.get("oom_kill").copied();

        let processor = read_keyed(&cgroup.join("cpu.stat"));
        frame.cpu_usage_usec = processor.get("usage_usec").copied();
        frame.cpu_millis_limit = read_cpu_limit(&cgroup.join("cpu.max"));
        frame
    }
}

fn resolve_cgroup(roots: &Roots) -> Option<PathBuf> {
    let membership = std::fs::read_to_string(roots.proc.join("self/cgroup")).ok()?;
    let path = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?;
    let relative = path.strip_prefix('/')?;
    Some(roots.cgroup_mount.join(relative))
}

fn read_u64(path: &Path) -> Option<u64> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "resource counter cannot be read");
            return None;
        }
    };
    match raw.trim().parse() {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "resource counter is not an integer");
            None
        }
    }
}

fn read_limit(path: &Path) -> Option<u64> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "resource limit cannot be read");
            return None;
        }
    };
    let value = raw.trim();
    if value == "max" {
        return None;
    }
    match value.parse() {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "resource limit is not an integer");
            None
        }
    }
}

fn read_keyed(path: &Path) -> BTreeMap<String, u64> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "keyed resource counters cannot be read");
            return BTreeMap::new();
        }
    };
    raw.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (Some(key), Some(value), None) = (words.next(), words.next(), words.next()) else {
                tracing::debug!(path = %path.display(), "malformed resource counter line ignored");
                return None;
            };
            match value.parse() {
                Ok(value) => Some((key.to_string(), value)),
                Err(error) => {
                    tracing::debug!(path = %path.display(), %error, "malformed resource counter value ignored");
                    None
                }
            }
        })
        .collect()
}

fn read_pressure(path: &Path) -> Option<u16> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "memory pressure cannot be read");
            return None;
        }
    };
    let value = raw.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        if words.next() != Some("full") {
            return None;
        }
        words.find_map(|word| word.strip_prefix("avg10="))
    });
    let Some(value) = value else {
        tracing::debug!(path = %path.display(), "memory pressure has no full avg10 reading");
        return None;
    };
    let percent: f64 = match value.parse() {
        Ok(percent) => percent,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "memory pressure is not a percentage");
            return None;
        }
    };
    if !percent.is_finite() {
        tracing::debug!(path = %path.display(), "memory pressure is not finite");
        return None;
    }
    Some((percent * 10.0).round().clamp(0.0, 1000.0) as u16)
}

fn read_cpu_limit(path: &Path) -> Option<u32> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "processor limit cannot be read");
            return None;
        }
    };
    let mut words = raw.split_whitespace();
    let Some(quota) = words.next() else {
        tracing::debug!(path = %path.display(), "processor limit has no quota");
        return None;
    };
    if quota == "max" {
        return None;
    }
    let quota_usec: u128 = match quota.parse() {
        Ok(quota) => quota,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "processor quota is not an integer");
            return None;
        }
    };
    let Some(period) = words.next() else {
        tracing::debug!(path = %path.display(), "processor limit has no period");
        return None;
    };
    let period_usec: u128 = match period.parse() {
        Ok(period) => period,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "processor period is not an integer");
            return None;
        }
    };
    if period_usec == 0 {
        tracing::debug!(path = %path.display(), "processor limit has a zero period");
        return None;
    }
    let millis = quota_usec.saturating_mul(1000) / period_usec;
    Some(u32::try_from(millis).unwrap_or(u32::MAX))
}

fn read_network(net: &Path) -> (Option<u64>, Option<u64>) {
    let entries = match std::fs::read_dir(net) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!(path = %net.display(), %error, "network counters cannot be listed");
            return (None, None);
        }
    };
    let mut received = 0_u128;
    let mut sent = 0_u128;
    let mut readable = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!(path = %net.display(), %error, "network interface cannot be listed");
                continue;
            }
        };
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                tracing::debug!(path = %net.display(), "non-UTF-8 network interface ignored");
                continue;
            }
        };
        if name == "lo" {
            continue;
        }
        if !trusted_interface(&name) {
            tracing::debug!(path = %net.display(), "untrusted network interface ignored");
            continue;
        }
        let statistics = net.join(name).join("statistics");
        let (Some(rx), Some(tx)) = (
            read_u64(&statistics.join("rx_bytes")),
            read_u64(&statistics.join("tx_bytes")),
        ) else {
            continue;
        };
        received = received.saturating_add(u128::from(rx));
        sent = sent.saturating_add(u128::from(tx));
        readable = true;
    }
    if !readable {
        return (None, None);
    }
    (
        Some(u64::try_from(received).unwrap_or(u64::MAX)),
        Some(u64::try_from(sent).unwrap_or(u64::MAX)),
    )
}

fn trusted_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'@' | b':')
        })
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod resources_tests;
