use super::fixture::{seed_path, Fixture};
use super::*;
use serde_json::json;
use std::collections::BTreeMap;

pub(super) fn spec() -> NativePoolSpec {
    NativePoolSpec {
        cold_canary: false,
        image: crate::RuntimeImage {
            uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
            labels: BTreeMap::from([
                (crate::RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                (
                    crate::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL.into(),
                    "1".into(),
                ),
            ]),
        },
        cpu_millis: 750,
        memory_mib: 1536,
        home: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "cma-home".into(),
            capacity_gib: 4,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    }
}

pub(super) fn request(binding: NativePoolBinding) -> NativePoolClaimRequest {
    NativePoolClaimRequest {
        home: crate::HomeVolumeRequest {
            scope: binding.spec.home.is_native_vct().then(|| {
                crate::HomeVolumeScope::from_authority(
                    "native-vct-empty-v1",
                    &["fixture", "cma-test", "cma-test"],
                )
                .unwrap()
            }),
            volume_id: "home-one".into(),
            sandbox_id: "sbx_one".into(),
            policy: binding.spec.home.clone(),
        },
        pool: binding,
        runtime_generation: 1,
    }
}

#[test]
fn sandbox_spec_comparison_accepts_kubernetes_canonicalization_only_for_defaults_and_quantities() {
    let desired = json!({
        "hostNetwork": false,
        "hostPID": false,
        "containers": [{
            "resources": {
                "requests": {"cpu": "1000m", "memory": "1024Mi"},
                "limits": {"cpu": "1000m", "memory": "1024Mi"}
            }
        }]
    });
    let actual = json!({
        "containers": [{
            "resources": {
                "requests": {"cpu": "1", "memory": "1Gi"},
                "limits": {"cpu": "1", "memory": "1Gi"}
            }
        }]
    });
    assert!(model::contains(&actual, &desired));

    let mut changed = actual.clone();
    changed["containers"][0]["resources"]["limits"]["memory"] = json!("2Gi");
    assert!(!model::contains(&changed, &desired));

    let mut identity = actual;
    identity["containers"][0]["resources"]["requests"]["cpu"] = json!("1000");
    assert!(!model::contains(&identity, &desired));
}

#[tokio::test]
async fn pool_prewarms_independent_seeded_disks_without_tenant_identity_and_replays_lost_writes() {
    let fixture = Fixture::new().await;
    fixture.lost("POST", "sandboxtemplates");
    fixture.lost("POST", "sandboxwarmpools");
    let pool = fixture.client.configure(&spec(), 2).await.unwrap();
    assert!(!pool.observed);
    assert_eq!(pool.ready_replicas, 0);
    let template = fixture.row("sandboxtemplates", &pool.binding.template_name);
    assert_eq!(
        template["spec"]["podTemplate"]["spec"]["automountServiceAccountToken"],
        false
    );
    let container = &template["spec"]["podTemplate"]["spec"]["containers"][0];
    assert_eq!(
        container["command"],
        json!(["/usr/local/bin/layer4-runtime", "--wait-for-assignment"])
    );
    assert_eq!(container["env"].as_array().unwrap().len(), 1);
    assert_eq!(
        container["env"][0]["valueFrom"]["fieldRef"]["fieldPath"],
        "metadata.uid"
    );
    assert!(container["readinessProbe"].get("tcpSocket").is_none());
    assert_eq!(container["readinessProbe"]["exec"]["command"][1], "-S");
    assert!(container["volumeMounts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v["name"] == "home"));
    assert!(template["spec"]["podTemplate"]["spec"]
        .get("volumes")
        .is_none());
    let volume = &template["spec"]["volumeClaimTemplates"][0];
    assert_eq!(volume["metadata"]["name"], "home");
    assert_eq!(volume["spec"]["dataSource"]["name"], "seed");
    assert_eq!(
        volume["metadata"]["annotations"]["cma/pool-seed-uid"],
        "seed-uid"
    );
    assert_eq!(volume["spec"]["accessModes"], json!(["ReadWriteOncePod"]));
    assert!(!template
        .to_string()
        .contains(&fixture.client.config.control_key));
    assert!(!template
        .to_string()
        .contains("CHRONO_RUNTIME_CONTROL_TOKEN"));
    assert!(!template.to_string().contains("CHRONO_SANDBOX_ID"));
    assert_eq!(fixture.client.configure(&spec(), 2).await.unwrap(), pool);

    fixture.lost("PATCH", "sandboxwarmpools");
    let scaled = fixture.client.configure(&spec(), 4).await.unwrap();
    assert_eq!(scaled.binding, pool.binding);
    assert_eq!(scaled.desired_replicas, 4);
    fixture.edit("sandboxwarmpools", &scaled.binding.name, |row| {
        row["status"] = json!({"observedGeneration":1,"readyReplicas":4})
    });
    assert_eq!(
        fixture
            .client
            .configure(&spec(), 4)
            .await
            .unwrap()
            .ready_replicas,
        0
    );
    fixture.edit("sandboxwarmpools", &scaled.binding.name, |row| {
        row["status"]["observedGeneration"] = row["metadata"]["generation"].clone()
    });
    assert_eq!(
        fixture
            .client
            .configure(&spec(), 4)
            .await
            .unwrap()
            .ready_replicas,
        4
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows.len(), 3);
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(c, _)| c.starts_with("POST "))
            .count(),
        2
    );
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(c, _)| c.starts_with("PATCH "))
            .count(),
        1
    );
}

#[tokio::test]
async fn pool_refuses_unverified_images_seed_replacements_foreign_objects_and_spec_drift() {
    let fixture = Fixture::new().await;
    let mut invalid = spec();
    invalid
        .image
        .labels
        .remove(crate::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL);
    assert!(fixture.client.configure(&invalid, 1).await.is_err());
    assert!(fixture.client.configure(&spec(), 1001).await.is_err());
    assert!(fixture.state.lock().unwrap().calls.is_empty());
    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(seed_path())
        .unwrap()["metadata"]["uid"] = json!("replacement");
    assert!(fixture.client.configure(&spec(), 1).await.is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(c, _)| c.starts_with("GET ")));

    for field in ["owner", "image", "template_uid", "seed_uid"] {
        let fixture = Fixture::new().await;
        let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
        match field {
            "owner" => fixture.edit("sandboxwarmpools", &binding.name, |v| {
                v["metadata"]["labels"][OWNER_LABEL] = json!("foreign")
            }),
            "image" => fixture.edit("sandboxtemplates", &binding.template_name, |v| {
                v["spec"]["podTemplate"]["spec"]["containers"][0]["image"] = json!("foreign:tag")
            }),
            "template_uid" => fixture.edit("sandboxtemplates", &binding.template_name, |v| {
                v["metadata"]["uid"] = json!("replacement")
            }),
            _ => {
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .rows
                    .get_mut(seed_path())
                    .unwrap()["metadata"]["uid"] = json!("replacement")
            }
        }
        let before = fixture.state.lock().unwrap().calls.len();
        assert!(
            fixture.client.configure(&spec(), 3).await.is_err(),
            "{field}"
        );
        assert!(fixture.state.lock().unwrap().calls[before..]
            .iter()
            .all(|(c, _)| c.starts_with("GET ")));
    }
}

#[tokio::test]
async fn claim_has_no_cold_overrides_and_pins_receipted_identity_across_retries_and_deletion() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 2).await.unwrap().binding;
    let request = request(binding);
    fixture.lost("POST", "sandboxclaims");
    let original = fixture.client.claim(&request).await.unwrap();
    assert_eq!(fixture.client.claim(&request).await.unwrap(), original);
    assert_eq!(
        fixture.row("sandboxclaims", &original.name)["spec"],
        json!({"warmPoolRef":{"name":request.pool.name}})
    );
    fixture.edit("sandboxclaims", &original.name, |v| {
        v["status"] = json!({"sandbox":{"name":"member-one"}})
    });
    let assigned = fixture.client.observe_claim(&original).await.unwrap();
    assert_eq!(assigned.sandbox_name.as_deref(), Some("member-one"));
    fixture.edit("sandboxclaims", &original.name, |v| {
        v["status"]["sandbox"]["name"] = json!("other-member")
    });
    assert!(fixture.client.observe_claim(&assigned).await.is_err());
    assert!(fixture.client.delete_claim(&assigned).await.is_err());
    fixture.edit("sandboxclaims", &original.name, |v| {
        v["status"]["sandbox"]["name"] = json!("member-one")
    });

    // Existing claim recovery needs its recorded authority, even after pool scale-down.
    fixture.client.configure(&spec(), 0).await.unwrap();
    assert_eq!(
        fixture.client.claim(&request).await.unwrap().uid,
        original.uid
    );
    fixture.client.delete_claim(&assigned).await.unwrap();
    fixture.client.delete_claim(&assigned).await.unwrap();
    assert!(fixture.client.observe_claim(&assigned).await.is_err());
    let state = fixture.state.lock().unwrap();
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(c, _)| c.starts_with("POST ") && c.ends_with("/sandboxclaims"))
            .count(),
        1
    );
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(c, _)| c.starts_with("DELETE "))
            .count(),
        1
    );
}

#[tokio::test]
async fn pool_can_scale_to_zero_after_seed_retirement_but_cannot_allocate_new_claims() {
    let fixture = Fixture::new().await;
    let original = fixture.client.configure(&spec(), 2).await.unwrap();
    fixture.state.lock().unwrap().rows.remove(seed_path());
    let stopped = fixture.client.configure(&spec(), 0).await.unwrap();
    assert_eq!(stopped.binding, original.binding);
    assert_eq!(stopped.desired_replicas, 0);
    assert!(fixture
        .client
        .claim(&request(stopped.binding))
        .await
        .is_err());
    assert!(fixture.client.configure(&spec(), 1).await.is_err());
}

#[tokio::test]
async fn claim_rejects_replaced_pool_or_claim_and_changed_admission_without_allocating_again() {
    for change in [
        "pool_uid",
        "template_uid",
        "claim_uid",
        "environment",
        "request",
    ] {
        let fixture = Fixture::new().await;
        let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
        let mut request = request(binding.clone());
        if change == "pool_uid" || change == "template_uid" {
            let (kind, name) = if change == "pool_uid" {
                ("sandboxwarmpools", &binding.name)
            } else {
                ("sandboxtemplates", &binding.template_name)
            };
            fixture.edit(kind, name, |v| v["metadata"]["uid"] = json!("replacement"));
            assert!(fixture.client.claim(&request).await.is_err());
            assert!(fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(c, _)| !c.ends_with("/sandboxclaims") || !c.starts_with("POST ")));
            continue;
        }
        let original = fixture.client.claim(&request).await.unwrap();
        match change {
            "claim_uid" => {
                fixture.edit("sandboxclaims", &original.name, |v| {
                    v["metadata"]["uid"] = json!("replacement")
                });
                assert!(fixture.client.observe_claim(&original).await.is_err());
                assert!(fixture.client.delete_claim(&original).await.is_err());
            }
            "environment" => {
                fixture.edit("sandboxclaims", &original.name, |v| {
                    v["spec"]["env"] = json!([{"name":"TENANT","value":"other"}])
                });
                assert!(fixture.client.claim(&request).await.is_err());
            }
            _ => {
                request.home.volume_id = "other-home".into();
                assert!(fixture.client.claim(&request).await.is_err());
            }
        }
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(c, _)| c.starts_with("POST ") && c.ends_with("/sandboxclaims"))
                .count(),
            1
        );
    }
}
