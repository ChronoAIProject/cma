use super::*;
use layer2_core::{sandbox::store::SandboxLimitClass, PlatformSettingsStore};

fn visitor_admission(id: &str, launch: &str, limit: u16) -> SandboxAdmission {
    let mut candidate = admission(id, launch, limit);
    candidate.limit_class = SandboxLimitClass::Visitor;
    candidate
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn visitor_limit_admits_two_and_refuses_third_across_replicas() {
    let (client, database) = database("visitor_limit").await;
    let left = MongoSandboxStore::open(&client, &database).await.unwrap();
    let right = MongoSandboxStore::open(&client, &database).await.unwrap();
    let settings = crate::store::MongoPlatformSettingsStore::open(&client, &database)
        .await
        .unwrap();
    let mut policy = settings.read().await.unwrap();
    policy.settings.limits.visitor_max_sandboxes = 2;
    settings
        .replace(policy.revision, &policy.settings)
        .await
        .unwrap();
    let results = futures_util::future::join_all((0..6).map(|index| {
        let store = if index % 2 == 0 { &left } else { &right };
        async move {
            store
                .admit_sandbox(visitor_admission(
                    &format!("sbx_visitor_{index}"),
                    &format!("launch-visitor-{index}"),
                    2,
                ))
                .await
        }
    }))
    .await;
    let mut created = Vec::new();
    let mut refused = 0;
    for result in results {
        match result {
            Ok(SandboxAdmissionResult::Created(sandbox)) => created.push(sandbox),
            Err(SandboxStoreError::LimitReached {
                limit: 2, live: 2, ..
            }) => refused += 1,
            other => panic!("unexpected visitor admission result: {other:?}"),
        }
    }
    assert_eq!(created.len(), 2);
    assert_eq!(refused, 4);
    let live = doc! { "owner": "owner", "retired_at": mongodb::bson::Bson::Null };
    assert_eq!(
        left.sandboxes.count_documents(live.clone()).await.unwrap(),
        2
    );
    assert_eq!(left.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        left.sandbox_events.count_documents(doc! {}).await.unwrap(),
        2
    );

    let mut retiring = created.remove(0);
    retiring.status = SandboxStatus::Retiring;
    retiring.updated_at = LATER.into();
    retiring.revision += 1;
    left.save_sandbox(retiring.revision - 1, retiring.clone())
        .await
        .unwrap();
    assert!(matches!(
        right
            .admit_sandbox(visitor_admission(
                "sbx_visitor_retiring",
                "launch-visitor-retiring",
                2
            ))
            .await,
        Err(SandboxStoreError::LimitReached {
            limit: 2,
            live: 2,
            ..
        })
    ));

    retiring.status = SandboxStatus::Retired;
    retiring.retired_at = Some(LATER.into());
    retiring.revision += 1;
    left.save_sandbox(retiring.revision - 1, retiring)
        .await
        .unwrap();
    assert!(matches!(
        right
            .admit_sandbox(visitor_admission(
                "sbx_visitor_replacement",
                "launch-visitor-replacement",
                2
            ))
            .await
            .unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    assert_eq!(
        left.sandboxes.count_documents(live.clone()).await.unwrap(),
        2
    );

    let before = left
        .list_sandboxes(&SubjectId::new("owner"), &Default::default())
        .await
        .unwrap();
    assert!(matches!(
        right
            .admit_sandbox(visitor_admission(
                "sbx_visitor_lowered",
                "launch-visitor-lowered",
                1
            ))
            .await,
        Err(SandboxStoreError::LimitReached {
            limit: 1,
            live: 2,
            ..
        })
    ));
    let after = left
        .list_sandboxes(&SubjectId::new("owner"), &Default::default())
        .await
        .unwrap();
    assert_eq!(before.items, after.items);
    assert_eq!(left.sandboxes.count_documents(live).await.unwrap(), 2);
    assert_eq!(left.sandboxes.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(
        left.sandbox_events.count_documents(doc! {}).await.unwrap(),
        3
    );
    client.database(&database).drop().await.unwrap();
}
