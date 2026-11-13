use super::*;
use futures_util::future::join_all;

// Keep fixed windows beyond the TTL monitor's clock during the campaign.
const BEFORE_MIDNIGHT: &str = "2099-09-05T23:59:59Z";
const AFTER_MIDNIGHT: &str = "2099-09-06T00:00:00Z";

fn admission(trigger: &PublicTrigger, owner: &str, key: usize, at: &str) -> LaunchAdmission {
    let mut candidate = launch_at(trigger, 'a', 'b', at);
    candidate.owner = SubjectId::new(owner);
    candidate.authority = LaunchAuthorityClass::Visitor;
    candidate.command_key_hash = format!("{key:064x}");
    LaunchAdmission {
        launch: candidate,
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    }
}

async fn replicas(seed: &Seeded) -> [MongoTriggerDefinitionStore; 2] {
    [
        seed.store.clone(),
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap(),
    ]
}

async fn race(
    stores: &[MongoTriggerDefinitionStore; 2],
    candidates: Vec<LaunchAdmission>,
) -> Vec<Result<LaunchAdmissionResult, LaunchStoreError>> {
    join_all(
        candidates
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| stores[index % 2].admit_launch(candidate)),
    )
    .await
}

async fn used(seed: &Seeded, owner: &str, at: &str) -> i64 {
    let day = layer2_core::seconds_from_rfc3339(at).unwrap() / 86_400;
    seed.client
        .database(&seed.database)
        .collection::<Document>("visitor_launch_days")
        .find_one(doc! { "_id": format!("{day}:{owner}") })
        .await
        .unwrap()
        .unwrap()
        .get_i64("used")
        .unwrap()
}

fn outcomes(
    results: &[Result<LaunchAdmissionResult, LaunchStoreError>],
) -> (usize, usize, usize, usize) {
    let mut counts = (0, 0, 0, 0);
    for result in results {
        match result {
            Ok(LaunchAdmissionResult::Created(_)) => counts.0 += 1,
            Ok(LaunchAdmissionResult::Existing(_)) => counts.1 += 1,
            Err(LaunchStoreError::VisitorQuota { limit: 5, .. }) => counts.2 += 1,
            Err(LaunchStoreError::Unavailable(_)) => counts.3 += 1,
            other => panic!("unexpected admission outcome: {other:?}"),
        }
    }
    eprintln!(
        "Created={}, Existing={}, VisitorQuota={}, Unavailable={}",
        counts.0, counts.1, counts.2, counts.3
    );
    if let Some(reason) = results.iter().find_map(|result| match result {
        Err(LaunchStoreError::Unavailable(reason)) => Some(reason),
        _ => None,
    }) {
        eprintln!("First unavailable result: {reason}");
    }
    counts
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn fifty_concurrent_visitor_launches_admit_exactly_five() {
    let seed = seeded("visitor_fifty").await;
    let triggers = [
        published(&seed, "first").await,
        published(&seed, "second").await,
    ];
    let stores = replicas(&seed).await;
    let candidates = (0..50)
        .map(|index| admission(&triggers[index % 2], "visitor", index, BEFORE_MIDNIGHT))
        .collect();
    let results = race(&stores, candidates).await;
    let counts = outcomes(&results);
    let charged = used(&seed, "visitor", BEFORE_MIDNIGHT).await;
    seed.client.database(&seed.database).drop().await.unwrap();
    assert_eq!(counts, (5, 0, 45, 0));
    assert_eq!(charged, 5);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn duplicate_keys_charge_once() {
    let seed = seeded("visitor_duplicates").await;
    let trigger = published(&seed, "duplicates").await;
    let stores = replicas(&seed).await;
    let original = admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT);
    let candidates = (0..20)
        .map(|_| admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT))
        .collect();
    let results = race(&stores, candidates).await;
    assert_eq!(outcomes(&results), (1, 19, 0, 0));
    let id = &results[0].as_ref().unwrap().launch().id;
    assert!(results
        .iter()
        .all(|result| &result.as_ref().unwrap().launch().id == id));
    assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 1);
    assert_eq!(stats(&seed, &trigger).await.launches_total, 1);
    let mut changed = original;
    changed.launch.command_fingerprint = digest('c');
    assert_eq!(
        stores[1].admit_launch(changed).await,
        Err(LaunchStoreError::CommandConflict)
    );
    let second = published(&seed, "another_public_id").await;
    assert!(stores[1]
        .admit_launch(admission(&second, "visitor", 0, BEFORE_MIDNIGHT))
        .await
        .unwrap()
        .was_created());
    assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 2);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn cancel_does_not_refund_the_day() {
    let seed = seeded("visitor_cancel").await;
    let trigger = published(&seed, "cancel").await;
    let stores = replicas(&seed).await;
    let mut admitted = Vec::new();
    for index in 0..5 {
        admitted.push(
            stores[index % 2]
                .admit_launch(admission(&trigger, "visitor", index, BEFORE_MIDNIGHT))
                .await
                .unwrap()
                .launch()
                .clone(),
        );
    }
    let mut cancelled = admitted.remove(0);
    let expected_revision = cancelled.revision;
    cancelled.cancel(BEFORE_MIDNIGHT.into()).unwrap();
    stores[1]
        .commit_launch(LaunchCommit {
            expected_revision,
            launch: cancelled,
            event_at: BEFORE_MIDNIGHT.into(),
            events: vec![LaunchEventKind::Cancelled],
        })
        .await
        .unwrap();
    assert!(matches!(
        stores[0]
            .admit_launch(admission(&trigger, "visitor", 5, BEFORE_MIDNIGHT))
            .await,
        Err(LaunchStoreError::VisitorQuota { limit: 5, .. })
    ));
    assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 5);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn midnight_sides_count_separately() {
    let seed = seeded("visitor_midnight").await;
    let trigger = published(&seed, "midnight").await;
    let stores = replicas(&seed).await;
    let original = admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT);
    let candidates = (0..10)
        .map(|index| {
            admission(
                &trigger,
                "visitor",
                index,
                if index < 5 {
                    BEFORE_MIDNIGHT
                } else {
                    AFTER_MIDNIGHT
                },
            )
        })
        .collect();
    let results = race(&stores, candidates).await;
    assert_eq!(outcomes(&results), (10, 0, 0, 0));
    assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 5);
    assert_eq!(used(&seed, "visitor", AFTER_MIDNIGHT).await, 5);
    let mut replay = original;
    replay.launch.created_at = AFTER_MIDNIGHT.into();
    replay.launch.updated_at = AFTER_MIDNIGHT.into();
    assert!(
        matches!(stores[1].admit_launch(replay).await.unwrap(), LaunchAdmissionResult::Existing(ref launch)
        if launch.created_at == BEFORE_MIDNIGHT)
    );
    assert_eq!(used(&seed, "visitor", AFTER_MIDNIGHT).await, 5);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn visitor_allowance_read_is_shared_scoped_and_rejects_corrupt_rows() {
    let seed = seeded("visitor_read").await;
    let trigger = published(&seed, "read").await;
    let stores = replicas(&seed).await;
    let owner = SubjectId::new("visitor");
    let day = layer2_core::seconds_from_rfc3339(BEFORE_MIDNIGHT).unwrap() / 86_400;
    for store in &stores {
        assert_eq!(store.visitor_launches_admitted(&owner, day).await, Ok(0));
    }
    for (index, store) in stores.iter().enumerate() {
        assert!(store
            .admit_launch(admission(&trigger, "visitor", index, BEFORE_MIDNIGHT))
            .await
            .unwrap()
            .was_created());
    }
    for store in &stores {
        assert_eq!(store.visitor_launches_admitted(&owner, day).await, Ok(2));
        assert_eq!(
            store.visitor_launches_admitted(&owner, day + 1).await,
            Ok(0)
        );
        assert_eq!(
            store
                .visitor_launches_admitted(&SubjectId::new("other"), day)
                .await,
            Ok(0)
        );
    }
    let rows = seed
        .client
        .database(&seed.database)
        .collection::<Document>("visitor_launch_days");
    let key = format!("{day}:visitor");
    rows.update_one(
        doc! { "_id": &key },
        doc! { "$set": { "used": i64::from(u32::MAX) } },
    )
    .await
    .unwrap();
    assert_eq!(
        stores[0].visitor_launches_admitted(&owner, day).await,
        Ok(u32::MAX)
    );
    for mut row in [
        doc! { "document_schema_version": 1_i64, "used": -1_i64 },
        doc! { "document_schema_version": 1_i64, "used": i64::from(u32::MAX) + 1 },
        doc! { "document_schema_version": 1_i64, "used": "invalid" },
        doc! { "document_schema_version": 1_i64 },
        doc! { "document_schema_version": 2_i64, "used": 1_i64 },
    ] {
        row.insert("_id", &key);
        rows.replace_one(doc! { "_id": &key }, row).await.unwrap();
        for store in &stores {
            assert!(matches!(
                store.visitor_launches_admitted(&owner, day).await,
                Err(LaunchStoreError::Corrupt(_))
            ));
        }
    }
    seed.client.database(&seed.database).drop().await.unwrap();
}
