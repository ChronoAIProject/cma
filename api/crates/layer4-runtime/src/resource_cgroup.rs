use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::resources::{ResourceFrame, Roots};

#[derive(Default)]
pub(crate) struct Cgroups {
    v2: Option<PathBuf>,
    v1_memory: Option<PathBuf>,
    v1_cpu: Option<PathBuf>,
    v1_cpuacct: Option<PathBuf>,
}

impl Cgroups {
    pub(crate) fn sample(&self, frame: &mut ResourceFrame) {
        if let Some(cgroup) = self.v2.as_ref() {
            frame.memory_bytes_used = read_u64(&cgroup.join("memory.current"));
            frame.memory_bytes_limit = read_limit(&cgroup.join("memory.max"));
            frame.memory_bytes_peak = read_u64(&cgroup.join("memory.peak"));
            frame.stall_permille = read_pressure(&cgroup.join("memory.pressure"));
            let events = read_keyed(&cgroup.join("memory.events"));
            frame.ceiling_hits = events.get("max").copied();
            frame.allocation_failures = events.get("oom").copied();
            frame.kills = events.get("oom_kill").copied();
            let cpu = read_keyed(&cgroup.join("cpu.stat"));
            frame.cpu_usage_usec = cpu.get("usage_usec").copied();
            frame.cpu_millis_limit = read_cpu_limit(&cgroup.join("cpu.max"));
        }
        if let Some(memory) = self.v1_memory.as_ref() {
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
        if let Some(cpuacct) = self.v1_cpuacct.as_ref() {
            frame.cpu_usage_usec = frame
                .cpu_usage_usec
                .or_else(|| read_u64(&cpuacct.join("cpuacct.usage")).map(|value| value / 1_000));
        }
        if let Some(cpu) = self.v1_cpu.as_ref() {
            frame.cpu_millis_limit = frame.cpu_millis_limit.or_else(|| {
                read_v1_cpu_limit(
                    &cpu.join("cpu.cfs_quota_us"),
                    &cpu.join("cpu.cfs_period_us"),
                )
            });
        }
    }
}

pub(crate) fn resolve(roots: &Roots) -> Cgroups {
    let mut result = Cgroups::default();
    let Ok(membership) = std::fs::read_to_string(roots.proc.join("self/cgroup")) else {
        return result;
    };
    let mut v1 = BTreeMap::new();
    for line in membership.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(hierarchy), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if hierarchy == "0" && controllers.is_empty() {
            result.v2 = relative(path).map(|path| roots.cgroup_mount.join(path));
        } else {
            for controller in controllers.split(',').filter(|name| !name.is_empty()) {
                v1.insert(controller.to_string(), path.to_string());
            }
        }
    }
    result.v1_memory = v1.get("memory").and_then(|path| {
        resolve_v1(
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
    result.v1_cpu = v1.get("cpu").and_then(|path| {
        resolve_v1(
            roots,
            "cpu",
            path,
            &["cpu.cfs_quota_us", "cpu.cfs_period_us"],
        )
    });
    result.v1_cpuacct = v1
        .get("cpuacct")
        .and_then(|path| resolve_v1(roots, "cpuacct", path, &["cpuacct.usage"]));
    result
}

fn relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path).strip_prefix("/").ok()?;
    path.components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
        .then(|| path.to_path_buf())
}

fn resolve_v1(
    roots: &Roots,
    controller: &str,
    membership: &str,
    counters: &[&str],
) -> Option<PathBuf> {
    let relative = relative(membership)?;
    let mut missing = Vec::new();
    for mount in controller_mounts(&roots.cgroup_mount, controller) {
        let member = mount.join(&relative);
        match std::fs::metadata(&member) {
            Ok(_) if counters.iter().any(|name| member.join(name).is_file()) => {
                return Some(member)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing.push(mount),
            Err(_) => {}
        }
    }
    missing
        .into_iter()
        .find(|mount| counters.iter().any(|name| mount.join(name).is_file()))
}

fn controller_mounts(root: &Path, controller: &str) -> Vec<PathBuf> {
    let direct = root.join(controller);
    let mut mounts = vec![direct.clone()];
    let Ok(entries) = std::fs::read_dir(root) else {
        return mounts;
    };
    let mut combined = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (entry.path() != direct && name.split(',').any(|name| name == controller))
                .then_some(entry.path())
        })
        .collect::<Vec<_>>();
    combined.sort();
    mounts.extend(combined);
    mounts
}

fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_i64(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_limit(path: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(path).ok()?;
    (raw.trim() != "max")
        .then(|| raw.trim().parse().ok())
        .flatten()
}

const V1_UNLIMITED_MIN: u64 = 0x7fff_ffff_ffff_0000;

fn read_v1_memory_limit(path: &Path) -> Option<u64> {
    read_u64(path).filter(|value| *value < V1_UNLIMITED_MIN)
}

fn read_keyed(path: &Path) -> BTreeMap<String, u64> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (Some(name), Some(value), None) = (fields.next(), fields.next(), fields.next())
            else {
                return None;
            };
            Some((name.to_string(), value.parse().ok()?))
        })
        .collect()
}

fn read_pressure(path: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value = raw.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next() == Some("full"))
            .then(|| fields.find_map(|field| field.strip_prefix("avg10=")))
            .flatten()
    })?;
    let percent: f64 = value.parse().ok()?;
    percent
        .is_finite()
        .then(|| (percent * 10.0).round().clamp(0.0, 1000.0) as u16)
}

fn read_cpu_limit(path: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut fields = raw.split_whitespace();
    let quota = fields.next()?;
    if quota == "max" {
        return None;
    }
    let quota: u128 = quota.parse().ok()?;
    let period: u128 = fields.next()?.parse().ok()?;
    if period == 0 {
        return None;
    }
    Some(u32::try_from(quota.saturating_mul(1_000) / period).unwrap_or(u32::MAX))
}

fn read_v1_cpu_limit(quota: &Path, period: &Path) -> Option<u32> {
    let quota = read_i64(quota)?;
    if quota < 0 {
        return None;
    }
    let period = read_u64(period)?;
    if period == 0 {
        return None;
    }
    let value = u128::try_from(quota).ok()?.checked_mul(1_000)? / u128::from(period);
    u32::try_from(value).ok()
}
