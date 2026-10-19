use super::{eligibility::*, *};
use layer2_core::sandbox::{SandboxHome, SandboxNativePool};
use layer3_adapter::agent_sandbox::{
    NativePoolClaim, NativePoolClaimRequest, NativePoolHomeOrigin,
};
use layer3_adapter::{HomeVolumeMode, HomeVolumeReceipt, HomeVolumeScope};

fn upgraded() -> (Sandbox, PoolTarget, String) {
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
