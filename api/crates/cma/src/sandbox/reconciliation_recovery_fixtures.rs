use super::*;
use layer3_adapter::{
    HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope, RuntimeImage,
    RUNTIME_CONTRACT_LABEL,
};

pub(super) fn candidate(index: usize) -> Sandbox {
    let mut row = sandbox(index);
    row.status = SandboxStatus::Failed;
    row.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "fixture".into(),
    });
    row.work
        .schedule(SandboxWorkStep::None, row.updated_at.clone());
    row.lifecycle.home = Some(Box::new(home(&row.id)));
    row.lifecycle.runtime_generation_high_water = 2;
    row.runtime = Some(RuntimeBinding {
        runtime_id: "agent-sandbox:member:sandbox-uid".into(),
        runtime_version: "fixture-image".into(),
        provision_generation: Some(2),
        boot_id: Some("b".repeat(32)),
        bound_at: row.updated_at.clone(),
    });
    row.credential.key_generation = 1;
    row.credential.delivered = true;
    row.credential.current = Some(SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: "fixture".into(),
        provider_state_version: 1,
        issued_at: row.updated_at.clone(),
        expires_at: "2026-09-06T00:00:00Z".into(),
        grant_fingerprint: "b".repeat(64),
    });
    row.validate().unwrap();
    assert!(row.unpublished_recovery_candidate());
    row
}

pub(super) struct Verify {
    pub(super) calls: Mutex<Vec<SandboxId>>,
    pub(super) refuse: Option<SandboxId>,
}
#[async_trait::async_trait]
impl SandboxUnpublishedRecoveryVerifier for Verify {
    async fn verify(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        self.calls.lock().unwrap().push(snapshot.sandbox.id.clone());
        if self.refuse.as_ref() == Some(&snapshot.sandbox.id) {
            return Err(SandboxStoreError::Unavailable(
                "private provider error".into(),
            ));
        }
        let plan = serde_json::json!({ "fixture": "no processes" });
        Ok(SandboxUnpublishedRecoveryProof {
            sandbox_id: snapshot.sandbox.id.clone(),
            runtime_id: snapshot
                .sandbox
                .runtime
                .as_ref()
                .unwrap()
                .runtime_id
                .clone(),
            runtime_generation: snapshot.runtime_generation()?,
            home_identity: snapshot.home_identity()?,
            boot_id: None,
            reserved_provision: None,
            observed_provision_generation: None,
            runtime_process_absent: true,
            requires_native_quiescence: true,
            provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&plan)
                .unwrap(),
            provider_plan: plan,
            conversations: vec![],
        })
    }
}

pub(super) async fn recovery_setup(
    count: usize,
    refuse: Option<usize>,
) -> (Arc<fixtures::Store>, SandboxReconciler, Arc<Verify>) {
    let (store, reconciler) = setup(0, Duration::ZERO).await;
    for index in 0..count {
        store.create_sandbox(candidate(index)).await.unwrap();
    }
    let verifier = Arc::new(Verify {
        calls: Mutex::default(),
        refuse: refuse.map(|i| sandbox(i).id),
    });
    let service = Arc::new(
        SandboxRuntimeUpdateService::new(store.clone(), "fixture-image".into())
            .with_unpublished_recovery(verifier.clone()),
    );
    (
        store,
        reconciler.with_failed_runtime_recovery(service),
        verifier,
    )
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
