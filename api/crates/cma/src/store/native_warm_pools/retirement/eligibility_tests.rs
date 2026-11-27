use super::{eligibility::*, *};
use layer2_core::sandbox::{SandboxHome, SandboxNativePool};
use layer3_adapter::agent_sandbox::{
    NativePoolClaim, NativePoolClaimRequest, NativePoolHomeOrigin,
};
use layer3_adapter::{HomeVolumeMode, HomeVolumeReceipt, HomeVolumeScope};

pub(super) fn upgraded() -> (Sandbox, PoolTarget, String) {
    let mut sandbox =
        crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox("upgraded");
    let mut target = crate::warm_pools::tests::target();
    let policy = &mut target.plan.spec.home;
    policy.mode = HomeVolumeMode::NativeVctFullHomeV2;
    policy.template_name.clear();
    policy.template_generation.clear();
    target.plan.spec.image =
        crate::warm_pools::deployment_image(target.plan.spec.image.uri.clone());
    let binding = NativePoolBinding {
        spec: target.plan.spec.clone(),
        pod_policy: target.plan.pod_policy.clone(),
        name: "pool".into(),
        uid: "pool-uid".into(),
        template_name: "template".into(),
        template_uid: "template-uid".into(),
        config_sha256: target.plan.config_sha256.clone(),
    };
    target.binding = Some(binding.clone());
    let mut home = SandboxHome::reserve(&sandbox.id, binding.spec.home.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority("native-vct-empty-v1", &["test", "default", "runtime"])
            .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: binding,
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    let image = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    sandbox.runtime.as_mut().unwrap().runtime_version = image.clone();
    sandbox.validate().unwrap();
    (sandbox, target, image)
}

#[test]
fn settled_old_or_failed_home_allows_spare_drain_without_definition_retirement() {
    let (mut sandbox, target, image) = upgraded();
    let original_home = sandbox.lifecycle.home.clone();
    sandbox.runtime.as_mut().unwrap().runtime_version = target.plan.spec.image.uri.clone();
    assert_eq!(
        settled_native_home(&sandbox, &target).unwrap(),
        original_home.as_ref().unwrap().attachment()
    );
    assert!(matches!(
        owned_home(&sandbox, &target, &image).unwrap(),
        Owned::Blocked
    ));
    // Adoption can observe a newer Claim resourceVersion than the durable
    // assignment receipt without changing any original allocation identity.
    sandbox
        .lifecycle
        .home
        .as_mut()
        .unwrap()
        .native_pool
        .as_mut()
        .unwrap()
        .claim
        .as_mut()
        .unwrap()
        .resource_version = "newer-observation".into();
    assert_eq!(
        settled_native_home(&sandbox, &target).unwrap(),
        original_home.as_ref().unwrap().attachment()
    );
    sandbox.lifecycle.home = original_home.clone();
    sandbox.status = SandboxStatus::Failed;
    sandbox.runtime = None;
    sandbox.failure = Some(
        layer2_core::sandbox::SandboxReviveFailureClass::RestoreFailed
            .typed_failure("historical restore failed"),
    );
    sandbox.validate().unwrap();
    assert_eq!(
        settled_native_home(&sandbox, &target).unwrap(),
        original_home.as_ref().unwrap().attachment()
    );
    assert!(matches!(
        owned_home(&sandbox, &target, &image).unwrap(),
        Owned::Blocked
    ));
    assert_eq!(sandbox.lifecycle.home, original_home);
}

#[test]
fn same_image_desktop_retirement_distinguishes_selected_and_unresolved_homes() {
    let (mut sandbox, mut original, _) = upgraded();
    original.machine = true;
    let image = original.plan.spec.image.uri.clone();
    sandbox.runtime.as_mut().unwrap().runtime_version = image.clone();
    let mut obsolete = original.clone();
    obsolete.plan.config_sha256 = "b".repeat(64);
    let binding = obsolete.binding.as_mut().unwrap();
    binding.config_sha256 = obsolete.plan.config_sha256.clone();
    binding.uid = "obsolete-pool-uid".into();
    binding.name = "obsolete-pool".into();
    assert!(matches!(
        owned_home(&sandbox, &obsolete, &image).unwrap(),
        Owned::Unrelated
    ));
    assert!(matches!(
        owned_home(&sandbox, &original, &image).unwrap(),
        Owned::Blocked
    ));

    obsolete.binding.as_mut().unwrap().uid = original.binding.as_ref().unwrap().uid.clone();
    assert!(matches!(
        owned_home(&sandbox, &obsolete, &image),
        Err(WarmPoolError::Conflict)
    ));

    sandbox.status = SandboxStatus::Provisioning;
    sandbox.lifecycle.home = None;
    assert!(matches!(
        owned_home(&sandbox, &obsolete, &image).unwrap(),
        Owned::Blocked
    ));
}

#[test]
fn drain_requires_completed_original_attachment_not_a_claim_reservation() {
    let (sandbox, target, _) = upgraded();
    for status in [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Reviving,
    ] {
        let mut pending = sandbox.clone();
        pending.status = status;
        assert!(settled_native_home(&pending, &target).unwrap().is_none());
    }
    let mut pending = sandbox.clone();
    pending.lifecycle.home.as_mut().unwrap().receipt = None;
    assert!(settled_native_home(&pending, &target).unwrap().is_none());
    let mut pending = sandbox.clone();
    pending
        .lifecycle
        .home
        .as_mut()
        .unwrap()
        .native_pool
        .as_mut()
        .unwrap()
        .claim = None;
    assert!(matches!(
        settled_native_home(&pending, &target),
        Err(WarmPoolError::Conflict)
    ));
    let mut changed = target;
    changed.binding.as_mut().unwrap().uid = "replaced-pool".into();
    assert!(matches!(
        settled_native_home(&sandbox, &changed),
        Err(WarmPoolError::Conflict)
    ));
}

#[test]
fn upgraded_home_with_template_networking_allows_drain_but_keeps_definitions() {
    let (mut sandbox, mut target, image) = upgraded();
    let policy = Some(
        layer3_adapter::agent_sandbox::NativeTemplateNetworkPolicy::parse(
            r#"{"ingress":[],"egress":[]}"#,
        )
        .unwrap(),
    );
    target.plan.pod_policy.template_network_policy = policy.clone();
    target
        .binding
        .as_mut()
        .unwrap()
        .pod_policy
        .template_network_policy = policy.clone();
    let home = sandbox.lifecycle.home.as_mut().unwrap();
    let native = home.native_pool.as_mut().unwrap();
    native.request.pool.pod_policy.template_network_policy = policy.clone();
    native
        .claim
        .as_mut()
        .unwrap()
        .request
        .pool
        .pod_policy
        .template_network_policy = policy.clone();
    home.receipt
        .as_mut()
        .unwrap()
        .native_authority
        .as_mut()
        .unwrap()
        .claim
        .request
        .pool
        .pod_policy
        .template_network_policy = policy;
    sandbox.validate().unwrap();
    assert!(matches!(
        owned_home(&sandbox, &target, &image).unwrap(),
        Owned::Blocked
    ));
    assert_eq!(
        settled_native_home(&sandbox, &target).unwrap(),
        sandbox.lifecycle.home.as_ref().unwrap().attachment()
    );
}

#[test]
fn upgraded_active_and_paused_users_retain_exact_original_home() {
    let (mut sandbox, target, image) = upgraded();
    let expected = sandbox
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap();
    let Owned::Retained(home) = owned_home(&sandbox, &target, &image).unwrap() else {
        panic!("upgraded active user must retain original home")
    };
    assert_eq!(*home, expected);
    sandbox.lifecycle.suspended_runtime = sandbox.runtime.take().map(Box::new);
    sandbox.status = SandboxStatus::Paused;
    sandbox.lifecycle.home_recovery = Some(Box::new(layer2_core::sandbox::SandboxHomeRecovery {
        home_identity: expected.identity().unwrap(),
        source_generation: 1,
        resume_pause: None,
    }));
    sandbox.validate().unwrap();
    let Owned::Retained(home) = owned_home(&sandbox, &target, &image).unwrap() else {
        panic!("upgraded paused user must retain original home")
    };
    assert_eq!(*home, expected);
    sandbox
        .lifecycle
        .suspended_runtime
        .as_mut()
        .unwrap()
        .runtime_version = target.plan.spec.image.uri.clone();
    assert!(matches!(
        owned_home(&sandbox, &target, &image).unwrap(),
        Owned::Blocked
    ));
}

#[test]
fn incomplete_upgrade_failed_recovery_and_replaced_pool_never_allow_retirement() {
    let (sandbox, target, image) = upgraded();
    let mut updating = sandbox.clone();
    updating.status = SandboxStatus::Reviving;
    assert!(matches!(
        owned_home(&updating, &target, &image).unwrap(),
        Owned::Blocked
    ));
    let mut failed = sandbox.clone();
    failed.status = SandboxStatus::Failed;
    assert!(matches!(
        owned_home(&failed, &target, &image).unwrap(),
        Owned::Blocked
    ));
    let mut other = target.clone();
    other.binding.as_mut().unwrap().uid = "replaced-pool".into();
    assert!(matches!(
        owned_home(&sandbox, &other, &image),
        Err(WarmPoolError::Conflict)
    ));
    let mut old = sandbox;
    old.runtime.as_mut().unwrap().runtime_version = target.plan.spec.image.uri.clone();
    assert!(matches!(
        owned_home(&old, &target, &image).unwrap(),
        Owned::Blocked
    ));
}

#[test]
fn incomplete_allocation_blocks_every_pool_only_until_it_records_its_pool() {
    let (sandbox, target, image) = upgraded();
    let mut other = target.clone();
    other.plan.config_sha256 = "other-config".into();
    other.plan.spec.image.uri = "ghcr.io/chronoaiproject/cma-runtime@sha256:other".into();
    for status in [SandboxStatus::Provisioning, SandboxStatus::Installing] {
        let mut pending = sandbox.clone();
        pending.status = status;
        // Its recorded pool stays protected, but another pool may now retire.
        assert!(matches!(
            owned_home(&pending, &target, &image).unwrap(),
            Owned::Blocked
        ));
        assert!(matches!(
            owned_home(&pending, &other, &image).unwrap(),
            Owned::Unrelated
        ));
        // Before the pool is recorded, the selection may still be in flight.
        let mut unrecorded = pending.clone();
        unrecorded.lifecycle.home.as_mut().unwrap().native_pool = None;
        assert!(matches!(
            owned_home(&unrecorded, &other, &image).unwrap(),
            Owned::Blocked
        ));
        let mut homeless = pending.clone();
        homeless.lifecycle.home = None;
        assert!(matches!(
            owned_home(&homeless, &other, &image).unwrap(),
            Owned::Blocked
        ));
    }
}
