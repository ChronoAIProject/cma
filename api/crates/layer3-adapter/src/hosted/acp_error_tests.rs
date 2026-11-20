use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    sizing, AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope,
};
use futures_util::FutureExt;

#[test]
fn isolated_rpc_error_formatting_precedes_construction_and_retains_the_result() {
    const TEST: &str = "hosted::acp::calls::error_tests::isolated_rpc_error_formatting_precedes_construction_and_retains_the_result";
    if std::env::var_os("CMA_RPC_DIAGNOSTIC_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_RPC_DIAGNOSTIC_ALLOCATION_CHILD", "1")
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
    let root = IngressBudget::new(8 * 1024 * 1024);
    let plan = |bytes| AllocationPlan::checked("RPC diagnostic fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(1024 * 1024), plan(1024 * 1024)).unwrap(),
    )
    .unwrap();
    // Shared process pools have their own lifetime; keep their lazy lock setup
    // outside this per-operation diagnostic allocation measurement.
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(policy.admit(class, plan(1)).unwrap());
    }
    for detail in [
        serde_json::json!({"message":"x".repeat(65_536)}),
        serde_json::json!({"message":"\u{0}\n\\\"é".repeat(8192), "number":1e-300}),
        serde_json::json!({"nested":[null, true, [1,2,3], {"empty":{}, "items":[]}]}),
    ] {
        let expected = format!("method: {detail}");
        let frame = JsonFrame::legacy(serde_json::json!({"error":detail}));
        let scope = IngressWorkScope::new(
            policy
                .admit(IngressClass::Data, plan(sizing::shared_owner_bytes::<()>()))
                .unwrap(),
        );
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| {
            scope
                .initialize_owned(|| async { rpc_result("method", frame) })
                .resolve_admitted_result(&policy, IngressClass::Data)
                .now_or_never()
                .unwrap()
                .unwrap_err()
        });
        let charged = scope.requested_bytes();
        assert!(
            probe.peak() <= charged,
            "RPC diagnostic peak={} admission={charged}",
            probe.peak()
        );
        assert!(
            matches!(error.as_adapter(), Some(AdapterError::Rejected(detail)) if detail==&expected)
        );
        let cloned = probe.track(|| error.clone());
        drop((scope, error));
        assert!(probe.live() >= expected.len());
        assert_eq!(root.reserved_work_bytes(), charged);
        eprintln!(
            "rpc_diagnostic requested_peak={} retained={} admitted={charged}",
            probe.peak(),
            probe.live()
        );
        probe.track(|| drop(cloned));
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }

    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        let frame = JsonFrame::legacy(serde_json::json!({"error":{"message":"x".repeat(65_536)}}));
        let scope = IngressWorkScope::new(
            policy
                .admit(class, plan(sizing::shared_owner_bytes::<()>()))
                .unwrap(),
        );
        let mut probe = AllocationProbe::new();
        let result = scope
            .initialize_owned(|| async {
                let available = policy.budget(class).available_bytes();
                let _pressure = policy
                    .admit(class, plan(available))
                    .unwrap()
                    .initialize(|| vec![0u8; available - 1024]);
                probe.track(|| rpc_result("method", frame))
            })
            .resolve_admitted_result(&policy, class)
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert_eq!(probe.allocated(), 0, "refuse before allocating RPC text");
        assert!(
            matches!(result.as_adapter(), Some(AdapterError::AtCapacity(detail)) if detail.is_empty())
        );
        assert!(
            result.requested_bytes() > 0,
            "refusal uses a pre-admitted result owner"
        );
        eprintln!(
            "rpc_diagnostic_pressure class={class:?} text_allocations={}",
            probe.allocated()
        );
        drop((result, scope, probe));
        root.assert_idle();
    }

    let frame = JsonFrame::legacy(serde_json::json!({"error":{"message":"x".repeat(65_536)}}));
    let mut probe = AllocationProbe::new();
    let legacy = probe.track(|| rpc_result("method", frame)).unwrap_err();
    assert!(
        probe.allocated() > 65_536,
        "legacy control must expose formatter allocations"
    );
    eprintln!(
        "legacy_rpc_diagnostic peak={} retained={}",
        probe.peak(),
        probe.live()
    );
    drop(legacy);
    assert_eq!(probe.live(), 0);
}
