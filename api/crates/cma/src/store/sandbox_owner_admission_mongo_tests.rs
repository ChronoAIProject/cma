//! Per-owner limits survive independent managers and fresh process state.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_clients_enforce_three_sandboxes_per_owner_without_splitting_capacity() {
    let (client, name) = database("owner_three_replicas").await;
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let left = MongoSandboxStore::open(&client, &name).await.unwrap();
    let right = MongoSandboxStore::open(&peer, &name).await.unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let results = futures_util::future::join_all((0..12).map(|index| {
        let store = if index % 2 == 0 {
            left.clone()
        } else {
            right.clone()
        };
        let barrier = barrier.clone();
        async move {
            let request = admission(
                &format!("sbx_owner_{index}"),
                &format!("launch-owner-{index}"),
                3,
            );
            barrier.wait().await;
            (request.clone(), store.admit_sandbox(request).await)
        }
    }))
    .await;
    let mut accepted = Vec::new();
    let mut refused = 0;
    for (request, result) in results {
        match result {
            Ok(SandboxAdmissionResult::Created(value)) => accepted.push((request, value)),
            Err(SandboxStoreError::LimitReached { limit: 3, live: 3 }) => refused += 1,
            other => panic!("unexpected shared owner admission: {other:?}"),
        }
    }
    assert_eq!(accepted.len(), 3);
    assert_eq!(refused, 9);
    assert_eq!(
        right
            .sandboxes
            .count_documents(doc! { "owner": "owner" })
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        right.sandbox_events.count_documents(doc! {}).await.unwrap(),
        3
    );
    // A new manager sees retained admission receipts even at full capacity.
    let restarted = MongoSandboxStore::open(&peer, &name).await.unwrap();
    for (request, value) in accepted {
        assert_eq!(
            restarted.admit_sandbox(request).await.unwrap(),
            SandboxAdmissionResult::Existing(value)
        );
    }
    assert!(matches!(
        restarted
            .admit_sandbox(admission("sbx_fourth", "launch-fourth", 3))
            .await,
        Err(SandboxStoreError::LimitReached { limit: 3, live: 3 })
    ));
    // Another owner's allowance remains independently available on either pod.
    let mut other = sandbox("sbx_other_owner", "launch-other-owner");
    other.owner = SubjectId("other-owner".into());
    other.name = "other".into();
    let other = SandboxAdmission::new(
        other,
        SandboxAdmissionName::Allocate("other".into()),
        3,
        NOW.into(),
    )
    .unwrap();
    assert!(matches!(
        left.admit_sandbox(other).await.unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    client.database(&name).drop().await.unwrap();
}
