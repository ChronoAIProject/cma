use super::fixture::{seed_path, Fixture};
use super::home_tests::{adopted, assigned_spec, MEMBER, VOLUME};
use super::*;
use crate::{HomeVolumeMode, HomeVolumeStore};
use serde_json::json;

#[path = "native_admission_tests.rs"]
mod admission_tests;
#[path = "native_adoption_read_tests.rs"]
mod adoption_read_tests;
#[path = "native_full_home_tests.rs"]
mod full_home_tests;
#[path = "native_image_selection_tests.rs"]
mod image_selection_tests;
#[path = "native_home_read_tests.rs"]
mod read_tests;
#[path = "native_home_ready_tests.rs"]
mod ready_tests;
#[path = "native_resume_read_tests.rs"]
mod resume_read_tests;
#[path = "native_runtime_render_read_tests.rs"]
mod runtime_render_read_tests;

#[path = "runtime_retirement_finality_tests.rs"]
mod retirement_finality_tests;

#[path = "metric_identity_tests.rs"]
mod metrics_identity;

fn spec() -> NativePoolSpec {
    let mut spec = super::tests::spec();
    spec.home.mode = HomeVolumeMode::NativeVctEmptyV1;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    spec
}
fn homes(fixture: &Fixture) -> NativeHomeVolumes {
    NativeHomeVolumes::new(
        fixture.kubernetes.clone(),
        fixture.client.config.clone(),
        "fixture",
    )
    .unwrap()
}
fn no_storage_effects(fixture: &Fixture) {
    let state = fixture.state.lock().unwrap();
    assert!(
        state.calls.iter().all(|(call, _)| {
            !call.contains("snapshot.storage.k8s.io")
                && (!call.contains("persistentvolumeclaims") || call.starts_with("GET "))
        }),
        "new native storage is exclusively controller-owned"
    );
}

#[tokio::test]
async fn empty_vct_initializes_root_once_and_adopts_without_seed_or_pvc_writes() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().rows.remove(seed_path());
    homes(&fixture).preflight(&spec().home).await.unwrap();
    let claim = assigned_spec(&fixture, &spec()).await;
    let template = fixture.row("sandboxtemplates", &claim.request.pool.template_name);
    let pod = &template["spec"]["podTemplate"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    let init = &pod["initContainers"][0];
    assert_eq!(init["image"], spec().image.uri);
    assert_eq!(
        init["command"],
        json!([
            "/usr/local/bin/layer4-runtime",
            "--initialize-home",
            "/mnt/cma-home"
        ])
    );
    assert_eq!(
        init["volumeMounts"],
        json!([{"name":"home","mountPath":"/mnt/cma-home","readOnly":false}])
    );
    assert_eq!(init["securityContext"]["runAsUser"], 10001);
    assert_eq!(init["env"], json!([]));
    assert!(template["spec"]["volumeClaimTemplates"][0]["spec"]
        .get("dataSource")
        .is_none());
    let before = fixture.state.lock().unwrap().rows[VOLUME].clone();
    let member_spec = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    let home = adopted(&fixture, &claim).await;
    assert_eq!(
        home.receipt.native_authority.as_ref().unwrap().pvc_uid,
        "pvc-uid"
    );
    homes(&fixture).verify(&home).await.unwrap();
    assert_eq!(
        homes(&fixture).inventory().await.unwrap(),
        vec![home.clone()]
    );
    assert!(homes(&fixture).prepare(&home.request).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().rows[VOLUME], before);
    assert_eq!(
        fixture.state.lock().unwrap().rows[MEMBER]["spec"],
        member_spec
    );
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn native_home_accepts_controller_normalized_member_fields() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    {
        let mut state = fixture.state.lock().unwrap();
        let member = state.rows.get_mut(MEMBER).unwrap();
        let pod = &mut member["spec"]["podTemplate"]["spec"];
        for field in ["hostNetwork", "hostPID", "hostIPC"] {
            pod.as_object_mut().unwrap().remove(field);
        }
        let resources = &mut pod["containers"][0]["resources"];
        resources["requests"]["cpu"] = json!("0.75");
        resources["requests"]["memory"] = json!("1.5Gi");
        resources["limits"]["cpu"] = json!("0.75");
        resources["limits"]["memory"] = json!("1.5Gi");
    }
    let home = adopted(&fixture, &claim).await;
    assert_eq!(home.receipt.storage_reference, "home-warm-member");
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn native_home_replays_lost_claim_and_sandbox_authority_writes() {
    for phase in ["claim", "sandbox"] {
        let fixture = Fixture::new().await;
        let claim = assigned_spec(&fixture, &spec()).await;
        let path = if phase == "claim" {
            format!("{}/sandboxclaims/{}", super::fixture::prefix(), claim.name)
        } else {
            MEMBER.into()
        };
        fixture
            .state
            .lock()
            .unwrap()
            .lose
            .push(format!("PATCH {path}"));
        assert!(fixture.client.adopt_home(&claim).await.is_err());
        let home = adopted(&fixture, &claim).await;
        assert_eq!(
            fixture.client.recover_home(&claim).await.unwrap(),
            Some(home.clone())
        );
        homes(&fixture).verify(&home).await.unwrap();
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(call, _)| call == &format!("PATCH {path}"))
                .count(),
            1
        );
        no_storage_effects(&fixture);
    }
}

#[tokio::test]
async fn native_home_retries_version_conflicts_without_replacing_claim_or_storage() {
    for phase in ["claim", "sandbox"] {
        for replaced_claim in [false, true] {
            let fixture = Fixture::new().await;
            let claim = assigned_spec(&fixture, &spec()).await;
            let path = if phase == "claim" {
                format!("{}/sandboxclaims/{}", super::fixture::prefix(), claim.name)
            } else {
                MEMBER.into()
            };
            let patch = format!("PATCH {path}");
            let (before, volume, floor) = {
                let mut state = fixture.state.lock().unwrap();
                state.conflict.push(patch.clone());
                (
                    state.rows.clone(),
                    state.rows[VOLUME].clone(),
                    state.calls.len(),
                )
            };
            assert!(matches!(
                fixture.client.adopt_home(&claim).await,
                Err(crate::AdapterError::Unavailable(_))
            ));
            assert!(
                fixture.state.lock().unwrap().rows[&path]["metadata"]["annotations"]
                    .get(NATIVE_HOME_ANNOTATION)
                    .is_none()
            );
            let fresh_version =
                fixture.state.lock().unwrap().rows[&path]["metadata"]["resourceVersion"].clone();
            if replaced_claim {
                fixture.edit("sandboxclaims", &claim.name, |row| {
                    row["metadata"]["uid"] = json!("replacement")
                });
                let after_conflict = fixture.state.lock().unwrap().calls.len();
                assert!(matches!(
                    fixture.client.adopt_home(&claim).await,
                    Err(crate::AdapterError::Rejected(_))
                ));
                assert!(fixture.state.lock().unwrap().calls[after_conflict..]
                    .iter()
                    .all(|(call, _)| call.starts_with("GET ")));
                continue;
            }
            let home = fixture.client.adopt_home(&claim).await.unwrap();
            assert_eq!(fixture.client.adopt_home(&claim).await.unwrap(), home);
            assert_eq!(
                fixture.client.recover_home(&claim).await.unwrap(),
                Some(home)
            );
            let state = fixture.state.lock().unwrap();
            assert_eq!(
                state.rows.keys().collect::<Vec<_>>(),
                before.keys().collect::<Vec<_>>()
            );
            for (key, row) in &before {
                assert_eq!(state.rows[key]["metadata"]["uid"], row["metadata"]["uid"]);
            }
            assert_eq!(state.rows[VOLUME], volume);
            let calls = &state.calls[floor..];
            assert!(calls
                .iter()
                .all(|(call, _)| call.starts_with("GET ") || call.starts_with("PATCH ")));
            let patches: Vec<_> = calls
                .iter()
                .enumerate()
                .filter_map(|(index, (call, body))| (call == &patch).then_some((index, body)))
                .collect();
            assert_eq!(patches.len(), 2);
            assert_ne!(patches[0].1["metadata"]["resourceVersion"], fresh_version);
            assert_eq!(patches[1].1["metadata"]["resourceVersion"], fresh_version);
            assert!(calls[patches[0].0 + 1..patches[1].0]
                .iter()
                .any(|(call, _)| call == &format!("GET {path}")));
            drop(state);
            no_storage_effects(&fixture);
        }
    }
}

#[tokio::test]
async fn native_home_refuses_replaced_uid_owner_storage_or_snapshot_after_adoption() {
    for change in ["pvc", "sandbox", "claim", "owner", "storage", "snapshot"] {
        let fixture = Fixture::new().await;
        let claim = assigned_spec(&fixture, &spec()).await;
        let home = adopted(&fixture, &claim).await;
        if change == "claim" {
            fixture.edit("sandboxclaims", &claim.name, |row| {
                row["metadata"]["uid"] = json!("replacement")
            });
        } else {
            let mut state = fixture.state.lock().unwrap();
            match change {
                "pvc" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                "sandbox" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                "owner" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["ownerReferences"][0]["uid"] =
                        json!("replacement")
                }
                "storage" => {
                    state.rows.get_mut(VOLUME).unwrap()["spec"]["storageClassName"] = json!("other")
                }
                _ => {
                    state.rows.get_mut(VOLUME).unwrap()["spec"]["dataSource"] = json!({"kind":"VolumeSnapshot","apiGroup":"snapshot.storage.k8s.io","name":"unexpected"})
                }
            }
        }
        assert!(homes(&fixture).verify(&home).await.is_err(), "{change}");
        assert!(fixture.client.adopt_home(&claim).await.is_err(), "{change}");
        no_storage_effects(&fixture);
    }
}

#[tokio::test]
async fn native_runtime_keeps_vct_initializer_and_same_disk_through_pause_resume() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let original = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    let runtime = super::runtime_tests::client(&fixture, false);
    let request = super::runtime_tests::request(&home, 1);
    let handle = runtime
        .admit_pool_runtime(&claim, &home, &request)
        .await
        .unwrap();
    assert_eq!(
        runtime.provision_pool_runtime(&request).await.unwrap(),
        Some(handle.clone())
    );
    runtime.suspend(&handle, &request).await.unwrap();
    runtime.verify_suspended(&handle, &request).await.unwrap();
    let next = super::runtime_tests::request(&home, 2);
    runtime.resume(&handle, &request, &next).await.unwrap();
    runtime
        .validate_request(&handle, &next)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.state.lock().unwrap().rows[MEMBER]["spec"], original);
    assert_eq!(
        fixture.state.lock().unwrap().rows[VOLUME]["metadata"]["uid"],
        "pvc-uid"
    );
    homes(&fixture).verify(&home).await.unwrap();
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn native_cleanup_waits_for_controller_gc_and_rejects_same_name_replacement() {
    for replacement in [false, true] {
        let fixture = Fixture::new().await;
        let claim = assigned_spec(&fixture, &spec()).await;
        let home = adopted(&fixture, &claim).await;
        assert!(fixture.client.cleanup_retained_home(&home).await.is_err());
        assert!(!fixture.state.lock().unwrap().rows.contains_key(MEMBER));
        assert!(homes(&fixture)
            .remove(&home.request, Some(&home.receipt))
            .await
            .is_err());
        if replacement {
            fixture.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
                json!("replacement");
            assert!(fixture.client.cleanup_retained_home(&home).await.is_err());
            assert!(homes(&fixture)
                .remove(&home.request, Some(&home.receipt))
                .await
                .is_err());
        } else {
            fixture.state.lock().unwrap().rows.remove(VOLUME);
            fixture.client.cleanup_retained_home(&home).await.unwrap();
            homes(&fixture)
                .remove(&home.request, Some(&home.receipt))
                .await
                .unwrap();
        }
        no_storage_effects(&fixture);
    }
}

#[tokio::test]
async fn native_cleanup_captures_late_pvc_uid_after_member_disappears_before_effects() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let volume = fixture.state.lock().unwrap().rows.remove(VOLUME).unwrap();
    let original = NativePoolCleanup {
        request: claim.request.clone(),
        claim: Some(claim),
        members: vec![],
    };
    let old = fixture.client.plan_cleanup(&original).await.unwrap();
    assert_eq!(old.members.len(), 1);
    assert!(old.members[0].pvc_uid.is_none());
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.remove(MEMBER);
        state.rows.insert(VOLUME.into(), volume);
    }
    let before = fixture.state.lock().unwrap().calls.len();
    assert!(fixture.client.cleanup(&old).await.is_err());
    assert!(fixture.state.lock().unwrap().calls[before..]
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
    let captured = fixture.client.plan_cleanup(&old).await.unwrap();
    assert_eq!(captured.members[0].pvc_uid.as_deref(), Some("pvc-uid"));
    assert!(fixture.client.cleanup(&captured).await.is_err());
    fixture.state.lock().unwrap().rows.remove(VOLUME);
    fixture.client.cleanup(&captured).await.unwrap();
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn cold_selection_creates_zero_pool_and_preserves_existing_target_without_seed() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().rows.remove(seed_path());
    let spec = spec();
    let rendered = crate::hosted::HostedDeployment {
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
    let home = crate::HomeVolumeRequest {
        volume_id: "home-one".into(),
        sandbox_id: "sbx_one".into(),
        policy: spec.home.clone(),
        scope: homes(&fixture).allocation_scope(),
    };
    let binding = fixture
        .client
        .select_runtime(&rendered, &spec.image, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture.row("sandboxwarmpools", &binding.name)["spec"]["replicas"],
        0
    );
    let claim = fixture
        .client
        .claim(&NativePoolClaimRequest {
            pool: binding.clone(),
            home: home.clone(),
            runtime_generation: 1,
        })
        .await
        .unwrap();
    assert!(
        claim.sandbox_name.is_none(),
        "controller cold fallback supplies the member"
    );
    fixture.client.resize(&binding, 7).await.unwrap();
    assert_eq!(
        fixture
            .client
            .select_runtime(&rendered, &spec.image, &home)
            .await
            .unwrap(),
        Some(binding.clone())
    );
    assert_eq!(
        fixture.row("sandboxwarmpools", &binding.name)["spec"]["replicas"],
        7
    );
    no_storage_effects(&fixture);
}

#[test]
fn new_policy_is_versioned_and_legacy_policy_and_receipt_serialization_are_unchanged() {
    let legacy = json!({"storage_class":"disk","capacity_gib":4,"template_name":"seed","template_generation":"seed-uid"});
    let decoded: crate::HomeVolumePolicy = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
    let serialized = serde_json::to_value(spec().home).unwrap();
    assert_eq!(serialized["mode"], "native_vct_empty_v1");
    assert!(serialized.get("template_name").is_none());
    let old = json!({"volume_id":"home","request_sha256":"0".repeat(64),"storage_reference":"pvc","storage_generation":"uid"});
    let receipt: crate::HomeVolumeReceipt = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(receipt).unwrap(), old);
}

#[tokio::test]
async fn native_migration_keeps_same_cluster_legacy_inventory_and_scope_once() {
    let fixture = Fixture::new().await;
    let legacy = super::home_tests::homes(&fixture);
    let old_scope = legacy.allocation_scope().unwrap();
    let native = homes(&fixture);
    assert!(native.accepts_scope(&old_scope));
    let native_scope = native.allocation_scope().unwrap();
    assert_ne!(native_scope, old_scope);
    let native_only = crate::RoutedHomeVolumes::new(
        None,
        Some((std::sync::Arc::new(native.clone()), spec().home)),
    )
    .unwrap();
    assert!(native_only.accepts_scope(&old_scope));
    assert!(native_only.accepts_scope(&native_scope));
    assert!(!native_only.accepts_scope(
        &crate::HomeVolumeScope::from_authority(
            "kubernetes-pvc-v1",
            &["foreign-cluster", "cma-test", "cma-test"],
        )
        .unwrap()
    ));
    let claim = super::home_tests::assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    let routes = crate::RoutedHomeVolumes::new(
        Some((std::sync::Arc::new(legacy), super::tests::spec().home)),
        Some((std::sync::Arc::new(native), spec().home)),
    )
    .unwrap();
    assert!(routes.shares_legacy_authority());
    assert!(routes.accepts_scope(&old_scope));
    assert!(routes.accepts_scope(&native_scope));
    assert_eq!(routes.inventory().await.unwrap(), vec![home.clone()]);
    routes.verify(&home).await.unwrap();
    let new = routes.reserve_for(true, &home.request).unwrap();
    assert!(new.policy.is_native_vct());
    assert_ne!(new.computed_sha256(), home.request.computed_sha256());
    assert_eq!(home.request.scope, None);
}
