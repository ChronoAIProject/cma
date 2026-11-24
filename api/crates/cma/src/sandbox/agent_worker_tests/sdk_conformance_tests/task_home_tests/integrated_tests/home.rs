use layer3_adapter::agent_sandbox::*;
use layer3_adapter::{HomeVolumeReceipt, HomeVolumeRequest, RuntimeImage};

pub(in super::super) fn receipt(home: &HomeVolumeRequest) -> HomeVolumeReceipt {
    let origin = NativePoolHomeOrigin {
        claim: NativePoolClaim {
            request: NativePoolClaimRequest {
                pool: NativePoolBinding {
                    spec: NativePoolSpec {
                        cold_canary: false,
                        image: RuntimeImage {
                            uri: super::super::desktop::image(),
                            labels: std::collections::BTreeMap::from([
                                ("dev.chrono.cma.runtime-contract".into(), "3".into()),
                                ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                                ("dev.chrono.cma.home-layout".into(), "2".into()),
                            ]),
                        },
                        cpu_millis: 1000,
                        memory_mib: 2048,
                        home: home.policy.clone(),
                    },
                    pod_policy: NativePoolPodPolicy {
                        runtime_class: None,
                        service_account: "runtime".into(),
                        image_pull_secrets: vec![],
                        template_network_policy: None,
                        fs_group_change_policy: None,
                    },
                    name: "proof-pool".into(),
                    uid: "proof-pool-uid".into(),
                    template_name: "proof-template".into(),
                    template_uid: "proof-template-uid".into(),
                    config_sha256: "a".repeat(64),
                },
                home: home.clone(),
                runtime_generation: 1,
            },
            name: "proof-claim".into(),
            uid: "proof-claim-uid".into(),
            resource_version: "1".into(),
            sandbox_name: Some("proof-member".into()),
        },
        sandbox_uid: "proof-member-uid".into(),
        pvc_uid: "proof-home-uid".into(),
    };
    let receipt = HomeVolumeReceipt {
        storage_reference: origin.storage_reference().unwrap(),
        storage_generation: origin.pvc_uid.clone(),
        native_authority: Some(Box::new(origin)),
        volume_id: home.volume_id.clone(),
        request_sha256: home.computed_sha256(),
    };
    receipt.validate_for(home).unwrap();
    receipt
}
