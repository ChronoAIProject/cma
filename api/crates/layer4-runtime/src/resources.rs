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
    /// Cumulative processor time reported by the active cgroup hierarchy.
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

impl ResourceFrame {
    /// Whether this frame carries an observed resource rather than only time.
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

#[derive(Default)]
struct Cgroups {
    v2: Option<PathBuf>,
    v1_memory: Option<PathBuf>,
    v1_cpu: Option<PathBuf>,
    v1_cpuacct: Option<PathBuf>,
}

impl Cgroups {
    fn has_v1(&self) -> bool {
        self.v1_memory.is_some() || self.v1_cpu.is_some() || self.v1_cpuacct.is_some()
    }
}

/// A stateless reader for this container's kernel counters.
pub struct Sampler {
    roots: Roots,
    /// This container's cgroup directories, where any were resolvable.
    cgroups: Cgroups,
    /// When this sampler was built, which is when the supervisor started.
    started_at: Instant,
}

impl Sampler {
    /// Resolve this pod's cgroup once and retain the monotonic start time.
    pub fn new(roots: Roots, started_at: Instant) -> Self {
        let cgroups = resolve_cgroups(&roots);
        if let Some(path) = cgroups.v2.as_ref() {
            tracing::info!(cgroup = %path.display(), "resource sampler resolved this pod's cgroup v2 directory");
        } else if cgroups.has_v1() {
            tracing::info!(
                memory = ?cgroups.v1_memory,
                cpu = ?cgroups.v1_cpu,
                cpuacct = ?cgroups.v1_cpuacct,
                "resource sampler resolved this pod's cgroup v1 controllers"
            );
        } else {
            tracing::warn!("this kernel exposes no readable cgroup hierarchy to this pod");
            tracing::info!("resource sampler started without a resolved cgroup");
        }
        Self {
            roots,
            cgroups,
            started_at,
        }
    }

    /// Read one independent snapshot without deriving rates or retaining it.
    pub fn sample(&self, now: Instant) -> ResourceFrame {
        let uptime_ms = u64::try_from(now.saturating_duration_since(self.started_at).as_millis())
            .unwrap_or(u64::MAX);
        let (received_bytes, sent_bytes) =
            read_network(&self.roots.net, &self.roots.proc.join("net/dev"));
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

        if let Some(cgroup) = self.cgroups.v2.as_ref() {
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
        }

        if let Some(memory) = self.cgroups.v1_memory.as_ref() {
            frame.memory_bytes_used = frame
                .memory_bytes_used
                .or_else(|| read_u64(&memory.join("memory.usage_in_bytes")));
            frame.memory_bytes_limit = frame
                .memory_bytes_limit
                .or_else(|| read_v1_memory_limit(&memory.join("memory.limit_in_bytes")));
            frame.memory_bytes_peak = frame
                .memory_bytes_peak
                .or_else(|| read_u64(&memory.join("memory.max_usage_in_bytes")));
        }
        if let Some(cpuacct) = self.cgroups.v1_cpuacct.as_ref() {
            frame.cpu_usage_usec = frame
                .cpu_usage_usec
                .or_else(|| read_cpuacct_usage(&cpuacct.join("cpuacct.usage")));
        }
        if let Some(cpu) = self.cgroups.v1_cpu.as_ref() {
            frame.cpu_millis_limit = frame.cpu_millis_limit.or_else(|| {
                read_v1_cpu_limit(
                    &cpu.join("cpu.cfs_quota_us"),
                    &cpu.join("cpu.cfs_period_us"),
                )
            });
        }
        frame
    }
}

fn resolve_cgroups(roots: &Roots) -> Cgroups {
    let mut cgroups = Cgroups::default();
    let Ok(membership) = std::fs::read_to_string(roots.proc.join("self/cgroup")) else {
        return cgroups;
    };
    let mut v1 = BTreeMap::<String, String>::new();
    for line in membership.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(hierarchy), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            tracing::debug!(line, "malformed cgroup membership ignored");
            continue;
        };
        if hierarchy == "0" && controllers.is_empty() {
            cgroups.v2 = cgroup_relative(path).map(|path| roots.cgroup_mount.join(path));
            continue;
        }
        for controller in controllers.split(',').filter(|name| !name.is_empty()) {
            v1.insert(controller.to_string(), path.to_string());
        }
    }

    cgroups.v1_memory = v1.get("memory").and_then(|path| {
        resolve_v1_controller(
            roots,
            "memory",
            path,
            &[
                "memory.usage_in_bytes",
                "memory.limit_in_bytes",
                "memory.max_usage_in_bytes",
            ],
        )
    });
    cgroups.v1_cpu = v1.get("cpu").and_then(|path| {
        resolve_v1_controller(
            roots,
            "cpu",
            path,
            &["cpu.cfs_quota_us", "cpu.cfs_period_us"],
        )
    });
    cgroups.v1_cpuacct = v1
        .get("cpuacct")
        .and_then(|path| resolve_v1_controller(roots, "cpuacct", path, &["cpuacct.usage"]));
    cgroups
}

fn cgroup_relative(path: &str) -> Option<PathBuf> {
    let relative = Path::new(path).strip_prefix("/").ok()?;
    relative
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
        .then(|| relative.to_path_buf())
}

fn resolve_v1_controller(
    roots: &Roots,
    controller: &str,
    membership: &str,
    counter_names: &[&str],
) -> Option<PathBuf> {
    let relative = cgroup_relative(membership)?;
    let mut mount_root_fallbacks = Vec::new();
    for mount in controller_mounts(&roots.cgroup_mount, controller) {
        let member = mount.join(&relative);
        match std::fs::metadata(&member) {
            Ok(_) => {
                if counter_names
                    .iter()
                    .any(|counter| member.join(counter).is_file())
                {
                    return Some(member);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                mount_root_fallbacks.push(mount);
            }
            Err(error) => {
                tracing::debug!(path = %member.display(), %error, "cgroup membership cannot be inspected");
            }
        }
    }

    // Some gVisor guests name a membership below the mount while exposing the
    // guest's counters at the mount root. Consider that shape only after every
    // actual membership directory has had a chance to match.
    mount_root_fallbacks.into_iter().find(|mount| {
        counter_names
            .iter()
            .any(|counter| mount.join(counter).is_file())
    })
}

fn controller_mounts(root: &Path, controller: &str) -> Vec<PathBuf> {
    let direct = root.join(controller);
    let mut mounts = vec![direct.clone()];
    let Ok(entries) = std::fs::read_dir(root) else {
        return mounts;
    };
    let mut combined = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (name.split(',').any(|name| name == controller) && entry.path() != direct)
                .then_some(entry.path())
        })
        .collect::<Vec<_>>();
    combined.sort();
    mounts.extend(combined);
    mounts
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

// Linux cgroup v1 represents "unlimited" as LONG_MAX, commonly rounded down
// to a page boundary. Values in this range are not useful finite ceilings.
const CGROUP_V1_UNLIMITED_MIN: u64 = 0x7fff_ffff_ffff_0000;

fn read_v1_memory_limit(path: &Path) -> Option<u64> {
    let value = read_u64(path)?;
    if value >= CGROUP_V1_UNLIMITED_MIN {
        tracing::debug!(path = %path.display(), value, "cgroup v1 memory limit is unlimited");
        return None;
    }
    Some(value)
}

fn read_cpuacct_usage(path: &Path) -> Option<u64> {
    read_u64(path).map(|nanoseconds| nanoseconds / 1_000)
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

fn read_v1_cpu_limit(quota: &Path, period: &Path) -> Option<u32> {
    let quota_usec = read_i64(quota)?;
    if quota_usec < 0 {
        tracing::debug!(path = %quota.display(), quota_usec, "cgroup v1 processor quota is unlimited or invalid");
        return None;
    }
    let period_usec = read_u64(period)?;
    if period_usec == 0 {
        tracing::debug!(path = %period.display(), "cgroup v1 processor period is zero");
        return None;
    }
    let Some(scaled_quota) = u128::try_from(quota_usec)
        .ok()
        .and_then(|quota| quota.checked_mul(1_000))
    else {
        tracing::debug!(path = %quota.display(), quota_usec, "cgroup v1 processor quota overflows its calculation");
        return None;
    };
    let millis = scaled_quota / u128::from(period_usec);
    match u32::try_from(millis) {
        Ok(millis) => Some(millis),
        Err(error) => {
            tracing::debug!(path = %quota.display(), quota_usec, period_usec, %error, "cgroup v1 processor limit does not fit the wire");
            None
        }
    }
}

fn read_i64(path: &Path) -> Option<i64> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "signed resource counter cannot be read");
            return None;
        }
    };
    match raw.trim().parse() {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "signed resource counter is not an integer");
            None
        }
    }
}

fn read_network(net: &Path, proc_net_dev: &Path) -> (Option<u64>, Option<u64>) {
    let sysfs = read_network_sysfs(net);
    if sysfs.0.is_some() {
        return sysfs;
    }
    read_network_proc(proc_net_dev)
}

fn read_network_sysfs(net: &Path) -> (Option<u64>, Option<u64>) {
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

fn read_network_proc(path: &Path) -> (Option<u64>, Option<u64>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "proc network counters cannot be read");
            return (None, None);
        }
    };
    let mut received = 0_u128;
    let mut sent = 0_u128;
    let mut readable = false;
    for line in raw.lines() {
        let Some((name, counters)) = line.rsplit_once(':') else {
            continue;
        };
        let name = name.trim();
        if name == "lo" {
            continue;
        }
        if !trusted_interface(name) {
            tracing::debug!(path = %path.display(), "untrusted proc network interface ignored");
            continue;
        }
        let counters = counters.split_whitespace().collect::<Vec<_>>();
        if counters.len() != 16 {
            tracing::debug!(path = %path.display(), interface = name, columns = counters.len(), "proc network row does not have the documented column count");
            continue;
        }
        let counters = counters
            .into_iter()
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>();
        let Ok(counters) = counters else {
            tracing::debug!(path = %path.display(), interface = name, "malformed proc network counter ignored");
            continue;
        };
        let rx = counters[0];
        let tx = counters[8];
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
