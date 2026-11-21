use super::*;
use crate::store::{MongoAdmissionCeilingsStore, MongoRegistrationCapacityStore};
use layer2_core::admission::PendingHomesObservation;

fn feature_commands(commands: &[Document]) -> Vec<&Document> {
    commands
        .iter()
        .filter(|command| {
            command.get_str("listIndexes").ok() == Some("sandbox_admissions")
                || command.get_str("aggregate").ok() == Some("sandbox_admissions")
                || command.get_str("hint").ok() == Some(INDEX)
                || command.get_array("indexes").is_ok_and(|indexes| {
                    indexes.iter().any(|index| {
                        index
                            .as_document()
                            .is_some_and(|index| index.get_str("name").ok() == Some(INDEX))
                    })
                })
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn disabled_startup_with_active_capacity_preserves_usage_receipts_and_ordinary_writers() {
    let (seed, public, _) = setup("pending_policy_off").await;
    let lost = leak(&seed, &public, 0).await;
    admissions(&seed).drop_index(INDEX).await.unwrap();
    let before = receipt(&seed, &lost).await;
    let (client, commands) = monitored_client().await;
    for writes_enabled in [false, true] {
        commands.lock().unwrap().clear();
        let registration =
            MongoRegistrationCapacityStore::open(&client, &seed.database, writes_enabled)
                .await
                .unwrap();
        let settings = MongoAdmissionCeilingsStore::open(&client, &seed.database, 0)
            .await
            .unwrap()
            .with_activation(registration.activation_observation());
        let _ordinary = crate::store::MongoSandboxStore::open(&client, &seed.database)
            .await
            .unwrap();
        assert!(!settings.pending_homes.enabled());
        assert!(settings.pending_homes.active());
        let startup = commands.lock().unwrap().clone();
        assert!(
            feature_commands(&startup).is_empty(),
            "off startup must not inspect or create the recovery index"
        );
        commands.lock().unwrap().clear();
        assert_eq!(
            settings.pending_homes.readiness().await,
            Err(PendingHomesObservation::Disabled)
        );
        assert!(settings.pending_homes.observe().await.is_err());
        assert_eq!(settings.pending_homes.sweep(None).await.unwrap().scanned, 0);
        for dry_run in [true, false] {
            assert!(settings
                .reconcile_pending_homes(
                    1,
                    &SubjectId::new("operator"),
                    "off correction",
                    dry_run,
                    100
                )
                .await
                .is_err());
        }
        assert!(
            commands.lock().unwrap().is_empty(),
            "off direct recovery calls must issue no commands"
        );
        let read = settings.read_with_history().await.unwrap();
        assert_eq!(read.counts.pending_launch_homes, 1);
        assert_eq!(read.counts.retained_homes.unwrap().observed, 1);
        assert_eq!(read.counts.pending_homes, PendingHomesObservation::Disabled);
        let trace = commands.lock().unwrap().clone();
        assert!(feature_commands(&trace).is_empty());
        assert!(writes(&trace).is_empty(), "ordinary reporting is read-only");
        assert_eq!(receipt(&seed, &lost).await, before);
    }
    let normal = seed
        .store
        .admit_launch(request(&public, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    let transfer = seed
        .store
        .admit_launch(request(&public, 2))
        .await
        .unwrap()
        .launch()
        .clone();
    assert_eq!(pending(&seed).await, 3);
    resources(&seed)
        .await
        .admit_sandbox(home(&seed, &transfer, "sbx_off_transfer", false))
        .await
        .unwrap();
    seed.store
        .commit_launch(cancel_commit(normal.clone()))
        .await
        .unwrap();
    assert_eq!(pending(&seed).await, 1);
    assert_eq!(
        receipt(&seed, &normal).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &transfer).await.get_str("state").unwrap(),
        "consumed"
    );
    assert_eq!(receipt(&seed, &lost).await, before);
    assert_eq!(
        admissions(&seed)
            .count_documents(doc! {"_id.scope":"launch_home_capacity"})
            .await
            .unwrap(),
        3
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn enabled_recovery_requires_the_exact_prepared_index_and_preserves_baseline_reads() {
    let (seed, public, _) = setup("pending_index_policy").await;
    let lost = leak(&seed, &public, 0).await;
    admissions(&seed).drop_index(INDEX).await.unwrap();
    let (client, commands) = monitored_client().await;
    let registration = MongoRegistrationCapacityStore::open(&client, &seed.database, false)
        .await
        .unwrap();
    let store = MongoAdmissionCeilingsStore::open(&client, &seed.database, 0)
        .await
        .unwrap()
        .with_activation(registration.activation_observation())
        .with_pending_home_reconciliation(true);
    for (definition, expected) in [
        (None, PendingHomesObservation::IndexMissing),
        (
            Some(doc! {"state": "released", "_id.scope": "launch_home_capacity"}),
            PendingHomesObservation::IndexMismatch,
        ),
    ] {
        if let Some(filter) = definition {
            admissions(&seed)
                .create_index(
                    mongodb::IndexModel::builder()
                        .keys(doc! {"state":1,"_id":1})
                        .options(
                            mongodb::options::IndexOptions::builder()
                                .name(INDEX.to_owned())
                                .partial_filter_expression(filter)
                                .build(),
                        )
                        .build(),
                )
                .await
                .unwrap();
        }
        commands.lock().unwrap().clear();
        let read = store.read_with_history().await.unwrap();
        assert_eq!(read.counts.pending_launch_homes, 1);
        assert_eq!(read.counts.retained_homes.unwrap().observed, 1);
        assert_eq!(read.counts.pending_homes, expected);
        assert!(store.pending_homes.sweep(None).await.is_err());
        for preview in [true, false] {
            assert!(store
                .reconcile_pending_homes(
                    1,
                    &SubjectId::new("operator"),
                    "index unavailable",
                    preview,
                    100
                )
                .await
                .is_err());
        }
        let trace = commands.lock().unwrap().clone();
        assert!(writes(&trace).is_empty());
        assert!(!trace
            .iter()
            .any(|command| command.contains_key("createIndexes")
                || command.contains_key("dropIndexes")));
        assert_eq!(
            receipt(&seed, &lost).await.get_str("state").unwrap(),
            "reserved"
        );
    }
    admissions(&seed).drop_index(INDEX).await.unwrap();
    prepare_index(&seed.client, &seed.database).await;
    let read = store.read_with_history().await.unwrap();
    assert_eq!(
        read.counts.pending_homes,
        PendingHomesObservation::Verified {
            expected_pending_launch_homes: 1,
            pending_launch_homes_drift: 0
        }
    );
    assert_eq!(store.pending_homes.sweep(None).await.unwrap().released, 1);
    admissions(&seed).drop_index(INDEX).await.unwrap();
    assert_eq!(
        store
            .read_with_history()
            .await
            .unwrap()
            .counts
            .pending_homes,
        PendingHomesObservation::IndexMissing
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}
