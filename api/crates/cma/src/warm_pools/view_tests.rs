use std::sync::{atomic::AtomicBool, Arc};

use layer3_adapter::agent_sandbox::NativePoolStatus;

use super::{target, Provider};

#[test]
fn readiness_requires_original_binding_and_observed_current_target() {
    for machine in [false, true] {
        let mut original = target();
        original.machine = machine;
        original.document_schema_version = if machine { 2 } else { 1 };
        let provider = Provider {
            calls: Arc::default(),
            original: original.plan.clone(),
            refuse_replaced_uid: AtomicBool::new(false),
            retired: AtomicBool::new(false),
        };
        let mut live = NativePoolStatus {
            binding: provider.binding(),
            desired_replicas: 2,
            ready_replicas: 2,
            observed: true,
        };
        original.binding = Some(live.binding.clone());
        original.applied_revision = original.revision;

        for (desired, ready, status) in [
            (2, 2, "ready"),
            (2, 1, "pending"),
            (0, 2, "pending"),
            (0, 1, "pending"),
            (0, 0, "disabled"),
        ] {
            original.target_replicas = desired;
            live.desired_replicas = desired;
            live.ready_replicas = ready;
            let view = original.view(Some(&live), true);
            assert_eq!(
                view.status, status,
                "machine={machine}, target={desired}, ready={ready}"
            );
            assert_eq!(view.target_replicas, desired);
            assert_eq!(view.ready_replicas, Some(ready));
            assert!(view.observed);
            assert_eq!(view.machine, machine);
        }

        original.target_replicas = 2;
        live.desired_replicas = 2;
        live.ready_replicas = 2;
        original.applied_revision = 0;
        let unapplied = original.view(Some(&live), true);
        assert_eq!(unapplied.status, "pending");
        assert_eq!(unapplied.ready_replicas, Some(2));
        original.applied_revision = original.revision;

        live.binding.uid = "replacement".into();
        let replaced = original.view(Some(&live), true);
        assert_eq!(replaced.status, "pending");
        assert!(!replaced.observed);
        assert_eq!(replaced.ready_replicas, None);
        live.binding = original.binding.clone().unwrap();

        live.observed = false;
        let stale = original.view(Some(&live), true);
        assert_eq!(stale.status, "pending");
        assert!(!stale.observed);
        assert_eq!(stale.ready_replicas, None);
        live.observed = true;

        live.desired_replicas = 3;
        let different_target = original.view(Some(&live), true);
        assert_eq!(different_target.status, "pending");
        assert!(!different_target.observed);
        assert_eq!(different_target.ready_replicas, None);
        assert_eq!(different_target.provider_target_replicas, Some(3));
        assert_eq!(different_target.retirement_status, None);

        let unavailable = original.view(None, false);
        assert_eq!(unavailable.status, "unavailable");
        assert!(!unavailable.observed);
        assert_eq!(unavailable.ready_replicas, None);
    }
}

#[test]
fn desktop_retirement_reports_provider_target_until_drain_is_confirmed() {
    let mut desktop = target();
    desktop.machine = true;
    desktop.retiring = true;
    desktop.target_replicas = 0;
    let provider = Provider {
        calls: Arc::default(),
        original: desktop.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let mut live = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: 2,
        ready_replicas: 2,
        observed: true,
    };
    desktop.binding = Some(live.binding.clone());

    let pending = desktop.view(Some(&live), true);
    assert_eq!(pending.target_replicas, 0);
    assert_eq!(pending.provider_target_replicas, Some(2));
    assert_eq!(pending.retirement_status.as_deref(), Some("pending"));

    live.desired_replicas = 0;
    live.ready_replicas = 0;
    assert_eq!(
        desktop.view(Some(&live), true).retirement_status.as_deref(),
        Some("pending")
    );

    desktop.applied_revision = desktop.revision;
    let drained = desktop.view(Some(&live), true);
    assert_eq!(drained.provider_target_replicas, Some(0));
    assert_eq!(drained.retirement_status.as_deref(), Some("drained"));

    desktop.error = Some("retirement reconciliation will retry".into());
    let held = desktop.view(Some(&live), true);
    assert_eq!(held.retirement_status.as_deref(), Some("drained"));
    assert_eq!(held.status, "unavailable");
    assert_eq!(held.error, desktop.error);
    assert!(held.observed);
    assert_eq!(held.ready_replicas, Some(0));
    assert_eq!(
        desktop
            .view(Some(&live), false)
            .retirement_status
            .as_deref(),
        Some("pending")
    );

    live.ready_replicas = 1;
    assert_eq!(
        desktop.view(Some(&live), true).retirement_status.as_deref(),
        Some("pending")
    );
    live.ready_replicas = 0;
    live.binding.uid = "replacement".into();
    let replaced = desktop.view(Some(&live), true);
    assert_eq!(replaced.provider_target_replicas, None);
    assert_eq!(replaced.retirement_status.as_deref(), Some("pending"));
}
