use super::*;
use layer2_core::sandbox::SandboxPolicy;
use layer2_core::{Authorization, SubjectId};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_scope_preflight_is_owner_bound_requires_baseline_and_never_grants() {
    let (client, name, store, original) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let baseline = vec![
        ServiceId::new("chrono-llm-public"),
        ServiceId::new("ornn-api"),
    ];
    let policy = SandboxPolicy {
        baseline_services: baseline.clone(),
        ..Default::default()
    };
    let caller = Caller {
        subject: original.requester.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let selected = ["api-github", "chrono-llm-public", "ornn-api"]
        .map(String::from)
        .to_vec();
    let before = store
        .get_sandbox(&original.requester.sandbox_id)
        .await
        .unwrap();
    let pending = store.read_access_request(&original.id).await.unwrap();
    let mut result = services(
        &store,
        &policy,
        &caller,
        &original.requester.agent_id,
        &selected,
    )
    .await
    .unwrap();
    result.sort();
    assert_eq!(
        result,
        selected
            .iter()
            .cloned()
            .map(ServiceId::new)
            .collect::<Vec<_>>()
    );
    for selected in [
        vec!["api-github".into()],
        vec![
            "chrono-llm-public".into(),
            "ornn-api".into(),
            "ornn-api".into(),
        ],
        vec!["bad/slug".into()],
    ] {
        assert_eq!(
            services(
                &store,
                &policy,
                &caller,
                &original.requester.agent_id,
                &selected
            )
            .await,
            Err(ConnectionError::Changed)
        );
    }
    let foreign = Caller {
        subject: SubjectId::new("foreign"),
        authorization: caller.authorization.clone(),
    };
    assert_eq!(
        services(
            &store,
            &policy,
            &foreign,
            &original.requester.agent_id,
            &selected
        )
        .await,
        Err(ConnectionError::NotFound)
    );
    let forbidden = Caller {
        subject: caller.subject.clone(),
        authorization: Authorization::new([Capability::CreateSandbox]),
    };
    assert_eq!(
        services(
            &store,
            &policy,
            &forbidden,
            &original.requester.agent_id,
            &selected
        )
        .await,
        Err(ConnectionError::Forbidden)
    );
    assert_eq!(
        store
            .get_sandbox(&original.requester.sandbox_id)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        store.read_access_request(&original.id).await.unwrap(),
        pending
    );
    client.database(&name).drop().await.unwrap();
}
