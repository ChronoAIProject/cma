use std::path::PathBuf;
use std::time::Instant;

use layer3_adapter::ResourceReading;
use serde::Serialize;

pub(crate) struct Roots {
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceFrame {
    pub uptime_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_millis_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_peak: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_permille: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_hits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocation_failures: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kills: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
}

impl ResourceFrame {
    #[cfg(test)]
    pub(crate) fn has_observations(&self) -> bool {
        self.cpu_usage_usec.is_some()
            || self.cpu_millis_limit.is_some()
            || self.memory_bytes_used.is_some()
            || self.memory_bytes_limit.is_some()
            || self.memory_bytes_peak.is_some()
            || self.stall_permille.is_some()
            || self.ceiling_hits.is_some()
            || self.allocation_failures.is_some()
            || self.kills.is_some()
            || self.received_bytes.is_some()
            || self.sent_bytes.is_some()
    }
}

impl From<ResourceFrame> for ResourceReading {
    fn from(frame: ResourceFrame) -> Self {
        Self {
            uptime_ms: frame.uptime_ms,
            cpu_usage_usec: frame.cpu_usage_usec,
            cpu_millis_limit: frame.cpu_millis_limit,
            memory_bytes_used: frame.memory_bytes_used,
            memory_bytes_limit: frame.memory_bytes_limit,
            memory_bytes_peak: frame.memory_bytes_peak,
            stall_permille: frame.stall_permille,
            ceiling_hits: frame.ceiling_hits,
            allocation_failures: frame.allocation_failures,
            kills: frame.kills,
            received_bytes: frame.received_bytes,
            sent_bytes: frame.sent_bytes,
        }
    }
}

pub(crate) struct Sampler {
    roots: Roots,
    cgroups: crate::resource_cgroup::Cgroups,
    started_at: Instant,
}

impl Sampler {
    pub(crate) fn new(roots: Roots, started_at: Instant) -> Self {
        let cgroups = crate::resource_cgroup::resolve(&roots);
        Self {
            roots,
            cgroups,
            started_at,
        }
    }

    pub(crate) fn sample(&self, now: Instant) -> ResourceFrame {
        let uptime_ms = u64::try_from(now.saturating_duration_since(self.started_at).as_millis())
            .unwrap_or(u64::MAX);
        let (received_bytes, sent_bytes) =
            crate::resource_network::read(&self.roots.net, &self.roots.proc.join("net/dev"));
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
        self.cgroups.sample(&mut frame);
        frame
    }
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
