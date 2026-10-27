use super::*;
use crate::agent_sandbox::pool::{retirement::RETIRING, NativePoolBinding};

fn binding(fixture: &Resume) -> &NativePoolBinding {
    &fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .pool
}
fn path(kind: &str, name: &str) -> String {
    format!("{}/{kind}/{name}", prefix())
}
async fn upgraded() -> Resume {
    let fixture = Resume::new().await;
    upgrade(
        &fixture,
        &fixture.previous,
        &target(&fixture.home, 2, &image('b')),
    )
    .await
    .unwrap();
    fixture.fixture.state.lock().unwrap().calls.clear();
    fixture
}
async fn barrier(fixture: &Resume) {
    fixture
        .fixture
        .client
        .begin_retirement(binding(fixture))
        .await
        .unwrap();
    fixture
        .fixture
        .client
        .resize(binding(fixture), 0)
        .await
        .unwrap();
    fixture.fixture.state.lock().unwrap().calls.clear();
}
async fn retire(fixture: &Resume) -> crate::Result<bool> {
    fixture
        .fixture
        .client
        .retire_pool(binding(fixture), std::slice::from_ref(&fixture.home))
        .await
}
fn no_deletes(fixture: &Resume) {
    assert!(fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| !call.starts_with("DELETE ")));
}

#[tokio::test]
async fn retirement_barrier_blocks_new_admission_but_retains_claim_replay_and_zero_resize() {
    let fixture = Resume::new().await;
    let pool = binding(&fixture);
    fixture
        .fixture
        .lost("PATCH", &format!("sandboxwarmpools/{}", pool.name));
    fixture.fixture.client.begin_retirement(pool).await.unwrap();
    fixture.fixture.client.begin_retirement(pool).await.unwrap();
    let marked = fixture.fixture.row("sandboxwarmpools", &pool.name);
    assert_eq!(marked["metadata"]["annotations"][RETIRING], "true");
    assert!(fixture.fixture.client.resize(pool, 2).await.is_err());
    assert!(fixture
        .fixture
        .client
        .configure(&pool.spec, 2)
        .await
        .is_err());
    let mut request = fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .clone();
    assert!(fixture.fixture.client.claim(&request).await.is_ok());
    request.runtime_generation += 1;
    assert!(fixture.fixture.client.claim(&request).await.is_err());
    assert!(fixture
        .fixture
        .client
        .select_runtime(&fixture.previous, &pool.spec.image, &fixture.home.request)
        .await
        .is_err());
    fixture.fixture.client.resize(pool, 0).await.unwrap();
    let state = fixture.fixture.state.lock().unwrap();
    assert!(state
        .calls
        .iter()
        .all(|(call, _)| !call.starts_with(&format!("POST {}/sandboxclaims", prefix()))));
    assert_eq!(
        state.rows[&path("sandboxwarmpools", &pool.name)]["spec"]["replicas"],
        0
    );
}

#[tokio::test]
async fn retirement_deletes_only_definitions_and_replays_after_both_lost_deletion_responses() {
    let fixture = upgraded().await;
    barrier(&fixture).await;
    let before = fixture.fixture.state.lock().unwrap().rows.clone();
    let pool = binding(&fixture);
    fixture
        .fixture
        .lost("DELETE", &format!("sandboxwarmpools/{}", pool.name));
    fixture.fixture.lost(
        "DELETE",
        &format!("sandboxtemplates/{}", pool.template_name),
    );
    assert!(retire(&fixture).await.unwrap());
    assert!(retire(&fixture).await.unwrap());
    fixture
        .fixture
        .client
        .claim(
            &fixture
                .home
                .receipt
                .native_authority
                .as_ref()
                .unwrap()
                .claim
                .request,
        )
        .await
        .unwrap();
    let state = fixture.fixture.state.lock().unwrap();
    for (path, row) in before {
        if !path.contains("/sandboxwarmpools/") && !path.contains("/sandboxtemplates/") {
            assert_eq!(state.rows[&path], row, "{path}");
        }
    }
    let deletes: Vec<_> = state
        .calls
        .iter()
        .filter(|(call, _)| call.starts_with("DELETE "))
        .collect();
    assert_eq!(deletes.len(), 2);
    assert!(deletes[0].0.contains("/sandboxwarmpools/"));
    assert!(deletes[1].0.contains("/sandboxtemplates/"));
}

#[tokio::test]
async fn retirement_requires_marker_zero_target_exact_homes_and_an_actual_upgrade() {
    for failure in [
        "marker",
        "target",
        "missing_home",
        "duplicate_home",
        "foreign_home",
        "unupgraded",
        "rolled_back",
        "network",
    ] {
        let fixture = if failure == "unupgraded" {
            Resume::new().await
        } else {
            upgraded().await
        };
        let pool = binding(&fixture).clone();
        if failure != "marker" {
            fixture
                .fixture
                .client
                .begin_retirement(&pool)
                .await
                .unwrap();
        }
        fixture.fixture.client.resize(&pool, 0).await.unwrap();
        if failure == "target" {
            fixture.fixture.edit("sandboxwarmpools", &pool.name, |p| {
                p["spec"]["replicas"] = json!(1)
            });
        }
        if failure == "rolled_back" {
            let next = target(&fixture.home, 2, &image('b'));
            client(&fixture.fixture, true)
                .suspend(&fixture.handle, &next)
                .await
                .unwrap();
            upgrade(
                &fixture,
                &next,
                &target(&fixture.home, 3, &pool.spec.image.uri),
            )
            .await
            .unwrap();
        }
        let mut homes = vec![fixture.home.clone()];
        if failure == "missing_home" {
            homes.clear();
        }
        if failure == "duplicate_home" {
            homes.push(fixture.home.clone());
        }
        if failure == "foreign_home" {
            homes[0]
                .receipt
                .native_authority
                .as_mut()
                .unwrap()
                .claim
                .request
                .pool
                .uid = "foreign-pool".into();
        }
        if failure == "network" {
            homes[0]
                .receipt
                .native_authority
                .as_mut()
                .unwrap()
                .claim
                .request
                .pool
                .pod_policy
                .template_network_policy = Some(
                crate::agent_sandbox::pool::network_policy_tests::policy("10.254.0.0/16"),
            );
        }
        fixture.fixture.state.lock().unwrap().calls.clear();
        let result = fixture.fixture.client.retire_pool(&pool, &homes).await;
        if failure == "target" {
            assert!(!result.unwrap());
        } else {
            assert!(result.is_err(), "{failure}");
        }
        no_deletes(&fixture);
    }
}

#[tokio::test]
async fn retirement_waits_for_spare_sandbox_pod_pvc_and_orphan_dependents() {
    for kind in ["sandbox", "pod", "pvc", "orphan_pod", "orphan_pvc"] {
        let fixture = upgraded().await;
        barrier(&fixture).await;
        let pool = binding(&fixture);
        let (resource, api, type_name) = match kind {
            "sandbox" => (
                "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/spare",
                "agents.x-k8s.io/v1beta1",
                "Sandbox",
            ),
            "pod" | "orphan_pod" => ("/api/v1/namespaces/cma-test/pods/spare", "v1", "Pod"),
            _ => (
                "/api/v1/namespaces/cma-test/persistentvolumeclaims/spare",
                "v1",
                "PersistentVolumeClaim",
            ),
        };
        let mut spare = json!({"apiVersion":api,"kind":type_name,"metadata":{"name":"spare","namespace":"cma-test","uid":"spare-uid","resourceVersion":"200"}});
        if kind.starts_with("orphan") {
            spare["metadata"]["ownerReferences"] = json!([{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"gone-spare","uid":"gone-spare-uid","controller":true}]);
        } else {
            spare["metadata"]["labels"] = json!({"cma/pool-config":&pool.config_sha256[..52]});
        }
        fixture
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .insert(resource.into(), spare);
        assert!(!retire(&fixture).await.unwrap(), "{kind}");
        no_deletes(&fixture);
        fixture.fixture.state.lock().unwrap().rows.remove(resource);
        assert!(retire(&fixture).await.unwrap(), "{kind}");
    }
}

#[tokio::test]
async fn retirement_rejects_unassigned_claim_and_template_shared_by_foreign_resources() {
    for kind in ["claim", "direct_claim", "pool"] {
        let fixture = upgraded().await;
        barrier(&fixture).await;
        let pool = binding(&fixture);
        let (resource, name, spec) = if kind == "pool" {
            (
                "sandboxwarmpools",
                "SandboxWarmPool",
                json!({"sandboxTemplateRef":{"name":pool.template_name}}),
            )
        } else if kind == "direct_claim" {
            (
                "sandboxclaims",
                "SandboxClaim",
                json!({"sandboxTemplateRef":{"name":pool.template_name}}),
            )
        } else {
            (
                "sandboxclaims",
                "SandboxClaim",
                json!({"warmPoolRef":{"name":pool.name}}),
            )
        };
        fixture.fixture.state.lock().unwrap().rows.insert(path(resource, "foreign"), json!({"apiVersion":API_VERSION,"kind":name,"metadata":{"name":"foreign","namespace":"cma-test","uid":"foreign-uid","resourceVersion":"200"},"spec":spec}));
        assert!(retire(&fixture).await.is_err(), "{kind}");
        no_deletes(&fixture);
    }
}

#[tokio::test]
async fn retirement_resumes_with_only_original_template_left_and_rejects_replacements() {
    for replacement in ["none", "pool", "template", "claim", "member", "pvc"] {
        let fixture = upgraded().await;
        barrier(&fixture).await;
        let pool = binding(&fixture);
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            if replacement != "pool" {
                state.rows.remove(&path("sandboxwarmpools", &pool.name));
            }
            let changed = match replacement {
                "pool" => Some(path("sandboxwarmpools", &pool.name)),
                "template" => Some(path("sandboxtemplates", &pool.template_name)),
                "claim" => Some(fixture.claim_path.clone()),
                "member" => Some(MEMBER.into()),
                "pvc" => Some(
                    "/api/v1/namespaces/cma-test/persistentvolumeclaims/home-warm-member".into(),
                ),
                _ => None,
            };
            if let Some(changed) = changed {
                state.rows.get_mut(&changed).unwrap()["metadata"]["uid"] = json!("replacement-uid");
            }
        }
        if replacement == "none" {
            assert!(retire(&fixture).await.unwrap());
        } else {
            assert!(retire(&fixture).await.is_err(), "{replacement}");
            no_deletes(&fixture);
        }
    }
}

#[tokio::test]
async fn retirement_rejects_incomplete_inventory_without_definition_deletion() {
    let fixture = upgraded().await;
    barrier(&fixture).await;
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .list_replies
        .push_back((
            200,
            json!({"apiVersion":API_VERSION,"kind":"SandboxClaimList","metadata":{},"items":[]}),
        ));
    assert!(retire(&fixture).await.is_err());
    no_deletes(&fixture);
}

#[tokio::test]
async fn retirement_of_empty_pool_never_needs_a_retained_home() {
    let fixture = Fixture::new().await;
    let pool = fixture.client.configure(&spec(), 0).await.unwrap().binding;
    fixture.client.begin_retirement(&pool).await.unwrap();
    assert!(fixture.client.retire_pool(&pool, &[]).await.unwrap());
}

#[tokio::test]
async fn retirement_marker_retries_conflict_without_draining_and_rejects_foreign_marker() {
    let fixture = upgraded().await;
    let pool = binding(&fixture);
    let before = fixture.fixture.row("sandboxwarmpools", &pool.name)["spec"]["replicas"].clone();
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .conflict
        .push(format!("PATCH {}", path("sandboxwarmpools", &pool.name)));
    assert!(fixture.fixture.client.begin_retirement(pool).await.is_err());
    fixture.fixture.client.begin_retirement(pool).await.unwrap();
    assert_eq!(
        fixture.fixture.row("sandboxwarmpools", &pool.name)["spec"]["replicas"],
        before
    );
    fixture.fixture.edit("sandboxwarmpools", &pool.name, |p| {
        p["metadata"]["annotations"][RETIRING] = json!("foreign")
    });
    fixture.fixture.state.lock().unwrap().calls.clear();
    assert!(fixture.fixture.client.begin_retirement(pool).await.is_err());
    fixture.no_writes();
}

#[tokio::test]
async fn retirement_pending_definition_finalizers_are_not_completion() {
    for kind in ["sandboxwarmpools", "sandboxtemplates"] {
        let fixture = upgraded().await;
        barrier(&fixture).await;
        let pool = binding(&fixture);
        if kind == "sandboxtemplates" {
            fixture
                .fixture
                .state
                .lock()
                .unwrap()
                .rows
                .remove(&path("sandboxwarmpools", &pool.name));
        }
        let name = if kind == "sandboxwarmpools" {
            &pool.name
        } else {
            &pool.template_name
        };
        fixture.fixture.edit(kind, name, |p| {
            p["metadata"]["deletionTimestamp"] = json!("2026-10-19T00:00:00Z")
        });
        assert!(!retire(&fixture).await.unwrap());
        no_deletes(&fixture);
    }
}

#[tokio::test]
async fn retirement_late_unassigned_claim_cannot_hide_by_changing_pool_reference() {
    let fixture = upgraded().await;
    barrier(&fixture).await;
    let gate = Gate::new();
    let effects = crate::agent_sandbox::pool::read_fixture::Effects::new(
        &fixture.fixture,
        BTreeMap::from([(format!("{}/sandboxclaims", prefix()), gate.clone())]),
    );
    let pools = effects.pools(&fixture.fixture);
    let pool = binding(&fixture).clone();
    let homes = vec![fixture.home.clone()];
    let operation = tokio::spawn(async move { pools.retire_pool(&pool, &homes).await });
    gate.wait().await;
    let mut request = fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .clone();
    request.runtime_generation += 1;
    let mut pending = serde_json::to_value(
        crate::agent_sandbox::pool::model::claim(&fixture.fixture.client.config, &request).unwrap(),
    )
    .unwrap();
    let name = pending["metadata"]["name"].as_str().unwrap().to_owned();
    pending["metadata"]["uid"] = json!("pending-uid");
    pending["metadata"]["resourceVersion"] = json!("210");
    pending["spec"]["warmPoolRef"]["name"] = json!("another-pool");
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .insert(path("sandboxclaims", &name), pending);
    gate.respond();
    assert!(operation.await.unwrap().is_err());
    effects.assert_read_only();
}

#[tokio::test]
async fn retirement_conflicts_preserve_definitions_and_retry_from_fresh_versions() {
    for kind in ["sandboxwarmpools", "sandboxtemplates"] {
        let fixture = upgraded().await;
        barrier(&fixture).await;
        let pool = binding(&fixture);
        let name = if kind == "sandboxwarmpools" {
            &pool.name
        } else {
            &pool.template_name
        };
        fixture
            .fixture
            .state
            .lock()
            .unwrap()
            .conflict
            .push(format!("DELETE {}", path(kind, name)));
        assert!(retire(&fixture).await.is_err());
        assert!(fixture
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .contains_key(&path(kind, name)));
        assert!(retire(&fixture).await.unwrap());
    }
}

#[tokio::test]
async fn retirement_preserves_live_retained_pods_and_unrelated_graphs() {
    for paused in [false, true] {
        let fixture = upgraded().await;
        barrier(&fixture).await;
        if paused {
            client(&fixture.fixture, true)
                .suspend(&fixture.handle, &target(&fixture.home, 2, &image('b')))
                .await
                .unwrap();
        } else {
            fixture.fixture.state.lock().unwrap().rows.insert("/api/v1/namespaces/cma-test/pods/retained".into(), json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"retained","namespace":"cma-test","uid":"retained-pod","resourceVersion":"210","ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]}}));
        }
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            state.rows.insert("/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/foreign".into(), json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","metadata":{"name":"foreign","namespace":"cma-test","uid":"foreign-member","resourceVersion":"210"}}));
            state.rows.insert("/api/v1/namespaces/cma-test/pods/foreign".into(), json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"foreign","namespace":"cma-test","uid":"foreign-pod","resourceVersion":"210","ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"foreign","uid":"foreign-member","controller":true}]}}));
        }
        let before = fixture.fixture.state.lock().unwrap().rows.clone();
        assert!(retire(&fixture).await.unwrap());
        let state = fixture.fixture.state.lock().unwrap();
        for (path, row) in before {
            if path.contains("/sandboxes/") || path.contains("/pods/") {
                assert_eq!(state.rows[&path], row);
            }
        }
    }
}
