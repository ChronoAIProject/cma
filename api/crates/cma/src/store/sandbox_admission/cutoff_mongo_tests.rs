use super::*;
use crate::store::settings_barrier::test_support::{set_ceilings, Pause, Point};
use layer2_core::{sandbox::store::SandboxLimitClass, PlatformSettingsStore};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn operator_cutoff_restarts_home_admission_even_when_reduced_cap_has_room() {
    for (old, new, allowed) in [
        (Some(10), 0, false),
        (Some(10), 1, false),
        (Some(10), 5, true),
        (None, 2, true),
    ] {
        let (client, name) = database("home_cutoff").await;
        let control = MongoSandboxStore::open(&client, &name).await.unwrap();
        configure(
            &control,
            AdmissionCeilings {
                retained_homes: old,
                ..Default::default()
            },
        )
        .await;
        control.admit_sandbox(fresh(0)).await.unwrap();
        let pause = Arc::new(Pause::default());
        let observed = pause.client().await;
        let candidate = MongoSandboxStore::open(&observed, &name).await.unwrap();
        let release = pause.arm(Point::Snapshot);
        let admitting = tokio::spawn(async move { candidate.admit_sandbox(fresh(1)).await });
        pause.wait().await;
        set_ceilings(
            &client,
            &name,
            AdmissionCeilings {
                retained_homes: Some(new),
                ..Default::default()
            },
        )
        .await;
        drop(release);
        let result = admitting.await.unwrap();
        assert_eq!(result.is_ok(), allowed, "{result:?}");
        if !allowed {
            assert!(matches!(
                result,
                Err(SandboxStoreError::GlobalCapacityReached)
            ));
        }
        assert!(
            pause.attempts() >= 2,
            "old policy transaction survived {old:?}->{new}"
        );
        assert_eq!(
            control.sandboxes.count_documents(doc! {}).await.unwrap(),
            1 + u64::from(allowed)
        );
        assert_eq!(
            control
                .sandbox_events
                .count_documents(doc! {})
                .await
                .unwrap(),
            1 + u64::from(allowed)
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn operator_cutoff_refreshes_both_sandbox_cap_classes_on_the_same_admission() {
    for class in [SandboxLimitClass::Owner, SandboxLimitClass::Visitor] {
        for (lowered, allowed) in [(0, false), (1, false), (5, true)] {
            let (client, name) = database("owner_cutoff").await;
            let control = MongoSandboxStore::open(&client, &name).await.unwrap();
            let operator = crate::store::MongoPlatformSettingsStore::open(&client, &name)
                .await
                .unwrap();
            let mut settings = layer2_core::PlatformSettings::default();
            settings.limits.visitor_max_sandboxes = 10;
            operator.replace(0, &settings).await.unwrap();
            let mut existing = fresh(0);
            existing.limit_class = class;
            control.admit_sandbox(existing).await.unwrap();
            let mut admission = fresh(1);
            admission.sandbox.owner = SubjectId::new("ceiling-owner-0");
            admission =
                SandboxAdmission::new(admission.sandbox, admission.name, 8, NOW.into()).unwrap();
            admission.limit_class = class;
            let pause = Arc::new(Pause::default());
            let observed = pause.client().await;
            let candidate = MongoSandboxStore::open(&observed, &name).await.unwrap();
            let release = pause.arm(Point::Snapshot);
            let admitting = tokio::spawn(async move { candidate.admit_sandbox(admission).await });
            pause.wait().await;
            match class {
                SandboxLimitClass::Owner => settings.limits.max_sandboxes_per_user = lowered,
                SandboxLimitClass::Visitor => settings.limits.visitor_max_sandboxes = lowered,
            }
            operator.replace(1, &settings).await.unwrap();
            drop(release);
            let result = admitting.await.unwrap();
            assert_eq!(result.is_ok(), allowed, "{class:?} {result:?}");
            if !allowed {
                assert!(
                    matches!(result, Err(SandboxStoreError::LimitReached { limit, live: 1, .. }) if u32::from(limit) == lowered)
                );
            }
            assert!(pause.attempts() >= 2);
            assert_eq!(
                control.sandboxes.count_documents(doc! {}).await.unwrap(),
                1 + u64::from(allowed)
            );
            // A tighter caller bound remains effective under an ample current cap.
            if allowed {
                let mut tight = fresh(2);
                tight.live_limit = 0;
                assert!(matches!(
                    control.admit_sandbox(tight).await,
                    Err(SandboxStoreError::LimitReached { limit: 0, .. })
                ));
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}
