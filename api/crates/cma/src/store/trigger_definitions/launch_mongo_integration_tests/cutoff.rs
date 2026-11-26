use super::*;
use crate::store::settings_barrier::test_support::{set_ceilings, Pause, Point};
use layer2_core::{
    admission::AdmissionCeilings, trigger_domain::LaunchAdmissionPolicy, PlatformSettingsStore,
};
use std::time::SystemTime;

fn candidate(
    trigger: &PublicTrigger,
    key: char,
    visitor: bool,
    created_at: &str,
) -> LaunchAdmission {
    let mut launch = launch_at(trigger, key, key, created_at);
    if visitor {
        launch.authority = LaunchAuthorityClass::Visitor;
    }
    LaunchAdmission {
        launch,
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn operator_cutoff_restarts_visitor_daily_charge_under_both_policies() {
    for policy in [
        LaunchAdmissionPolicy::Baseline,
        LaunchAdmissionPolicy::VisitorAllowance,
    ] {
        for (limit, allowed) in [(0, false), (1, false), (5, true)] {
            let seed = seeded("visitor_cutoff").await;
            let trigger = published(&seed, "visitor_cutoff").await;
            let operator =
                crate::store::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
                    .await
                    .unwrap();
            let mut settings = layer2_core::PlatformSettings::default();
            settings.limits.visitor_max_launches_per_day = 10;
            operator.replace(0, &settings).await.unwrap();
            // Mongo expires visitor charges against its wall clock. Reuse one
            // timestamp for both admissions so they share the same quota day.
            let created_at = layer2_core::rfc3339_at(SystemTime::now());
            seed.store
                .admit_launch(candidate(&trigger, 'a', true, &created_at))
                .await
                .unwrap();
            let pause = Arc::new(Pause::default());
            let observed = pause.client().await;
            let store =
                MongoTriggerDefinitionStore::open(&observed, &seed.database, seed.profiles.clone())
                    .await
                    .unwrap()
                    .with_launch_admission_policy(policy);
            let request = candidate(&trigger, 'b', true, &created_at);
            let replay = request.clone();
            let release = pause.arm(Point::Snapshot);
            let admitting = tokio::spawn(async move { store.admit_launch(request).await });
            pause.wait().await;
            settings.limits.visitor_max_launches_per_day = limit;
            operator.replace(1, &settings).await.unwrap();
            drop(release);
            let result = admitting.await.unwrap();
            assert_eq!(result.is_ok(), allowed, "{policy:?}: {result:?}");
            if !allowed {
                assert!(
                    matches!(result, Err(LaunchStoreError::VisitorQuota { limit: actual, .. }) if actual == limit)
                );
            }
            assert!(
                pause.attempts() >= 2,
                "stale daily cap survived 10->{limit}"
            );
            let db = seed.client.database(&seed.database);
            assert_eq!(
                db.collection::<Document>(LAUNCHES)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                1 + u64::from(allowed)
            );
            let day = db
                .collection::<Document>("visitor_launch_days")
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(day.get_i64("used").unwrap(), 1 + i64::from(allowed));
            if allowed {
                settings.limits.visitor_max_launches_per_day = 0;
                operator.replace(2, &settings).await.unwrap();
                assert!(matches!(
                    seed.store.admit_launch(replay).await.unwrap(),
                    LaunchAdmissionResult::Existing(_)
                ));
                assert_eq!(
                    db.collection::<Document>("visitor_launch_days")
                        .find_one(doc! {})
                        .await
                        .unwrap()
                        .unwrap(),
                    day
                );
            }
            db.drop().await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn operator_cutoff_restarts_new_launch_home_reservations_for_both_authorities() {
    for visitor in [false, true] {
        for policy in [
            LaunchAdmissionPolicy::Baseline,
            LaunchAdmissionPolicy::VisitorAllowance,
        ] {
            for (old, limit, allowed) in [
                (Some(10), 0, false),
                (Some(10), 1, false),
                (Some(10), 5, true),
                (None, 1, true),
            ] {
                let seed = seeded("launch_home_cutoff").await;
                let trigger = published(&seed, "launch_home_cutoff").await;
                crate::store::MongoRegistrationCapacityStore::open(
                    &seed.client,
                    &seed.database,
                    true,
                )
                .await
                .unwrap();
                set_ceilings(
                    &seed.client,
                    &seed.database,
                    AdmissionCeilings {
                        retained_homes: old,
                        ..Default::default()
                    },
                )
                .await;
                let created_at = layer2_core::rfc3339_at(SystemTime::now());
                seed.store
                    .admit_launch(candidate(&trigger, 'a', visitor, &created_at))
                    .await
                    .unwrap();
                let pause = Arc::new(Pause::default());
                let observed = pause.client().await;
                let store = MongoTriggerDefinitionStore::open(
                    &observed,
                    &seed.database,
                    seed.profiles.clone(),
                )
                .await
                .unwrap()
                .with_launch_admission_policy(policy);
                let request = candidate(&trigger, 'b', visitor, &created_at);
                let release = pause.arm(Point::Owner);
                let admitting = tokio::spawn(async move { store.admit_launch(request).await });
                pause.wait().await;
                set_ceilings(
                    &seed.client,
                    &seed.database,
                    AdmissionCeilings {
                        retained_homes: Some(limit),
                        ..Default::default()
                    },
                )
                .await;
                drop(release);
                let result = admitting.await.unwrap();
                assert_eq!(
                    result.is_ok(),
                    allowed,
                    "visitor={visitor}, {policy:?}: {result:?}"
                );
                assert!(pause.attempts() >= 2);
                let db = seed.client.database(&seed.database);
                assert_eq!(
                    db.collection::<Document>(LAUNCHES)
                        .count_documents(doc! {})
                        .await
                        .unwrap(),
                    1 + u64::from(allowed)
                );
                let row = db
                    .collection::<Document>("sandbox_admissions")
                    .find_one(crate::store::launch_home_capacity::fence_key())
                    .await
                    .unwrap();
                assert_eq!(
                    crate::store::launch_home_capacity::pending_homes(row.as_ref()).unwrap(),
                    u64::from(old.is_some()) + u64::from(allowed)
                );
                if visitor {
                    let day = db
                        .collection::<Document>("visitor_launch_days")
                        .find_one(doc! {})
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(
                        day.get_datetime("expires_at").unwrap().to_system_time()
                            > SystemTime::now()
                    );
                    assert_eq!(
                        day.get_i64("used").unwrap(),
                        1 + i64::from(allowed),
                        "refused home must roll back its daily charge"
                    );
                }
                db.drop().await.unwrap();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn sandbox_and_launch_reservation_serialize_the_last_home_in_both_orders() {
    use layer2_core::sandbox::store::{
        SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxStoreError,
    };
    for launch_wins in [false, true] {
        let seed = seeded("home_cross_path").await;
        let trigger = published(&seed, "home_cross_path").await;
        crate::store::MongoRegistrationCapacityStore::open(&seed.client, &seed.database, true)
            .await
            .unwrap();
        set_ceilings(
            &seed.client,
            &seed.database,
            AdmissionCeilings {
                retained_homes: Some(1),
                ..Default::default()
            },
        )
        .await;
        let pause = Arc::new(Pause::default());
        let observed = pause.client().await;
        let sandboxes = crate::store::MongoSandboxStore::open(&observed, &seed.database)
            .await
            .unwrap();
        let launches =
            MongoTriggerDefinitionStore::open(&observed, &seed.database, seed.profiles.clone())
                .await
                .unwrap();
        let mut sandbox = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
            "home-cross-path",
            "unused-launch",
        );
        sandbox.origin = layer2_core::sandbox::SandboxOrigin::Manual;
        sandbox.owner = SubjectId::new("ordinary-owner");
        sandbox.name = "Home".into();
        let home = SandboxAdmission::new(
            sandbox,
            SandboxAdmissionName::Allocate("Home".into()),
            8,
            NOW.into(),
        )
        .unwrap();
        let request = candidate(&trigger, 'c', false, NOW);
        let release = pause.arm(Point::Snapshot);
        if launch_wins {
            let pending = tokio::spawn(async move { sandboxes.admit_sandbox(home).await });
            pause.wait().await;
            seed.store.admit_launch(request).await.unwrap();
            drop(release);
            assert!(matches!(
                pending.await.unwrap(),
                Err(SandboxStoreError::GlobalCapacityReached)
            ));
        } else {
            let pending = tokio::spawn(async move { launches.admit_launch(request).await });
            pause.wait().await;
            crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
                .await
                .unwrap()
                .admit_sandbox(home)
                .await
                .unwrap();
            drop(release);
            assert!(matches!(
                pending.await.unwrap(),
                Err(LaunchStoreError::GlobalCapacityReached)
            ));
        }
        assert!(pause.attempts() >= 2);
        let db = seed.client.database(&seed.database);
        let live = db
            .collection::<Document>("sandboxes")
            .count_documents(doc! {})
            .await
            .unwrap();
        let row = db
            .collection::<Document>("sandbox_admissions")
            .find_one(crate::store::launch_home_capacity::fence_key())
            .await
            .unwrap();
        let reserved = crate::store::launch_home_capacity::pending_homes(row.as_ref()).unwrap();
        assert_eq!(
            (live, reserved),
            (u64::from(!launch_wins), u64::from(launch_wins))
        );
        assert_eq!(live + reserved, 1);
        assert_eq!(
            db.collection::<Document>(LAUNCHES)
                .count_documents(doc! {})
                .await
                .unwrap(),
            reserved
        );
        db.drop().await.unwrap();
    }
}
