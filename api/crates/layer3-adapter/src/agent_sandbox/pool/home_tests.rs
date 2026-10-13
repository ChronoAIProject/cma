use super::fixture::Fixture;
use super::tests::{request, spec};
use super::*;
use crate::{provider::PersistentHomeVolumes, HomeVolumeStore};
use serde_json::json;

pub(super) const MEMBER: &str =
    "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/warm-member";
pub(super) const VOLUME: &str =
    "/api/v1/namespaces/cma-test/persistentvolumeclaims/home-warm-member";

pub(super) async fn assigned(fixture: &Fixture) -> NativePoolClaim {
    let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
    let original = fixture
        .client
        .claim(&request(binding.clone()))
        .await
        .unwrap();
    fixture.edit("sandboxclaims", &original.name, |row| {
        row["status"] = json!({"sandbox":{"name":"warm-member"}})
    });
    let mut blueprint = fixture.row("sandboxtemplates", &binding.template_name)["spec"].clone();
    blueprint
        .as_object_mut()
        .unwrap()
        .remove("volumeClaimTemplatesPolicy");
    let mut volume = blueprint["volumeClaimTemplates"][0].clone();
    volume["apiVersion"] = json!("v1");
    volume["kind"] = json!("PersistentVolumeClaim");
    volume["metadata"]["name"] = json!("home-warm-member");
    volume["metadata"]["namespace"] = json!("cma-test");
    volume["metadata"]["uid"] = json!("pvc-uid");
    volume["metadata"]["resourceVersion"] = json!("51");
    volume["metadata"]["ownerReferences"] = json!([{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true,"blockOwnerDeletion":true}]);
    let member = json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
        "metadata":{"name":"warm-member","namespace":"cma-test","uid":"member-uid","resourceVersion":"50","generation":1,
            "ownerReferences":[{"apiVersion":API_VERSION,"kind":"SandboxClaim","name":original.name,"uid":original.uid,"controller":true,"blockOwnerDeletion":true}]},
        "spec":blueprint,"status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}});
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.insert(MEMBER.into(), member);
        state.rows.insert(VOLUME.into(), volume);
    }
    fixture.client.observe_claim(&original).await.unwrap()
}

fn member_observed(fixture: &Fixture) {
    let mut state = fixture.state.lock().unwrap();
    let row = state.rows.get_mut(MEMBER).unwrap();
    row["status"]["conditions"][0]["observedGeneration"] = row["metadata"]["generation"].clone();
}

pub(super) async fn adopted(
    fixture: &Fixture,
    claim: &NativePoolClaim,
) -> crate::HomeVolumeAttachment {
    for _ in 0..5 {
        match fixture.client.adopt_home(claim).await {
            Ok(home) => return home,
            Err(crate::AdapterError::Unavailable(_)) => member_observed(fixture),
            Err(error) => panic!("unexpected adoption refusal: {error}"),
        }
    }
    panic!("adoption did not settle");
}

pub(super) fn homes(fixture: &Fixture) -> PersistentHomeVolumes {
    PersistentHomeVolumes::namespaced(
        fixture.kubernetes.clone(),
        "cma-test",
        "cma-test",
        "fixture",
    )
    .unwrap()
}

#[tokio::test]
async fn adoption_preserves_member_and_volume_uids_and_waits_for_controller_before_detaching_disk()
{
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let before = fixture.state.lock().unwrap().rows[MEMBER]["spec"]["podTemplate"]["spec"].clone();
    assert!(matches!(
        fixture.client.adopt_home(&claim).await,
        Err(crate::AdapterError::Unavailable(_))
    ));
    assert!(matches!(
        fixture.client.adopt_home(&claim).await,
        Err(crate::AdapterError::Unavailable(_))
    ));
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(
            state.rows[VOLUME]["metadata"]["ownerReferences"][0]["uid"],
            "member-uid"
        );
        assert!(state
            .calls
            .iter()
            .all(|(c, _)| c != &format!("PATCH {VOLUME}")));
        let mut changed = state.rows[MEMBER]["spec"]["podTemplate"]["spec"].clone();
        assert_eq!(
            changed["volumes"],
            json!([{"name":"home","persistentVolumeClaim":{"claimName":"home-warm-member","readOnly":false}}])
        );
        changed.as_object_mut().unwrap().remove("volumes");
        assert_eq!(
            changed, before,
            "container, mounts and runtime configuration stay identical"
        );
    }
    member_observed(&fixture);
    let home = fixture.client.adopt_home(&claim).await.unwrap();
    assert_eq!(home.receipt.storage_reference, "home-warm-member");
    assert_eq!(home.receipt.storage_generation, "pvc-uid");
    assert_eq!(fixture.client.adopt_home(&claim).await.unwrap(), home);
    homes(&fixture).verify(&home).await.unwrap();
    assert_eq!(
        homes(&fixture).inventory().await.unwrap(),
        std::slice::from_ref(&home)
    );
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.rows[MEMBER]["metadata"]["uid"], "member-uid");
        assert_eq!(state.rows[VOLUME]["metadata"]["ownerReferences"], json!([]));
        assert!(!state
            .calls
            .iter()
            .any(|(call, _)| call.contains("/pods") || call.starts_with("DELETE ")));
        drop(state);
        let claim_record = fixture.row("sandboxclaims", &claim.name);
        let origin: NativePoolHomeOrigin = serde_json::from_str(
            claim_record["metadata"]["annotations"][NATIVE_HOME_ANNOTATION]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        origin
            .validate_for("cma-test", "cma-test", &home.request)
            .unwrap();
    }
    // Retirement can delete the claim, then release the retained PVC by its
    // recorded name. No deterministic cma-home-* replacement is created.
    fixture.client.delete_claim(&claim).await.unwrap();
    // Model completed foreground GC; the lifecycle layer must prove this before
    // invoking the storage port's independently UID-fenced deletion.
    fixture.state.lock().unwrap().rows.remove(MEMBER);
    homes(&fixture)
        .remove(&home.request, Some(&home.receipt))
        .await
        .unwrap();
    assert!(!fixture.state.lock().unwrap().rows.contains_key(VOLUME));
}

#[tokio::test]
async fn adoption_recovers_lost_claim_sandbox_and_pvc_patch_responses_without_replacing_storage() {
    for phase in ["claim", "sandbox", "pvc"] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        let path = match phase {
            "claim" => format!("{}/sandboxclaims/{}", super::fixture::prefix(), claim.name),
            "sandbox" => MEMBER.into(),
            _ => VOLUME.into(),
        };
        fixture
            .state
            .lock()
            .unwrap()
            .lose
            .push(format!("PATCH {path}"));
        let home = adopted(&fixture, &claim).await;
        homes(&fixture).verify(&home).await.unwrap();
        let state = fixture.state.lock().unwrap();
        for target in [MEMBER, VOLUME] {
            assert_eq!(
                state
                    .calls
                    .iter()
                    .filter(|(c, _)| c == &format!("PATCH {target}"))
                    .count(),
                1,
                "{phase}"
            );
        }
        assert_eq!(home.receipt.storage_generation, "pvc-uid");
    }
}

#[tokio::test]
async fn existing_claim_and_home_keep_original_pod_policy_when_deployment_defaults_change() {
    let mut fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    fixture.client.config.runtime_class = Some("different-class".into());
    fixture.client.config.service_account = "new-runtime-service".into();
    fixture.client.config.image_pull_secrets = vec!["new-pull-secret".into()];
    assert_eq!(
        fixture.client.claim(&claim.request).await.unwrap().uid,
        claim.uid
    );
    let home = adopted(&fixture, &claim).await;
    homes(&fixture).verify(&home).await.unwrap();
    assert_eq!(
        fixture.state.lock().unwrap().rows[MEMBER]["spec"]["podTemplate"]["spec"]
            ["runtimeClassName"],
        "gvisor"
    );
}

#[tokio::test]
async fn adoption_rejects_replaced_or_foreign_physical_resources_and_pvc_policy_drift() {
    for change in [
        "sandbox_uid",
        "pvc_uid",
        "pvc_owner",
        "pvc_policy",
        "sandbox_image",
        "sandbox_arguments",
        "sandbox_privileged",
        "claim_uid",
    ] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        assert!(fixture.client.adopt_home(&claim).await.is_err());
        member_observed(&fixture);
        {
            let mut state = fixture.state.lock().unwrap();
            match change {
                "sandbox_uid" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                "pvc_uid" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                "pvc_owner" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["ownerReferences"][0]["uid"] =
                        json!("foreign")
                }
                "pvc_policy" => {
                    state.rows.get_mut(VOLUME).unwrap()["spec"]["dataSource"]["name"] =
                        json!("other-seed")
                }
                "sandbox_image" => {
                    state.rows.get_mut(MEMBER).unwrap()["spec"]["podTemplate"]["spec"]
                        ["containers"][0]["image"] = json!("foreign:tag")
                }
                "sandbox_arguments" => {
                    state.rows.get_mut(MEMBER).unwrap()["spec"]["podTemplate"]["spec"]
                        ["containers"][0]["args"] = json!(["--unexpected"])
                }
                "sandbox_privileged" => {
                    state.rows.get_mut(MEMBER).unwrap()["spec"]["podTemplate"]["spec"]
                        ["containers"][0]["securityContext"]["privileged"] = json!(true)
                }
                _ => {}
            }
        }
        if change == "claim_uid" {
            fixture.edit("sandboxclaims", &claim.name, |v| {
                v["metadata"]["uid"] = json!("replacement")
            });
        }
        assert!(fixture.client.adopt_home(&claim).await.is_err(), "{change}");
        assert!(
            fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(c, _)| c != &format!("PATCH {VOLUME}")),
            "{change}"
        );
    }
}

#[tokio::test]
async fn retained_home_store_requires_original_pool_origin_before_verification_or_deletion() {
    for change in ["uid", "origin", "request"] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        let home = adopted(&fixture, &claim).await;
        {
            let mut state = fixture.state.lock().unwrap();
            let volume = state.rows.get_mut(VOLUME).unwrap();
            match change {
                "uid" => volume["metadata"]["uid"] = json!("replacement"),
                "origin" => {
                    volume["metadata"]["annotations"]
                        .as_object_mut()
                        .unwrap()
                        .remove(NATIVE_HOME_ANNOTATION);
                }
                _ => {
                    let mut request = home.request.clone();
                    request.sandbox_id = "other".into();
                    volume["metadata"]["annotations"]["cma/home-allocation"] =
                        json!(serde_json::to_string(&request).unwrap());
                }
            }
        }
        assert!(homes(&fixture).verify(&home).await.is_err(), "{change}");
        assert!(
            homes(&fixture)
                .remove(&home.request, Some(&home.receipt))
                .await
                .is_err(),
            "{change}"
        );
        assert!(!fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(c, _)| c.starts_with("DELETE ")));
    }
}
