use super::*;
use crate::store::settings_barrier::test_support::{platform, Pause, Point};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn operator_cutoff_restarts_registration_even_with_room_and_before_activation() {
    for (active, old, new, allowed, missing) in [
        (true, Some(10), 0, false, false),
        (true, Some(10), 1, false, false),
        (true, Some(10), 5, true, false),
        (true, None, 2, true, false),
        (false, None, 0, false, false),
        (false, None, 5, false, false),
        (false, None, 0, false, true),
    ] {
        let (client, name, control) = fixture(active).await;
        if active {
            assert!(control.admit(&SubjectId::new("existing")).await.unwrap());
        }
        if !active && !missing {
            use layer2_core::PlatformSettingsStore;
            crate::store::MongoPlatformSettingsStore::open(&client, &name)
                .await
                .unwrap()
                .replace(0, &Default::default())
                .await
                .unwrap();
        }
        set_limit(&control, old).await;
        let pause = Arc::new(Pause::default());
        let observed = pause.client().await;
        let candidate = MongoRegistrationCapacityStore::open(&observed, &name, false)
            .await
            .unwrap();
        let release = pause.arm(Point::Snapshot);
        let admitting = tokio::spawn(async move {
            candidate
                .admit_with_limit(&SubjectId::new("candidate"), Ok(None))
                .await
        });
        pause.wait().await;
        set_limit(&control, Some(new)).await;
        drop(release);
        assert_eq!(admitting.await.unwrap().unwrap(), allowed);
        assert!(
            pause.attempts() >= 2,
            "stale attempt committed under {old:?}->{new}"
        );
        assert_eq!(
            control
                .admissions
                .count_documents(doc! {"_id.scope": "registration"})
                .await
                .unwrap(),
            u64::from(active) + u64::from(allowed)
        );
        if active {
            assert_eq!(count(&control).await, 1 + u64::from(allowed));
        } else {
            assert!(control
                .admissions
                .find_one(capacity_key())
                .await
                .unwrap()
                .is_none());
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn activation_invalidates_an_old_untracked_registration_snapshot() {
    use layer2_core::PlatformSettingsStore;
    let (client, name, _) = fixture(false).await;
    let settings = crate::store::MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    settings.replace(0, &Default::default()).await.unwrap();
    let pause = Arc::new(Pause::default());
    let observed = pause.client().await;
    let candidate = MongoRegistrationCapacityStore::open(&observed, &name, false)
        .await
        .unwrap();
    let release = pause.arm(Point::Snapshot);
    let admitting =
        tokio::spawn(async move { candidate.admit(&SubjectId::new("new-cohort")).await });
    pause.wait().await;
    let activated = MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    drop(release);
    assert!(admitting.await.unwrap().unwrap());
    assert!(pause.attempts() >= 2);
    assert_eq!(count(&activated).await, 1);
    assert!(activated
        .admissions
        .find_one(registration_key(&SubjectId::new("new-cohort")))
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn registration_committed_before_cutoff_can_ack_late_and_replay_once() {
    let (client, name, control) = fixture(true).await;
    set_limit(&control, Some(10)).await;
    let pause = Arc::new(Pause::default());
    let observed = pause.client().await;
    let candidate = MongoRegistrationCapacityStore::open(&observed, &name, false)
        .await
        .unwrap();
    let release = pause.arm(Point::CommitAck);
    let admitting = tokio::spawn(async move { candidate.admit(&SubjectId::new("late-ack")).await });
    pause.wait().await;
    set_limit(&control, Some(0)).await;
    assert!(!admitting.is_finished());
    assert_eq!(count(&control).await, 1);
    drop(release);
    assert!(admitting.await.unwrap().unwrap());
    assert_eq!(pause.attempts(), 1);
    assert_eq!(pause.commits(), 1);
    let before = platform(&client, &name).await;
    assert!(control.admit(&SubjectId::new("late-ack")).await.unwrap());
    assert_eq!(platform(&client, &name).await, before);
    assert_eq!(count(&control).await, 1);
    client.database(&name).drop().await.unwrap();
}
