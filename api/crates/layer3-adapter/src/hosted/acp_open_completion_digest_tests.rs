use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(16 * 1024 * 1024);
    let plan = AllocationPlan::new("digest fixture", &[1024 * 1024]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), plan, plan).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    // Semaphore release lazily initializes platform mutex backing storage.
    // The shared pool exists before the digest payload measurement begins.
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(
            policy
                .reserve(
                    class,
                    AllocationPlan::new("pool initialization", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    (root, policy)
}

#[test]
fn digest_matches_all_pinned_native_vectors() {
    let vectors: Vec<Value> =
        serde_json::from_str(include_str!("../codex/open_completion_vectors.json")).unwrap();
    assert_eq!(vectors.len(), 11);
    let (root, policy) = policy();
    for vector in vectors {
        let input = &vector["input"];
        let servers = input["mcpServers"].as_array().cloned().unwrap_or_default();
        let snapshot = json!({"cwd":input["cwd"], "mcpServers":servers});
        assert_eq!(canonical_json(&snapshot).unwrap(), vector["canonical"]);
        assert_eq!(
            binding_digest(input["cwd"].as_str().unwrap(), &servers).unwrap(),
            vector["bindingDigest"],
            "{}",
            vector["name"]
        );
        let digest =
            binding_digest_with_policy(input["cwd"].as_str().unwrap(), &servers, Some(&policy))
                .unwrap();
        assert_eq!(&*digest, vector["bindingDigest"].as_str().unwrap());
        assert!(root.available_bytes() < root.capacity_bytes());
        drop(digest);
        root.assert_idle();
    }
}

#[test]
fn unsupported_numeric_and_oversize_snapshots_are_refused() {
    assert!(binding_digest("relative", &[]).is_err());
    assert!(binding_digest("/home", &[json!({"value": 1.0})]).is_err());
    assert!(binding_digest("/home", &[json!("x".repeat(65536))]).is_err());
    assert!(binding_digest("/home", &vec![json!({}); 33]).is_err());
    assert!(!uuid4("00000000-0000-0000-0000-000000000000"));
}

#[test]
fn admitted_digest_refuses_invalid_input_and_releases_partial_reservations() {
    let (root, policy) = policy();
    for (cwd, servers) in [
        ("relative", vec![]),
        ("/home", vec![json!({"value": 1.0})]),
        ("/home", vec![json!("x".repeat(65536))]),
        ("/home", vec![json!({}); 33]),
    ] {
        let admitted = binding_digest_with_policy(cwd, &servers, Some(&policy))
            .err()
            .unwrap();
        let legacy = binding_digest(cwd, &servers).unwrap_err();
        assert!(matches!(admitted, AdapterError::Unavailable(_)));
        assert_eq!(admitted.to_string(), legacy.to_string());
        root.assert_idle();
    }
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let available = policy.budget(class).available_bytes();
        let blocker = policy
            .reserve(
                class,
                AllocationPlan::new("digest pressure", &[available]).unwrap(),
            )
            .unwrap()
            .initialize(|| vec![0u8; available - 512]);
        let remaining = root.available_bytes();
        assert!(matches!(
            binding_digest_with_policy("/home", &[json!({"name":"tool"})], Some(&policy)),
            Err(AdapterError::AtCapacity(_))
        ));
        assert_eq!(root.available_bytes(), remaining);
        drop(blocker);
        root.assert_idle();
    }
}

#[test]
fn isolated_digest_allocation_contract() {
    use crate::allocation_probe::AllocationProbe;
    const TEST: &str =
        "hosted::acp_open_completion::digest_tests::isolated_digest_allocation_contract";
    if std::env::var_os("CMA_DIGEST_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_DIGEST_ALLOCATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        println!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let object = (0..1024)
        .map(|index| (format!("key-{index:04}"), json!({"nested":["value",false]})))
        .collect::<serde_json::Map<_, _>>();
    let servers = [Value::Object(object)];
    let (root, policy) = policy();
    let retained = 67 + crate::ingress_budget::sizing::shared_owner_bytes::<String>();
    let peak_bound = retained + sort_keys(&servers[0], 0).unwrap() * size_of::<&String>();
    let mut baseline = AllocationProbe::new();
    let expected = baseline
        .track(|| binding_digest("/fixture", &servers))
        .unwrap();
    assert!(
        baseline.peak() > peak_bound,
        "legacy string copies must exceed the sort-only plan"
    );
    eprintln!("digest baseline_requested_peak={}", baseline.peak());
    drop(baseline);
    let mut probe = AllocationProbe::new();
    let digest = probe
        .track(|| binding_digest_with_policy("/fixture", &servers, Some(&policy)))
        .unwrap();
    assert_eq!(&*digest, expected);
    assert!(
        probe.peak() <= peak_bound,
        "digest peak {} > {peak_bound}",
        probe.peak()
    );
    assert!(
        probe.live() <= retained,
        "digest retained {} > {retained}",
        probe.live()
    );
    assert_eq!(root.reserved_work_bytes(), retained);
    eprintln!("digest requested_peak={} requested_retained={} admitted_peak={peak_bound} admitted_retained={retained}", probe.peak(), probe.live());
    let BindingDigest::Admitted(digest) = digest else {
        panic!("digest escaped its owner")
    };
    let copy = digest.clone();
    drop(digest);
    assert_eq!(root.reserved_work_bytes(), retained);
    drop(copy);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}
