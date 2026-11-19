use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(8 * 1024 * 1024);
    let floor = AllocationPlan::new("tool fixture floor", &[1024 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(
        policy
            .reserve(
                IngressClass::Data,
                AllocationPlan::new("warm", &[1]).unwrap(),
            )
            .unwrap(),
    );
    (root, policy)
}

fn transports() -> [Option<ToolTransport>; 3] {
    let request = crate::AgentOpenRequest::new(
        "synthetic-open",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("synthetic-agent"),
            ..Default::default()
        },
        None,
    );
    let runtime = crate::RuntimeHandle("synthetic-runtime".into());
    let access = HostedAccessToolBinding::new(
        "https://example.test",
        crate::RuntimeCredential::new(format!("cma_at_{}", "a".repeat(64))),
        runtime.clone(),
        &request,
        1,
        None,
    )
    .unwrap();
    let provider = HostedProviderToolBinding::new(
        "a".repeat(64),
        runtime,
        request,
        1,
        crate::ToolProxyRequest {
            sandbox_id: "synthetic-sandbox".into(),
            generation: 1,
            endpoint: "https://gateway.example.test/mcp".into(),
            credential_generation: 1,
        },
        None,
    )
    .unwrap();
    [
        None,
        Some(ToolTransport::Access(access)),
        Some(ToolTransport::Provider(Box::new(provider))),
    ]
}

#[test]
fn isolated_managed_tool_copy_contract() {
    const TEST: &str =
        "hosted::managed_context::transport::ingress_tests::isolated_managed_tool_copy_contract";
    if std::env::var_os("CMA_MANAGED_TOOL_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_MANAGED_TOOL_ALLOCATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for (index, transport) in transports().into_iter().enumerate() {
        let tools = ["cma_access", "cmaeg"].map(|name| serde_json::json!({
            "name":name,"type":"http","url":format!("https://example.test/{}", "x".repeat(65_536)),"headers":[]
        })).to_vec();
        let (binding, _) = super::super::writer_tests::binding_with_tools(0, tools);
        *binding.0.installed_transport.write().unwrap() = transport;
        let expected = serde_json::Value::Array(binding.tools());
        let (root, policy) = policy();
        let mut probe = AllocationProbe::new();
        let frame = probe.track(|| binding.tools_frame(Some(&policy))).unwrap();
        let charged = root.capacity_bytes() - root.available_bytes();
        assert_eq!(*frame, expected);
        assert!(probe.peak() <= charged);
        eprintln!(
            "managed_tools variant={index} requested_peak={} retained={} admitted={charged}",
            probe.peak(),
            probe.live()
        );
        let clone = frame.shared().unwrap();
        drop(frame);
        assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
        assert!(probe.live() > 0);
        drop(clone);
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
        drop(probe);
        let available = policy.budget(IngressClass::Data).available_bytes();
        let blocker = policy
            .reserve(
                IngressClass::Data,
                AllocationPlan::new("tool pressure", &[available]).unwrap(),
            )
            .unwrap()
            .initialize(|| vec![0u8; available - 512]);
        let mut probe = AllocationProbe::new();
        let legacy = probe.track(|| binding.tools());
        assert!(probe.peak() >= 65_536);
        eprintln!(
            "managed_tools variant={index} legacy_unadmitted_requested_peak={}",
            probe.peak()
        );
        drop(legacy);
        assert_eq!(probe.live(), 0);
        drop(probe);
        let mut probe = AllocationProbe::new();
        let refused = probe.track(|| binding.tools_frame(Some(&policy)));
        assert!(matches!(refused, Err(AdapterError::AtCapacity(_))));
        assert!(probe.peak() <= "ingress allocation capacity exhausted".len());
        drop((refused, blocker));
        assert_eq!(probe.live(), 0);
        drop(probe);
        assert_eq!(*binding.tools_frame(None).unwrap(), expected);
    }
}

#[test]
fn admitted_tool_replacement_preserves_ambiguity_refusal_without_partial_admission() {
    for count in [0, 2] {
        let tools = vec![serde_json::json!({"name":"cma_access"}); count];
        let (binding, _) = super::super::writer_tests::binding_with_tools(0, tools);
        *binding.0.installed_transport.write().unwrap() = transports().into_iter().nth(1).unwrap();
        let (root, policy) = policy();
        let legacy = binding.tools_for_transport(None).unwrap_err();
        let admitted = binding.tools_frame(Some(&policy)).unwrap_err();
        assert_eq!(admitted.to_string(), legacy.to_string());
        assert!(matches!(admitted, AdapterError::Unsupported(_)));
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[test]
fn explicit_transport_replaces_the_matching_tool_before_the_installed_transport() {
    for explicit in transports().into_iter().skip(1) {
        let tools = ["cma_access", "cmaeg"]
            .map(|name| serde_json::json!({"name":name}))
            .to_vec();
        let (binding, _) = super::super::writer_tests::binding_with_tools(0, tools);
        for installed in transports() {
            *binding.0.installed_transport.write().unwrap() = installed;
            let expected =
                serde_json::Value::Array(binding.tools_for_transport(explicit.as_ref()).unwrap());
            let (root, policy) = policy();
            let frame = binding
                .tools_frame_for_transport(explicit.as_ref(), Some(&policy))
                .unwrap();
            assert_eq!(*frame, expected);
            let clone = frame.shared().unwrap();
            let held = root.available_bytes();
            drop(frame);
            assert_eq!(root.available_bytes(), held);
            assert_eq!(*clone, expected);
            drop(clone);
            assert_eq!(root.available_bytes(), root.capacity_bytes());
            assert_eq!(
                *binding
                    .tools_frame_for_transport(explicit.as_ref(), None)
                    .unwrap(),
                expected
            );
        }
    }
}
