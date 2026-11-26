use super::*;
use layer2_core::sandbox::{
    Sandbox, SandboxHome, SandboxHomeRecovery, SandboxNativePool, SandboxStatus, SandboxWorkStep,
};
use layer3_adapter::{
    HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope, RuntimeImage,
    RUNTIME_CONTRACT_LABEL,
};

fn retained(index: usize, paused: bool) -> Sandbox {
    let mut row = active(index, false);
    row.work
        .schedule(SandboxWorkStep::None, row.updated_at.clone());
    row.runtime.as_mut().unwrap().runtime_id = "agent-sandbox:member:sandbox-uid".into();
    row.lifecycle.home = Some(Box::new(home(&row.id)));
    if paused {
        row.status = SandboxStatus::Paused;
        row.lifecycle.suspended_runtime = row.runtime.take().map(Box::new);
        row.lifecycle.home_recovery = Some(Box::new(SandboxHomeRecovery {
            home_identity: row
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .attachment()
                .unwrap()
                .identity()
                .unwrap(),
            source_generation: row.lifecycle.runtime_generation_high_water,
            resume_pause: None,
        }));
    }
    row.validate().unwrap();
    row
}

async fn update_sweep(grace: bool) -> (Arc<fixtures::Store>, SandboxReconciler) {
    let (_, policy, settings) = live().await;
    if grace {
        enable(&settings, now(), 900);
    }
    let (store, mut reconciler) = setup(0, Duration::ZERO).await;
    reconciler.policy = policy.policy;
    reconciler = reconciler.with_runtime_version("image-v2".into());
    // Exercise the real policy/candidate/update admissions. Native provider
    // adoption is covered separately; these valid retained records have no I/O.
    let mut progress = Progress::default();
    progress.sweep.phase = Phase::Policy;
    *reconciler.progress.lock().await = Some(progress);
    (store, reconciler)
}

async fn queue(store: &fixtures::Store, mut row: Sandbox, automatic: bool) -> Sandbox {
    store.create_sandbox(row.clone()).await.unwrap();
    let revision = row.revision;
    row.request_runtime_update("image-v2".into(), automatic, rfc3339_at(now()))
        .unwrap();
    store.save_sandbox(revision, row.clone()).await.unwrap();
    row
}

#[tokio::test(start_paused = true)]
async fn explicit_updates_scan_past_unrelated_and_automatic_rows_when_auto_pause_is_off_or_in_grace(
) {
    for grace in [false, true] {
        for paused in [false, true] {
            let (store, reconciler) = update_sweep(grace).await;
            let unrelated = retained(0, true);
            store.create_sandbox(unrelated.clone()).await.unwrap();
            let automatic = queue(&store, retained(1, true), true).await;
            let explicit = queue(&store, retained(2, paused), false).await;
            let original_home = explicit.lifecycle.home.clone();
            assert!(reconciler.run_due(now()).await.unwrap());
            assert_eq!(store.keys("candidates"), ["start", "sbx_001"]);
            assert!(store.keys("pause").is_empty());
            assert_eq!(
                store.get_sandbox(&unrelated.id).await.unwrap(),
                Some(unrelated)
            );
            assert_eq!(
                store.get_sandbox(&automatic.id).await.unwrap(),
                Some(automatic)
            );
            let admitted = store.get_sandbox(&explicit.id).await.unwrap().unwrap();
            assert_eq!(admitted.status, SandboxStatus::Rotating);
            assert_eq!(admitted.work.step, SandboxWorkStep::RunLifecycleOperation);
            assert_eq!(admitted.lifecycle.home, original_home);
            assert_eq!(admitted.admitted_runtime_update_target(), Some("image-v2"));
            let request = admitted.runtime_update_request().unwrap();
            assert!(request.started_at.is_some());
            assert!(!request.automatic);
            assert!(request.completed_at.is_none());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_updates_do_not_override_platform_spend_stop() {
    let (store, reconciler) = update_sweep(false).await;
    let reconciler = reconciler.with_platform_spend(Arc::new(Platform(Some(true))));
    let queued = queue(&store, retained(0, true), false).await;
    assert!(reconciler.run_due(now()).await.unwrap());
    assert_eq!(store.get_sandbox(&queued.id).await.unwrap(), Some(queued));
}

#[tokio::test(start_paused = true)]
async fn explicit_updates_defer_to_existing_worker_ownership() {
    let (store, reconciler) = update_sweep(false).await;
    let mut queued = retained(0, false);
    queued.work.schedule(
        SandboxWorkStep::WaitForCredentialRotation,
        rfc3339_at(now()),
    );
    queued
        .work
        .claim(
            "existing-worker".into(),
            &rfc3339_at(now()),
            rfc3339_at(now() + Duration::from_secs(60)),
        )
        .unwrap();
    queued.validate().unwrap();
    let queued = queue(&store, queued, false).await;
    assert!(reconciler.run_due(now()).await.unwrap());
    assert_eq!(store.get_sandbox(&queued.id).await.unwrap(), Some(queued));
}

fn home(id: &layer2_core::SandboxId) -> SandboxHome {
    use layer3_adapter::agent_sandbox::*;
    let policy = HomeVolumePolicy {
        mode: HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "standard-rwo".into(),
        capacity_gib: 8,
        template_name: String::new(),
        template_generation: String::new(),
    };
    let mut home = SandboxHome::reserve(id, policy.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &["attachment-test", "default", "runtime"],
        )
        .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                    labels: std::collections::BTreeMap::from([
                        (RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: policy,
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
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
    home
}
