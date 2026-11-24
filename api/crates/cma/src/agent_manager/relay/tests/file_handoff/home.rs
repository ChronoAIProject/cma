use layer2_core::sandbox::{SandboxHome, SandboxNativePool};

pub(super) fn native_home(id: &layer2_core::SandboxId) -> SandboxHome {
    use layer3_adapter::agent_sandbox::*;
    use layer3_adapter::{
        HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope, RuntimeImage,
        RUNTIME_CONTRACT_LABEL,
    };
    let policy = HomeVolumePolicy {
        mode: HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "fixture-storage".into(),
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
