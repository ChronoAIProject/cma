use super::{fixture::Fixture, *};
use serde_json::json;

#[tokio::test]
async fn cleanup_checks_fifty_two_unrelated_pods_with_one_fresh_member_list() {
    let fixture = Fixture::new().await;
    let original = NativePoolCleanup {
        request: tests::request(
            fixture
                .client
                .configure(&tests::spec(), 0)
                .await
                .unwrap()
                .binding,
        ),
        claim: None,
        members: vec![],
    };
    {
        let mut state = fixture.state.lock().unwrap();
        for i in 0..52 {
            let name = format!("other-{i}");
            let uid = format!("uid-{i}");
            state.rows.insert(format!("/api/v1/namespaces/cma-test/pods/{name}"), json!({
                "apiVersion":"v1", "kind":"Pod", "metadata":{"name":name,"namespace":"cma-test","uid":format!("pod-{i}"),
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":name,"uid":uid}]}
            }));
            state.rows.insert(format!("/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/{name}"), json!({
                "apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox", "metadata":{"name":name,"namespace":"cma-test","uid":uid}
            }));
        }
        state.calls.clear();
    }
    fixture.client.cleanup_pods_absent(&original).await.unwrap();
    let calls = fixture.state.lock().unwrap().calls.clone();
    assert_eq!(
        calls.len(),
        2,
        "one complete Pod list and one complete member list"
    );
    assert!(calls[0].0.ends_with("/pods"));
    assert!(calls[1].0.ends_with("/sandboxes"));
    let path = "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/other-51";
    let original_member = fixture.state.lock().unwrap().rows[path].clone();
    fixture.state.lock().unwrap().rows.get_mut(path).unwrap()["metadata"]["uid"] =
        json!("replacement");
    assert!(fixture.client.cleanup_pods_absent(&original).await.is_err());
    fixture.state.lock().unwrap().rows.remove(path);
    assert!(fixture.client.cleanup_pods_absent(&original).await.is_err());
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.insert(path.into(), original_member.clone());
        let mut ambiguous = original_member;
        ambiguous["metadata"]["uid"] = json!("second-uid-for-same-name");
        state.rows.insert(format!("{path}-duplicate"), ambiguous);
    }
    assert!(fixture.client.cleanup_pods_absent(&original).await.is_err());
}

#[tokio::test]
async fn cleanup_rejects_a_fresh_unrecorded_member_owned_by_the_claim() {
    let fixture = Fixture::new().await;
    let claim = super::home_tests::assigned(&fixture).await;
    let original = NativePoolCleanup {
        request: claim.request.clone(),
        claim: Some(claim.clone()),
        members: vec![],
    };
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.insert("/api/v1/namespaces/cma-test/pods/unrecorded".into(), json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{"name":"unrecorded","namespace":"cma-test","uid":"pod-unrecorded",
        "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"late-member","uid":"late-uid"}]}
    }));
        state.rows.insert("/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/late-member".into(), json!({
        "apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox", "metadata":{"name":"late-member","namespace":"cma-test","uid":"late-uid",
        "ownerReferences":[{"apiVersion":API_VERSION,"kind":"SandboxClaim","name":claim.name,"uid":claim.uid}]}
    }));
    }
    assert!(fixture.client.cleanup_pods_absent(&original).await.is_err());
}
