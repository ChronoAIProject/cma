use super::{
    fixture::{prefix, Fixture},
    home_tests::{adopted, assigned_spec, MEMBER, VOLUME},
    read_fixture::{Effects, Gate},
    retirement::RETIRING,
    *,
};
use crate::{HomeVolumeAttachment, HomeVolumeMode};
use serde_json::json;
use std::collections::BTreeMap;

async fn fixture(network: bool) -> (Fixture, HomeVolumeAttachment) {
    let mut fixture = Fixture::new().await;
    if network {
        fixture.client.config.template_network_policy =
            Some(network_policy_tests::policy("10.254.0.0/16"));
    }
    let mut spec = tests::spec();
    spec.home.mode = HomeVolumeMode::NativeVctEmptyV1;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    let claim = assigned_spec(&fixture, &spec).await;
    let home = adopted(&fixture, &claim).await;
    fixture
        .client
        .begin_retirement(&claim.request.pool)
        .await
        .unwrap();
    // Model an already-zero pool. The drain path may inspect it without
    // requesting controller deletion.
    fixture.edit("sandboxwarmpools", &claim.request.pool.name, |pool| {
        pool["spec"]["replicas"] = json!(0);
    });
    fixture.state.lock().unwrap().calls.clear();
    (fixture, home)
}

fn binding(home: &HomeVolumeAttachment) -> &NativePoolBinding {
    &home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .pool
}

fn claim_path(home: &HomeVolumeAttachment) -> String {
    format!(
        "{}/sandboxclaims/{}",
        prefix(),
        home.receipt.native_authority.as_ref().unwrap().claim.name
    )
}

async fn drain(fixture: &Fixture, home: &HomeVolumeAttachment) -> Result<()> {
    fixture
        .client
        .drain_pool(binding(home), std::slice::from_ref(home), &[])
        .await
}

fn no_writes(fixture: &Fixture) {
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}

fn remove_graph(fixture: &Fixture, home: &HomeVolumeAttachment) {
    let mut state = fixture.state.lock().unwrap();
    for path in [claim_path(home), MEMBER.into(), VOLUME.into()] {
        assert!(state.rows.remove(&path).is_some());
    }
}

fn pod() -> serde_json::Value {
    json!({"apiVersion":"v1","kind":"Pod","metadata":{
        "name":"retained-pod","namespace":"cma-test","uid":"retained-pod-uid","resourceVersion":"210",
        "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]}})
}

fn unchanged(fixture: &Fixture, before: BTreeMap<String, serde_json::Value>) {
    no_writes(fixture);
    assert_eq!(fixture.state.lock().unwrap().rows, before);
}

#[tokio::test]
async fn drain_preserves_unupgraded_and_network_dependent_homes_and_all_definitions() {
    for network in [false, true] {
        let (fixture, home) = fixture(network).await;
        {
            let mut state = fixture.state.lock().unwrap();
            state.rows.insert(
                "/api/v1/namespaces/cma-test/pods/retained-pod".into(),
                pod(),
            );
            state.rows.insert(format!("/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/{}-spare", binding(&home).name),
                json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","metadata":{"name":format!("{}-spare",binding(&home).name),"namespace":"cma-test","uid":"spare-uid","resourceVersion":"210"}}));
        }
        let before = fixture.state.lock().unwrap().rows.clone();
        drain(&fixture, &home).await.unwrap();
        drain(&fixture, &home).await.unwrap();
        unchanged(&fixture, before);
        fixture.client.prove_native_home(&home).await.unwrap();
        assert!(fixture
            .client
            .retire_pool(binding(&home), std::slice::from_ref(&home))
            .await
            .is_err());
        assert!(fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(call, _)| !call.starts_with("DELETE ")));
    }
}

#[tokio::test]
async fn drain_accepts_only_complete_historical_graph_absence_without_mutating_the_record() {
    let (fixture, home) = fixture(true).await;
    let original = home.clone();
    remove_graph(&fixture, &home);
    let before = fixture.state.lock().unwrap().rows.clone();
    drain(&fixture, &home).await.unwrap();
    unchanged(&fixture, before);
    assert_eq!(home, original);
}

#[tokio::test]
async fn drain_rechecks_original_names_after_an_absent_inventory() {
    let (fixture, home) = fixture(false).await;
    let path = claim_path(&home);
    let original = fixture.state.lock().unwrap().rows[&path].clone();
    remove_graph(&fixture, &home);
    let gate = Gate::new();
    let effects = Effects::new(&fixture, BTreeMap::from([(path.clone(), gate.clone())]));
    let pools = effects.pools(&fixture);
    let task = tokio::spawn(async move {
        pools
            .drain_pool(binding(&home), std::slice::from_ref(&home), &[])
            .await
    });
    gate.wait().await;
    fixture.state.lock().unwrap().rows.insert(path, original);
    gate.respond();
    assert!(task.await.unwrap().is_err());
    effects.assert_read_only();
}

#[tokio::test]
async fn drain_rejects_partial_absence_replacement_and_duplicate_home_without_writes() {
    for change in [
        "claim_missing",
        "member_missing",
        "volume_missing",
        "only_claim",
        "only_member",
        "only_volume",
        "claim_replaced",
        "member_replaced",
        "volume_replaced",
        "duplicate",
    ] {
        let (fixture, home) = fixture(false).await;
        let paths = [claim_path(&home), MEMBER.into(), VOLUME.into()];
        {
            let mut state = fixture.state.lock().unwrap();
            let index = match change {
                "claim_missing" | "claim_replaced" | "only_claim" => 0,
                "member_missing" | "member_replaced" | "only_member" => 1,
                _ => 2,
            };
            if change.ends_with("_missing") {
                state.rows.remove(&paths[index]);
            } else if change.starts_with("only_") {
                for (i, path) in paths.iter().enumerate() {
                    if i != index {
                        state.rows.remove(path);
                    }
                }
            } else if change.ends_with("_replaced") {
                state.rows.get_mut(&paths[index]).unwrap()["metadata"]["uid"] =
                    json!("replacement-uid");
            }
        }
        let homes = if change == "duplicate" {
            vec![home.clone(), home.clone()]
        } else {
            vec![home.clone()]
        };
        assert!(
            fixture
                .client
                .drain_pool(binding(&home), &homes, &[])
                .await
                .is_err(),
            "{change}"
        );
        no_writes(&fixture);
    }
}

#[tokio::test]
async fn drain_rejects_absent_graph_with_lingering_pod_or_extra_original_dependents() {
    for kind in ["pod", "labelled_pod", "member", "volume"] {
        let (fixture, home) = fixture(false).await;
        let claim_uid = home
            .receipt
            .native_authority
            .as_ref()
            .unwrap()
            .claim
            .uid
            .clone();
        let claim_name = home
            .receipt
            .native_authority
            .as_ref()
            .unwrap()
            .claim
            .name
            .clone();
        let extra_volume = fixture.state.lock().unwrap().rows[VOLUME].clone();
        remove_graph(&fixture, &home);
        {
            let mut state = fixture.state.lock().unwrap();
            match kind {
                "pod" | "labelled_pod" => {
                    let mut value = pod();
                    if kind == "labelled_pod" {
                        value["metadata"]
                            .as_object_mut()
                            .unwrap()
                            .remove("ownerReferences");
                        value["metadata"]["labels"] = json!({super::super::pods::TRACKING_LABEL:super::super::pods::name_hash("warm-member")});
                    }
                    state.rows.insert(
                        "/api/v1/namespaces/cma-test/pods/retained-pod".into(),
                        value,
                    );
                }
                "member" => {
                    state.rows.insert("/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/extra".into(), json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","metadata":{"name":"extra","namespace":"cma-test","uid":"extra-uid","resourceVersion":"210","ownerReferences":[{"apiVersion":API_VERSION,"kind":"SandboxClaim","name":claim_name,"uid":claim_uid,"controller":true}]}}));
                }
                _ => {
                    let mut value = extra_volume;
                    value["metadata"]["name"] = json!("extra-home");
                    value["metadata"]["uid"] = json!("extra-home-uid");
                    state.rows.insert(
                        "/api/v1/namespaces/cma-test/persistentvolumeclaims/extra-home".into(),
                        value,
                    );
                }
            }
        }
        assert!(drain(&fixture, &home).await.is_err(), "{kind}");
        no_writes(&fixture);
    }
}

#[tokio::test]
async fn drain_rejects_unrecorded_claims_even_when_live_pool_reference_changed() {
    for kind in ["pool", "template", "recorded", "missing_authority"] {
        let (fixture, home) = fixture(false).await;
        let pool = binding(&home);
        let mut value = json!({"apiVersion":API_VERSION,"kind":"SandboxClaim","metadata":{"name":"pending","namespace":"cma-test","uid":"pending-uid","resourceVersion":"210"},"spec":{}});
        match kind {
            "pool" => value["spec"]["warmPoolRef"] = json!({"name":pool.name}),
            "template" => value["spec"]["sandboxTemplateRef"] = json!({"name":pool.template_name}),
            "recorded" => {
                value["spec"]["warmPoolRef"] = json!({"name":"another-pool"});
                value["metadata"]["annotations"] = json!({"cma/pool-claim-request":serde_json::to_string(&home.receipt.native_authority.as_ref().unwrap().claim.request).unwrap()});
            }
            _ => value["metadata"]["labels"] = json!({OWNER_LABEL:"cma-test"}),
        }
        fixture
            .state
            .lock()
            .unwrap()
            .rows
            .insert(format!("{}/sandboxclaims/pending", prefix()), value);
        assert!(drain(&fixture, &home).await.is_err(), "{kind}");
        no_writes(&fixture);
        assert_eq!(
            fixture.row("sandboxwarmpools", &pool.name)["spec"]["replicas"],
            0
        );
    }
}

#[tokio::test]
async fn drain_rechecks_late_claim_and_marker_changes_before_any_target_patch() {
    for change in ["claim", "marker", "positive-target"] {
        let (fixture, home) = fixture(false).await;
        let gate = Gate::new();
        let effects = Effects::new(
            &fixture,
            BTreeMap::from([(format!("{}/sandboxclaims", prefix()), gate.clone())]),
        );
        let pools = effects.pools(&fixture);
        let owned = home.clone();
        let task = tokio::spawn(async move {
            pools
                .drain_pool(binding(&owned), std::slice::from_ref(&owned), &[])
                .await
        });
        gate.wait().await;
        if change == "claim" {
            fixture.state.lock().unwrap().rows.insert(format!("{}/sandboxclaims/late", prefix()), json!({"apiVersion":API_VERSION,"kind":"SandboxClaim","metadata":{"name":"late","namespace":"cma-test","uid":"late-uid","resourceVersion":"210"},"spec":{"warmPoolRef":{"name":binding(&home).name}}}));
        } else {
            fixture.edit("sandboxwarmpools", &binding(&home).name, |row| {
                if change == "marker" {
                    row["metadata"]["annotations"]
                        .as_object_mut()
                        .unwrap()
                        .remove(RETIRING);
                } else {
                    row["spec"]["replicas"] = json!(1);
                }
            });
        }
        gate.respond();
        assert!(task.await.unwrap().is_err(), "{change}");
        effects.assert_read_only();
    }
}

#[tokio::test]
async fn drain_requires_marker_complete_inventory_and_original_definitions() {
    for change in ["marker", "inventory", "pool_uid", "template_uid"] {
        let (fixture, home) = fixture(false).await;
        match change {
            "marker" => fixture.edit("sandboxwarmpools", &binding(&home).name, |row| { row["metadata"]["annotations"].as_object_mut().unwrap().remove(RETIRING); }),
            "pool_uid" => fixture.edit("sandboxwarmpools", &binding(&home).name, |row| row["metadata"]["uid"] = json!("other-pool")),
            "template_uid" => fixture.edit("sandboxtemplates", &binding(&home).template_name, |row| row["metadata"]["uid"] = json!("other-template")),
            _ => fixture.state.lock().unwrap().list_replies.push_back((200, json!({"apiVersion":API_VERSION,"kind":"SandboxClaimList","metadata":{},"items":[]}))),
        }
        assert!(drain(&fixture, &home).await.is_err(), "{change}");
        no_writes(&fixture);
    }
}

#[tokio::test]
async fn drain_does_not_recreate_an_absent_pool() {
    let (fixture, home) = fixture(false).await;
    let path = format!("{}/sandboxwarmpools/{}", prefix(), binding(&home).name);
    fixture.state.lock().unwrap().rows.remove(&path);
    let before = fixture.state.lock().unwrap().rows.clone();
    drain(&fixture, &home).await.unwrap();
    unchanged(&fixture, before);
}

#[tokio::test]
async fn positive_retirement_target_never_delegates_deletion_to_the_controller() {
    let fixture = Fixture::new().await;
    let claim = home_tests::assigned(&fixture).await;
    let pool = &claim.request.pool;
    fixture.client.begin_retirement(pool).await.unwrap();
    let before = {
        let mut state = fixture.state.lock().unwrap();
        let member = state.rows.get_mut(MEMBER).unwrap();
        member["spec"]["podTemplate"]["metadata"]["labels"]["agents.x-k8s.io/claim-uid"] =
            json!(claim.uid);
        state.calls.clear();
        state.rows.clone()
    };
    for _ in 0..2 {
        let result = fixture
            .client
            .drain_pool(pool, &[], std::slice::from_ref(&claim))
            .await;
        assert!(matches!(result, Err(crate::AdapterError::Unsupported(_))));
    }
    unchanged(&fixture, before);
}

#[tokio::test]
async fn spare_drain_requests_the_provider_without_downscaling_and_replays_a_lost_response() {
    let (fixture, home) = fixture(false).await;
    let pool = binding(&home);
    let request = "cma/native-pool-spare-drain-v1";
    fixture.edit("sandboxwarmpools", &pool.name, |row| {
        row["spec"]["replicas"] = json!(2);
    });
    fixture.lost("PATCH", "sandboxwarmpools");
    for _ in 0..2 {
        assert!(fixture
            .client
            .request_spare_drain(pool, std::slice::from_ref(&home), &[])
            .await
            .is_err());
    }
    let row = fixture.row("sandboxwarmpools", &pool.name);
    assert_eq!(row["spec"]["replicas"], 2);
    assert_eq!(row["metadata"]["annotations"][request], pool.uid);
    {
        let state = fixture.state.lock().unwrap();
        let writes = state
            .calls
            .iter()
            .filter(|(call, _)| !call.starts_with("GET "))
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 1);
        let (call, patch) = writes[0];
        assert!(call.starts_with("PATCH "));
        assert_eq!(patch["metadata"]["uid"], pool.uid);
        assert!(patch["metadata"]["resourceVersion"].as_str().is_some());
        assert!(patch.get("spec").is_none());
    }

    fixture.edit("sandboxwarmpools", &pool.name, |row| {
        row["spec"]["replicas"] = json!(0);
        row["metadata"]["generation"] = json!(8);
        row["status"]["observedGeneration"] = json!(7);
    });
    assert!(fixture
        .client
        .request_spare_drain(pool, std::slice::from_ref(&home), &[])
        .await
        .is_err());
    fixture.edit("sandboxwarmpools", &pool.name, |row| {
        row["status"]["observedGeneration"] = json!(8);
        row["status"]["readyReplicas"] = json!(0);
    });
    fixture
        .client
        .request_spare_drain(pool, std::slice::from_ref(&home), &[])
        .await
        .unwrap();
}
