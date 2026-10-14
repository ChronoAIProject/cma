use super::{
    fixture::Fixture,
    home_tests::{adopted, assigned_spec, MEMBER, VOLUME},
    model, network_policy_tests, runtime_tests, tests, NativeFsGroupChangePolicy,
    NativePoolPodPolicy, NativePoolSpec,
};
use crate::{HomeVolumeMode, HomeVolumeStore};
use kube::ResourceExt;
use serde_json::json;

const PRESERVE: Option<NativeFsGroupChangePolicy> = Some(NativeFsGroupChangePolicy::OnRootMismatch);

fn spec(mode: HomeVolumeMode) -> NativePoolSpec {
    let mut spec = tests::spec();
    spec.home.mode = mode;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    spec.image.labels.insert(
        crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
        "2".into(),
    );
    spec
}

fn modes() -> [HomeVolumeMode; 2] {
    [
        HomeVolumeMode::NativeVctEmptyV1,
        HomeVolumeMode::NativeVctFullHomeV2,
    ]
}

#[test]
fn historical_policy_roundtrips_and_v1_v2_digests_remain_identical() {
    let old = r#"{"runtime_class":"gvisor","service_account":"runtime","image_pull_secrets":[]}"#;
    let decoded: NativePoolPodPolicy = serde_json::from_str(old).unwrap();
    assert_eq!(decoded.fs_group_change_policy, None);
    assert_eq!(serde_json::to_string(&decoded).unwrap(), old);
    for mode in modes() {
        let spec = spec(mode);
        for network in [None, Some(network_policy_tests::policy("10.254.0.0/16"))] {
            let mut config = super::super::tests::config();
            config.template_network_policy = network.clone();
            let policy = NativePoolPodPolicy::from(&config);
            let encoded = serde_json::to_string(&policy).unwrap();
            assert!(!encoded.contains("fs_group_change_policy"));
            assert_eq!(
                serde_json::from_str::<NativePoolPodPolicy>(&encoded).unwrap(),
                policy
            );
            let original = (
                "native-pool-v1",
                &config.namespace,
                &config.deployment,
                &config.runtime_class,
                &config.service_account,
                &config.image_pull_secrets,
                &spec,
            );
            let historical = match network {
                Some(ref network) => {
                    crate::provider_request_digest(&("native-pool-v2", original, network))
                }
                None => crate::provider_request_digest(&original),
            }
            .unwrap();
            assert_eq!(model::digest(&config, &spec).unwrap(), historical);
            let before = model::template(&config, &spec).unwrap();
            assert!(
                before.data["spec"]["podTemplate"]["spec"]["securityContext"]
                    .get("fsGroupChangePolicy")
                    .is_none()
            );
            config.template_fs_group_change_policy = PRESERVE;
            let after = model::template(&config, &spec).unwrap();
            assert_ne!(model::digest(&config, &spec).unwrap(), historical);
            assert_ne!(after.name_any(), before.name_any());
            assert_eq!(
                after.data["spec"]["podTemplate"]["spec"]["securityContext"]["fsGroupChangePolicy"],
                "OnRootMismatch"
            );
            assert_eq!(
                after.data["spec"]["podTemplate"]["spec"]["securityContext"]["fsGroup"],
                10001
            );
            let mut pod = after.data["spec"]["podTemplate"]["spec"].clone();
            pod["securityContext"]
                .as_object_mut()
                .unwrap()
                .remove("fsGroupChangePolicy");
            assert_eq!(pod, before.data["spec"]["podTemplate"]["spec"]);
        }
    }
    for unsupported in ["Always", "on_root_mismatch", "unknown"] {
        let mut value = serde_json::to_value(decoded.clone()).unwrap();
        value["fs_group_change_policy"] = json!(unsupported);
        assert!(serde_json::from_value::<NativePoolPodPolicy>(value).is_err());
    }
}

#[tokio::test]
async fn persisted_old_and_new_policy_survive_lost_ack_restart_inventory_and_resize() {
    for original_policy in [None, PRESERVE] {
        let mut fixture = Fixture::new().await;
        fixture.client.config.template_fs_group_change_policy = original_policy;
        let plan = fixture
            .client
            .plan(&spec(HomeVolumeMode::NativeVctFullHomeV2))
            .unwrap();
        fixture.lost("POST", "sandboxtemplates");
        fixture.lost("POST", "sandboxwarmpools");
        let original = fixture.client.configure_planned(&plan, 0).await.unwrap();
        let template = fixture.row("sandboxtemplates", &original.binding.template_name);
        fixture.client.config.template_fs_group_change_policy = if original_policy.is_some() {
            None
        } else {
            PRESERVE
        };
        assert_ne!(
            fixture.client.plan(&plan.spec).unwrap().config_sha256,
            plan.config_sha256
        );
        assert_eq!(
            fixture.client.configure_planned(&plan, 0).await.unwrap(),
            original
        );
        assert_eq!(
            fixture.client.inventory().await.unwrap(),
            vec![original.clone()]
        );
        fixture.lost("PATCH", "sandboxwarmpools");
        assert_eq!(
            fixture
                .client
                .resize(&original.binding, 2)
                .await
                .unwrap()
                .binding,
            original.binding
        );
        assert_eq!(
            fixture.row("sandboxtemplates", &original.binding.template_name),
            template
        );
        let mut tampered = plan;
        tampered.pod_policy.fs_group_change_policy =
            fixture.client.config.template_fs_group_change_policy;
        let before = fixture.state.lock().unwrap().calls.len();
        assert!(fixture
            .client
            .configure_planned(&tampered, 0)
            .await
            .is_err());
        assert_eq!(fixture.state.lock().unwrap().calls.len(), before);
    }
}

#[tokio::test]
async fn new_default_skips_ready_old_pool_and_selects_its_exact_cold_then_warm_pool() {
    let mut fixture = Fixture::new().await;
    let spec = spec(HomeVolumeMode::NativeVctFullHomeV2);
    let old = fixture.client.configure(&spec, 2).await.unwrap().binding;
    ready(&fixture, &old, 2);
    let original = fixture.row("sandboxwarmpools", &old.name);
    fixture.client.config.template_fs_group_change_policy = PRESERVE;
    assert!(fixture.client.select(&spec).await.unwrap().is_none());
    let request = crate::hosted::HostedDeployment {
        image: spec.image.clone(),
        ..Default::default()
    }
    .render(&crate::SandboxSpec {
        sandbox_id: "sbx_one".into(),
        created_by: Some("cma-test".into()),
        capacity: crate::SandboxCapacity {
            cpu_millis: spec.cpu_millis,
            memory_mib: spec.memory_mib,
        },
        ..Default::default()
    })
    .unwrap();
    let homes = super::NativeHomeVolumes::new(
        fixture.kubernetes.clone(),
        fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let home = crate::HomeVolumeRequest {
        volume_id: "home-one".into(),
        sandbox_id: "sbx_one".into(),
        policy: spec.home.clone(),
        scope: homes.allocation_scope(),
    };
    let new = fixture
        .client
        .select_runtime(&request, &spec.image, &home)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(new.name, old.name);
    assert_ne!(new.config_sha256, old.config_sha256);
    assert_eq!(new.pod_policy.fs_group_change_policy, PRESERVE);
    assert_eq!(
        fixture.row("sandboxwarmpools", &new.name)["spec"]["replicas"],
        0
    );
    fixture.client.resize(&new, 3).await.unwrap();
    ready(&fixture, &new, 3);
    assert_eq!(
        fixture.client.select(&spec).await.unwrap(),
        Some(new.clone())
    );
    assert_eq!(
        fixture
            .client
            .select_runtime(&request, &spec.image, &home)
            .await
            .unwrap(),
        Some(new)
    );
    assert_eq!(fixture.row("sandboxwarmpools", &old.name), original);
    assert_eq!(fixture.client.resize(&old, 1).await.unwrap().binding, old);
}

fn ready(fixture: &Fixture, binding: &super::NativePoolBinding, count: u32) {
    fixture.edit("sandboxwarmpools", &binding.name, |row| {
        row["status"] =
            json!({"observedGeneration":row["metadata"]["generation"],"readyReplicas":count});
    });
}

#[tokio::test]
async fn both_home_layouts_resume_with_original_policy_template_and_pvc_after_default_change() {
    for mode in modes() {
        for original_policy in [None, PRESERVE] {
            let mut fixture = Fixture::new().await;
            fixture.client.config.template_fs_group_change_policy = original_policy;
            let claim = assigned_spec(&fixture, &spec(mode)).await;
            let home = adopted(&fixture, &claim).await;
            let template = fixture.row("sandboxtemplates", &claim.request.pool.template_name);
            let (member, volume) = {
                let state = fixture.state.lock().unwrap();
                (
                    state.rows[MEMBER]["spec"].clone(),
                    state.rows[VOLUME].clone(),
                )
            };
            let previous = runtime_tests::request(&home, 1);
            let client = runtime_tests::client(&fixture, false);
            let runtime = client
                .admit_pool_runtime(&claim, &home, &previous)
                .await
                .unwrap();
            client.suspend(&runtime, &previous).await.unwrap();
            fixture.client.config.template_fs_group_change_policy = if original_policy.is_some() {
                None
            } else {
                PRESERVE
            };
            let restarted = runtime_tests::client(&fixture, true);
            let next = runtime_tests::request(&home, 2);
            restarted
                .resume_with_home(&runtime, &previous, &next, &home)
                .await
                .unwrap();
            restarted
                .validate_request(&runtime, &next)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                fixture.client.recover_home(&claim).await.unwrap(),
                Some(home)
            );
            assert_eq!(
                fixture.row("sandboxtemplates", &claim.request.pool.template_name),
                template
            );
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.rows[MEMBER]["spec"], member);
            assert_eq!(state.rows[VOLUME], volume);
            assert!(state
                .calls
                .iter()
                .all(|(call, _)| !call.contains("persistentvolumeclaims")
                    || call.starts_with("GET ")));
        }
    }
}

#[tokio::test]
async fn missing_or_changed_policy_in_template_or_member_refuses_before_writes() {
    for resource in ["template", "member"] {
        for phase in ["adopt", "resume"] {
            for replacement in [serde_json::Value::Null, json!("Always"), json!("unknown")] {
                let mut fixture = Fixture::new().await;
                fixture.client.config.template_fs_group_change_policy = PRESERVE;
                let claim =
                    assigned_spec(&fixture, &spec(HomeVolumeMode::NativeVctFullHomeV2)).await;
                let retained = if phase == "resume" {
                    let home = adopted(&fixture, &claim).await;
                    let previous = runtime_tests::request(&home, 1);
                    let client = runtime_tests::client(&fixture, false);
                    let runtime = client
                        .admit_pool_runtime(&claim, &home, &previous)
                        .await
                        .unwrap();
                    client.suspend(&runtime, &previous).await.unwrap();
                    Some((home, previous, client, runtime))
                } else {
                    None
                };
                let corrupt = |row: &mut serde_json::Value| {
                    let context = row["spec"]["podTemplate"]["spec"]["securityContext"]
                        .as_object_mut()
                        .unwrap();
                    if replacement.is_null() {
                        context.remove("fsGroupChangePolicy");
                    } else {
                        context.insert("fsGroupChangePolicy".into(), replacement.clone());
                    }
                };
                if resource == "template" {
                    fixture.edit(
                        "sandboxtemplates",
                        &claim.request.pool.template_name,
                        corrupt,
                    );
                } else {
                    corrupt(fixture.state.lock().unwrap().rows.get_mut(MEMBER).unwrap());
                }
                let before = fixture.state.lock().unwrap().calls.len();
                if let Some((home, previous, client, runtime)) = retained {
                    assert!(client
                        .resume_with_home(
                            &runtime,
                            &previous,
                            &runtime_tests::request(&home, 2),
                            &home
                        )
                        .await
                        .is_err());
                } else {
                    assert!(fixture.client.adopt_home(&claim).await.is_err());
                }
                assert!(fixture.state.lock().unwrap().calls[before..]
                    .iter()
                    .all(|(call, _)| call.starts_with("GET ")));
            }
        }
    }
}
