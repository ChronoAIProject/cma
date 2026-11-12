use super::*;

// Keep fixed windows beyond the TTL monitor's clock during the campaign.
const BEFORE_MIDNIGHT: &str = "2099-09-05T23:59:59Z";

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
