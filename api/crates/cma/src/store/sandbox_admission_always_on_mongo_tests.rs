use super::*;
use futures_util::FutureExt;
use layer2_core::sandbox::store::SandboxMaintenanceStore;
use std::panic::AssertUnwindSafe;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn always_on_updates_keep_original_admission_readable_across_store_instances() {
    let (client, database) = database("always_on_replay").await;
    let result = AssertUnwindSafe(async {
        let first_store = MongoSandboxStore::open(&client, &database).await.unwrap();
        let second_store = MongoSandboxStore::open(&client, &database).await.unwrap();
        for lifecycle in [false, true] {
            for (index, initial) in [None, Some(false), Some(true)].into_iter().enumerate() {
                let key = format!("policy-{lifecycle}-{index}");
                let mut value = sandbox(&format!("sbx_{key}"), &key);
                if lifecycle {
                    value.origin = SandboxOrigin::Lifecycle { operation_id: key };
                }
                value.profile.always_on = initial;
                let original = SandboxAdmission::new(
                    value,
                    SandboxAdmissionName::Allocate("trigger-research".into()),
                    100,
                    NOW.into(),
                )
                .unwrap();
                let SandboxAdmissionResult::Created(mut current) =
                    first_store.admit_sandbox(original.clone()).await.unwrap()
                else {
                    panic!("new origin must create");
                };
                for (writer, reader, policy) in [
                    (&first_store, &second_store, true),
                    (&second_store, &first_store, false),
                    (&first_store, &second_store, true),
                ] {
                    let previous = current.revision;
                    current = writer
                        .set_sandbox_always_on(current.id.clone(), previous, policy)
                        .await
                        .unwrap();
                    assert_eq!(current.profile.always_on, Some(policy));
                    assert_eq!(
                        reader.get_sandbox(&current.id).await.unwrap(),
                        Some(current.clone())
                    );
                    assert_eq!(
                        reader
                            .set_sandbox_always_on(current.id.clone(), previous, !policy)
                            .await,
                        Err(SandboxStoreError::Conflict)
                    );
                    let replay = reader
                        .sandbox_admission_replay(&current.owner, &current.origin)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(replay.sandbox, current);
                    assert_eq!(replay.admission_fingerprint, original.admission_fingerprint);
                    assert_eq!(
                        reader.admit_sandbox(original.clone()).await.unwrap(),
                        SandboxAdmissionResult::Existing(current.clone())
                    );
                    let document = reader
                        .sandboxes
                        .find_one(doc! {"_id": current.id.as_str()})
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        document.get_str("admission_fingerprint").unwrap(),
                        original.admission_fingerprint
                    );
                    for changed_policy in [None, Some(false), Some(true)] {
                        if changed_policy == initial {
                            continue;
                        }
                        let mut changed = original.sandbox.clone();
                        changed.profile.always_on = changed_policy;
                        let changed =
                            SandboxAdmission::new(changed, original.name.clone(), 100, NOW.into())
                                .unwrap();
                        assert_eq!(
                            reader.admit_sandbox(changed).await,
                            Err(SandboxStoreError::SandboxOriginConflict)
                        );
                    }
                }
                // A newly composed store after the writes must recover the same
                // immutable admission identity without process-local state.
                let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
                assert_eq!(
                    reopened.get_sandbox(&current.id).await.unwrap(),
                    Some(current.clone())
                );
                assert_eq!(
                    reopened.admit_sandbox(original).await.unwrap(),
                    SandboxAdmissionResult::Existing(current)
                );
            }
        }
    })
    .catch_unwind()
    .await;
    let cleanup = client.database(&database).drop().await;
    cleanup.expect("Always-on regression database must be removed before returning");
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
