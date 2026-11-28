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

#[test]
fn default_pool_authority_requires_exact_live_binding_and_excludes_duplicates() {
    let mut original = target();
    original.plan.spec.image = super::deployment_image(original.plan.spec.image.uri);
    let (service, _) = super::resize::service(original.clone(), original.plan.clone());
    let provider = Provider {
        calls: Arc::default(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let live = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: original.target_replicas,
        ready_replicas: 0,
        observed: true,
    };
    original.binding = Some(live.binding.clone());
    let released = service.released_images();
    let render = |targets: &[super::PoolTarget], live: &[NativePoolStatus], available| {
        service.inventory_view(
            &released,
            targets,
            live,
            available,
            [Some(original.plan.config_sha256.as_str()), None],
        )
    };
    let mut old = original.clone();
    old.id = "old".into();
    old.plan.pod_policy.service_account = "prior-policy".into();
    old.binding.as_mut().unwrap().uid = "prior-pool".into();
    let inventory = render(&[old, original.clone()], std::slice::from_ref(&live), true);
    assert_eq!(inventory.claims, [None, None]);
    assert_eq!(inventory.capacity_pools, [Some(original.id.clone()), None]);
    let ready = NativePoolStatus {
        ready_replicas: 1,
        ..live.clone()
    };
    let unobserved = NativePoolStatus {
        observed: false,
        ..ready.clone()
    };
    assert_eq!(
        render(std::slice::from_ref(&original), &[ready], true).claims,
        [Some(original.id.clone()), None]
    );
    assert_eq!(
        render(std::slice::from_ref(&original), &[unobserved], true).claims,
        [None, None]
    );
    assert_eq!(inventory.items[0].current_runtime, Some(true));
    assert_eq!(inventory.items[0].ready_replicas, Some(0));
    assert_eq!(inventory.items[1].current_runtime, Some(false));

    let mut duplicate = original.clone();
    duplicate.id = "duplicate".into();
    let duplicated = render(
        &[original.clone(), duplicate],
        std::slice::from_ref(&live),
        true,
    );
    assert_eq!(duplicated.items.len(), 1);
    assert_eq!(duplicated.claims, [None, None]);
    assert_eq!(duplicated.capacity_pools, [None, None]);
    assert_eq!(
        render(std::slice::from_ref(&original), &[], false).claims,
        [None, None]
    );

    service
        .default_holds
        .hold(false, released.agent.uri.clone());
    assert_eq!(
        render(std::slice::from_ref(&original), &[live], true).capacity_pools,
        [None, None]
    );
}

#[test]
fn native_default_pool_keeps_cold_template_authority_at_zero() {
    let mut original = target();
    original.plan.spec.image = super::deployment_image(original.plan.spec.image.uri);
    original.plan.spec.home.mode = layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2;
    original.target_replicas = 0;
    let (service, _) = super::resize::service(original.clone(), original.plan.clone());
    let provider = Provider {
        calls: Arc::default(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let live = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: 0,
        ready_replicas: 0,
        observed: true,
    };
    original.binding = Some(live.binding.clone());
    let inventory = service.inventory_view(
        &service.released_images(),
        std::slice::from_ref(&original),
        &[live],
        true,
        [Some(original.plan.config_sha256.as_str()), None],
    );
    assert_eq!(inventory.claims, [Some(original.id.clone()), None]);
    assert_eq!(inventory.capacity_pools, [Some(original.id), None]);
    assert_eq!(inventory.items[0].ready_replicas, Some(0));
}
