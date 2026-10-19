use super::*;
use crate::agent_sandbox::pool::network_policy_tests;

#[tokio::test]
async fn pool_retirement_rejects_genuinely_upgraded_template_network_dependents() {
    let fixture = fixture(true).await;
    upgrade(&fixture, &fixture.previous, &fixture.next)
        .await
        .unwrap();
    let pool = &fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .pool;
    fixture.fixture.client.begin_retirement(pool).await.unwrap();
    fixture.fixture.client.resize(pool, 0).await.unwrap();
    fixture.fixture.state.lock().unwrap().calls.clear();
    assert!(fixture
        .fixture
        .client
        .retire_pool(pool, std::slice::from_ref(&fixture.home))
        .await
        .is_err());
    fixture.no_writes();
}

async fn fixture(network: bool) -> Resume {
    let mut fixture = Fixture::new().await;
    fixture.client.config.template_fs_group_change_policy =
        Some(crate::agent_sandbox::NativeFsGroupChangePolicy::OnRootMismatch);
    if network {
        fixture.client.config.template_network_policy =
            Some(network_policy_tests::policy("10.41.0.0/16"));
    }
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let previous = request(&home, 1);
    let runtime = client(&fixture, false);
    let handle = runtime
        .admit_pool_runtime(&claim, &home, &previous)
        .await
        .unwrap();
    runtime.suspend(&handle, &previous).await.unwrap();
    let next = target(&home, 2, &image('b'));
    fixture.state.lock().unwrap().calls.clear();
    Resume {
        fixture,
        home,
        handle,
        previous,
        next,
        claim_path: format!("{}/sandboxclaims/{}", prefix(), claim.name),
    }
}
fn remove_definitions(fixture: &Resume) {
    let pool = &fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .pool;
    let mut state = fixture.fixture.state.lock().unwrap();
    assert!(state
        .rows
        .remove(&format!("{}/sandboxwarmpools/{}", prefix(), pool.name))
        .is_some());
    assert!(state
        .rows
        .remove(&format!(
            "{}/sandboxtemplates/{}",
            prefix(),
            pool.template_name
        ))
        .is_some());
    state.calls.clear();
}
fn add_pod(fixture: &Resume) {
    let mut state = fixture.fixture.state.lock().unwrap();
    state.rows.insert("/api/v1/namespaces/cma-test/pods/warm-member".into(), json!({"apiVersion":"v1","kind":"Pod",
        "metadata":{"name":"warm-member","namespace":"cma-test","uid":"retained-upgraded-pod",
            "labels":{"cma/control-plane":"cma-test",crate::agent_sandbox::pods::TRACKING_LABEL:crate::agent_sandbox::pods::name_hash("warm-member")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}]}}));
    let member = state.rows.get_mut(MEMBER).unwrap();
    member["status"]["conditions"][0]["observedGeneration"] =
        member["metadata"]["generation"].clone();
}

#[tokio::test]
async fn upgraded_native_runtime_retains_policy_after_original_definitions_are_removed() {
    let fixture = fixture(false).await;
    upgrade(&fixture, &fixture.previous, &fixture.next)
        .await
        .unwrap();
    remove_definitions(&fixture);
    let runtime = client(&fixture.fixture, true);
    assert_eq!(
        fixture
            .fixture
            .client
            .recover_known_native_vct(fixture.home.receipt.native_authority.as_deref().unwrap())
            .await
            .unwrap(),
        Some(fixture.home.clone())
    );
    runtime
        .validate_request(&fixture.handle, &fixture.next)
        .await
        .unwrap()
        .unwrap();
    upgrade(&fixture, &fixture.previous, &fixture.next)
        .await
        .unwrap();
    add_pod(&fixture);
    let pod = runtime.pod_binding(&fixture.handle).await.unwrap();
    runtime
        .assign_pool_runtime(&fixture.handle, &pod, &fixture.next)
        .await
        .unwrap()
        .unwrap();
    let proof = runtime
        .native_runtime_proof(&fixture.handle, &fixture.next, &fixture.home)
        .await
        .unwrap();
    runtime
        .complete_native_runtime_policy(
            &fixture.home,
            proof.member(),
            Err(crate::AdapterError::Unavailable(
                "retired definitions".into(),
            )),
        )
        .unwrap();
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .remove("/api/v1/namespaces/cma-test/pods/warm-member");
    runtime
        .suspend(&fixture.handle, &fixture.next)
        .await
        .unwrap();
    let resumed = target(&fixture.home, 3, &image('b'));
    runtime
        .resume_with_home(&fixture.handle, &fixture.next, &resumed, &fixture.home)
        .await
        .unwrap();
    runtime.suspend(&fixture.handle, &resumed).await.unwrap();
    let next = target(&fixture.home, 4, &image('c'));
    upgrade(&fixture, &resumed, &next).await.unwrap();
    runtime
        .validate_request(&fixture.handle, &next)
        .await
        .unwrap()
        .unwrap();
    assert!(runtime
        .validate_request(&fixture.handle, &fixture.previous)
        .await
        .is_err());
    runtime.suspend(&fixture.handle, &next).await.unwrap();
    let final_request = target(&fixture.home, 5, &image('c'));
    runtime
        .resume(&fixture.handle, &next, &final_request)
        .await
        .unwrap();
    assert_eq!(fixture.fixture.state.lock().unwrap().assignments.len(), 1);
}

#[tokio::test]
async fn original_admissions_and_template_network_policy_cannot_use_retirement_exemption() {
    for upgraded_network in [false, true] {
        let fixture = fixture(upgraded_network).await;
        if upgraded_network {
            upgrade(&fixture, &fixture.previous, &fixture.next)
                .await
                .unwrap();
        }
        remove_definitions(&fixture);
        let runtime = client(&fixture.fixture, true);
        assert!(upgrade(&fixture, &fixture.previous, &fixture.next)
            .await
            .is_err());
        let (previous, next) = if upgraded_network {
            fixture
                .fixture
                .state
                .lock()
                .unwrap()
                .rows
                .get_mut(MEMBER)
                .unwrap()["spec"]["operatingMode"] = json!("Suspended");
            (fixture.next.clone(), target(&fixture.home, 3, &image('b')))
        } else {
            (fixture.previous.clone(), request(&fixture.home, 2))
        };
        assert!(runtime
            .resume_with_home(&fixture.handle, &previous, &next, &fixture.home)
            .await
            .is_err());
        assert!(runtime
            .provision_pool_runtime_with_home(&fixture.previous, &fixture.home)
            .await
            .is_err());
        fixture.no_writes();
    }
}

#[tokio::test]
async fn retired_definition_exemption_cannot_hide_changed_member_policy_or_claim_metadata() {
    for field in ["fs_group", "claim_metadata", "origin", "proof"] {
        let fixture = fixture(false).await;
        upgrade(&fixture, &fixture.previous, &fixture.next)
            .await
            .unwrap();
        remove_definitions(&fixture);
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            if field == "claim_metadata" {
                state.rows.get_mut(&fixture.claim_path).unwrap()["spec"]["additionalPodMetadata"] =
                    json!({"labels":{"new":"override"}});
            } else {
                let member = state.rows.get_mut(MEMBER).unwrap();
                match field {
                    "fs_group" => {
                        member["spec"]["podTemplate"]["spec"]["securityContext"]
                            ["fsGroupChangePolicy"] = json!("Always")
                    }
                    "origin" => {
                        member["metadata"]["annotations"][NATIVE_HOME_ANNOTATION] = json!("{}")
                    }
                    "proof" => {
                        member["metadata"]["annotations"]
                            .as_object_mut()
                            .unwrap()
                            .remove(UPGRADE_ANNOTATION);
                    }
                    _ => unreachable!(),
                }
            }
        }
        assert!(
            upgrade(&fixture, &fixture.previous, &fixture.next)
                .await
                .is_err(),
            "{field}"
        );
        assert!(
            fixture
                .fixture
                .client
                .prove_native_home(&fixture.home)
                .await
                .is_err(),
            "{field}"
        );
        fixture.no_writes();
    }
}
