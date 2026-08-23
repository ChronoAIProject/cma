//! Fixture-backed checks for the pod's kernel counter reader.

use super::*;
use serde_json::{json, Value};
use std::time::Duration;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("chrono-resources-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root is created");
        Self { root }
    }

    fn complete(name: &str) -> Self {
        let fixture = Self::new(name);
        fixture.write("proc/self/cgroup", "0::/\n");
        fixture.write("cgroup/memory.current", "402653184\n");
        fixture.write("cgroup/memory.max", "1073741824\n");
        fixture.write("cgroup/memory.peak", "530579456\n");
        fixture.write(
            "cgroup/memory.events",
            "low 0\nhigh 0\nmax 3\noom 1\noom_kill 1\n",
        );
        fixture.write(
            "cgroup/memory.pressure",
            "some avg10=12.50 avg60=3.00 avg300=0.50 total=1234\n\
             full avg10=1.50 avg60=0.40 avg300=0.05 total=567\n",
        );
        fixture.write(
            "cgroup/cpu.stat",
            "usage_usec 12000000\nuser_usec 9000000\nsystem_usec 3000000\n",
        );
        fixture.write("cgroup/cpu.max", "100000 100000\n");
        fixture.write("net/eth0/statistics/rx_bytes", "8388608\n");
        fixture.write("net/eth0/statistics/tx_bytes", "2097152\n");
        fixture.write("net/lo/statistics/rx_bytes", "1024\n");
        fixture.write("net/lo/statistics/tx_bytes", "1024\n");
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("fixture directory is created");
        std::fs::write(path, contents).expect("fixture file is written");
    }

    fn roots(&self) -> Roots {
        Roots {
            proc: self.path("proc"),
            cgroup_mount: self.path("cgroup"),
            net: self.path("net"),
        }
    }

    fn sampler(&self, started_at: Instant) -> Sampler {
        Sampler::new(self.roots(), started_at)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn rendered(frame: &ResourceFrame) -> Value {
    serde_json::to_value(frame).expect("a resource frame serialises")
}

#[test]
fn a_complete_reading_carries_every_number_the_kernel_offered() {
    let fixture = Fixture::complete("complete");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);

    assert_eq!(frame.uptime_ms, 0);
    assert_eq!(frame.cpu_usage_usec, Some(12_000_000));
    assert_eq!(frame.cpu_millis_limit, Some(1000));
    assert_eq!(frame.memory_bytes_used, Some(402_653_184));
    assert_eq!(frame.memory_bytes_limit, Some(1_073_741_824));
    assert_eq!(frame.memory_bytes_peak, Some(530_579_456));
    assert_eq!(frame.stall_permille, Some(15));
    assert_eq!(frame.ceiling_hits, Some(3));
    assert_eq!(frame.allocation_failures, Some(1));
    assert_eq!(frame.kills, Some(1));
    assert_eq!(frame.received_bytes, Some(8_388_608));
    assert_eq!(frame.sent_bytes, Some(2_097_152));
}

#[test]
fn pressure_is_read_from_the_full_line_and_not_the_some_line() {
    let fixture = Fixture::complete("full-pressure");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.stall_permille, Some(15));
    assert_ne!(frame.stall_permille, Some(125));
}

#[test]
fn a_memory_limit_that_reads_max_is_an_absence_and_not_a_ceiling() {
    let fixture = Fixture::complete("unlimited-memory");
    fixture.write("cgroup/memory.max", "max\n");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.memory_bytes_limit, None);
    assert_ne!(frame.memory_bytes_limit, Some(0));
    assert_ne!(frame.memory_bytes_limit, Some(u64::MAX));
}

#[test]
fn a_missing_pressure_file_is_an_absence_and_not_a_calm_reading() {
    let fixture = Fixture::complete("missing-pressure");
    std::fs::remove_file(fixture.path("cgroup/memory.pressure"))
        .expect("pressure fixture is removed");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.stall_permille, None);
    assert_eq!(frame.memory_bytes_used, Some(402_653_184));
    assert_eq!(frame.cpu_usage_usec, Some(12_000_000));
    assert_eq!(frame.received_bytes, Some(8_388_608));
}

#[test]
fn the_processor_counter_is_sent_raw_and_no_rate_is_derived_here() {
    let fixture = Fixture::complete("raw-processor-counter");
    let started = Instant::now();
    let sampler = fixture.sampler(started);
    let first = sampler.sample(started);
    fixture.write("cgroup/cpu.stat", "usage_usec 13000000\n");
    let second = sampler.sample(started + Duration::from_secs(2));

    assert_eq!(first.cpu_usage_usec, Some(12_000_000));
    assert_eq!(second.cpu_usage_usec, Some(13_000_000));
    assert_eq!(second.uptime_ms - first.uptime_ms, 2000);
    assert!(rendered(&first).get("cpuMillisUsed").is_none());
    assert!(rendered(&second).get("cpuMillisUsed").is_none());

    fixture.write("cgroup/cpu.stat", "usage_usec 12000000\n");
    let third = sampler.sample(started + Duration::from_secs(3));
    assert_eq!(third.cpu_usage_usec, Some(12_000_000));
}

#[test]
fn uptime_is_this_supervisor_s_own_monotonic_milliseconds() {
    let fixture = Fixture::complete("monotonic-uptime");
    let started = Instant::now();
    let frame = fixture
        .sampler(started)
        .sample(started + Duration::from_secs(2));
    assert_eq!(frame.uptime_ms, 2000);
}

#[test]
fn a_missing_cpu_stat_leaves_the_processor_counter_absent_rather_than_zero() {
    let fixture = Fixture::complete("missing-processor-counter");
    std::fs::remove_file(fixture.path("cgroup/cpu.stat")).expect("processor fixture is removed");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.cpu_usage_usec, None);
    assert!(rendered(&frame).get("cpuUsageUsec").is_none());
}

#[test]
fn a_quota_that_reads_max_is_a_reading_with_no_denominator() {
    let fixture = Fixture::complete("unlimited-processor");
    fixture.write("cgroup/cpu.max", "max 100000\n");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.cpu_millis_limit, None);
    assert_eq!(frame.cpu_usage_usec, Some(12_000_000));
}

#[test]
fn memory_events_are_read_by_name_and_not_by_position() {
    let fixture = Fixture::complete("reordered-events");
    let now = Instant::now();
    let sampler = fixture.sampler(now);
    let before = sampler.sample(now);
    fixture.write(
        "cgroup/memory.events",
        "oom_group_kill 0\noom_kill 1\nhigh 0\noom 1\nlow 0\nmax 3\n",
    );
    let after = sampler.sample(now);
    assert_eq!(after.ceiling_hits, before.ceiling_hits);
    assert_eq!(after.allocation_failures, before.allocation_failures);
    assert_eq!(after.kills, before.kills);
}

#[test]
fn traffic_is_totalled_across_every_interface_that_is_not_loopback() {
    let fixture = Fixture::complete("totalled-traffic");
    let now = Instant::now();
    let sampler = fixture.sampler(now);
    let first = sampler.sample(now);
    assert_eq!(first.received_bytes, Some(8_388_608));
    assert_eq!(first.sent_bytes, Some(2_097_152));

    fixture.write("net/ens5/statistics/rx_bytes", "1000\n");
    fixture.write("net/ens5/statistics/tx_bytes", "2000\n");
    let second = sampler.sample(now);
    assert_eq!(second.received_bytes, Some(8_389_608));
    assert_eq!(second.sent_bytes, Some(2_099_152));
}

#[test]
fn an_interface_whose_counters_cannot_be_read_contributes_nothing_rather_than_zero() {
    let fixture = Fixture::complete("unreadable-interface");
    fixture.write("net/ens5/statistics/rx_bytes", "1000\n");
    fixture.write("net/ens5/statistics/tx_bytes", "2000\n");
    fixture.write("net/broken0/statistics/rx_bytes", "9000\n");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.received_bytes, Some(8_389_608));
    assert_eq!(frame.sent_bytes, Some(2_099_152));
}

#[test]
fn an_interface_directory_this_pod_would_not_trust_is_never_read() {
    let fixture = Fixture::complete("untrusted-interface");
    fixture.write("net/untrusted+device/statistics/rx_bytes", "9000\n");
    fixture.write("net/untrusted+device/statistics/tx_bytes", "9000\n");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.received_bytes, Some(8_388_608));
    assert_eq!(frame.sent_bytes, Some(2_097_152));
}

#[test]
fn a_pod_with_no_interfaces_at_all_reports_no_traffic_rather_than_none_moved() {
    let fixture = Fixture::complete("no-interfaces");
    std::fs::remove_dir_all(fixture.path("net")).expect("network fixture is removed");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.received_bytes, None);
    assert_eq!(frame.sent_bytes, None);
    assert_ne!(frame.received_bytes, Some(0));
    assert!(rendered(&frame).get("receivedBytes").is_none());
    assert!(rendered(&frame).get("sentBytes").is_none());
}

#[test]
fn a_pod_whose_kernel_exposes_nothing_says_nothing_rather_than_zero() {
    let fixture = Fixture::new("empty-kernel-interfaces");
    fixture.write("proc/self/cgroup", "0::/\n");
    std::fs::create_dir_all(fixture.path("cgroup")).expect("empty cgroup fixture is created");
    let started = Instant::now();
    let frame = fixture
        .sampler(started)
        .sample(started + Duration::from_millis(42));
    assert_eq!(rendered(&frame), json!({ "uptimeMs": 42 }));
}

#[test]
fn a_cgroup_path_below_the_mount_point_is_resolved_and_not_assumed() {
    let fixture = Fixture::new("nested-cgroup");
    fixture.write("proc/self/cgroup", "0::/kubepods/pod-x/container-y\n");
    fixture.write(
        "cgroup/kubepods/pod-x/container-y/memory.current",
        "402653184\n",
    );
    fixture.write(
        "cgroup/kubepods/pod-x/container-y/cpu.stat",
        "usage_usec 12000000\n",
    );
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.memory_bytes_used, Some(402_653_184));
    assert_eq!(frame.cpu_usage_usec, Some(12_000_000));
}

#[test]
fn a_kernel_without_cgroup_v2_reports_no_cgroup_reading_at_all() {
    let fixture = Fixture::complete("cgroup-v1");
    fixture.write("proc/self/cgroup", "5:memory:/kubepods/pod-x\n");
    let now = Instant::now();
    let frame = fixture.sampler(now).sample(now);
    assert_eq!(frame.cpu_usage_usec, None);
    assert_eq!(frame.cpu_millis_limit, None);
    assert_eq!(frame.memory_bytes_used, None);
    assert_eq!(frame.memory_bytes_limit, None);
    assert_eq!(frame.memory_bytes_peak, None);
    assert_eq!(frame.stall_permille, None);
    assert_eq!(frame.ceiling_hits, None);
    assert_eq!(frame.allocation_failures, None);
    assert_eq!(frame.kills, None);
    assert_eq!(frame.received_bytes, Some(8_388_608));
    assert_eq!(frame.sent_bytes, Some(2_097_152));
}

#[test]
fn the_field_names_are_contract() {
    let fixture = Fixture::complete("field-contract");
    let started = Instant::now();
    let frame = fixture
        .sampler(started)
        .sample(started + Duration::from_secs(2));
    assert_eq!(
        rendered(&frame),
        json!({
            "uptimeMs": 2000,
            "cpuUsageUsec": 12_000_000,
            "cpuMillisLimit": 1000,
            "memoryBytesUsed": 402_653_184,
            "memoryBytesLimit": 1_073_741_824_u64,
            "memoryBytesPeak": 530_579_456,
            "stallPermille": 15,
            "ceilingHits": 3,
            "allocationFailures": 1,
            "kills": 1,
            "receivedBytes": 8_388_608,
            "sentBytes": 2_097_152,
        })
    );
}

#[test]
fn a_sample_carries_no_string_at_all() {
    let fixture = Fixture::complete("notaninterface-root");
    std::fs::rename(
        fixture.path("net/eth0"),
        fixture.path("net/notaninterface0"),
    )
    .expect("the interface fixture is renamed");
    let now = Instant::now();
    let value = rendered(&fixture.sampler(now).sample(now));
    let encoded = serde_json::to_string(&value).expect("the frame renders");
    assert!(!encoded.contains("notaninterface"));

    fn assert_numeric_leaves(value: &Value) {
        match value {
            Value::Object(fields) => fields.values().for_each(assert_numeric_leaves),
            Value::Array(values) => values.iter().for_each(assert_numeric_leaves),
            Value::Number(_) => {}
            other => panic!("a resource frame leaf was not a number: {other:?}"),
        }
    }
    assert_numeric_leaves(&value);
}
