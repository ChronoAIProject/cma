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

        let unavailable = original.view(None, false);
        assert_eq!(unavailable.status, "unavailable");
        assert!(!unavailable.observed);
        assert_eq!(unavailable.ready_replicas, None);
    }
}
