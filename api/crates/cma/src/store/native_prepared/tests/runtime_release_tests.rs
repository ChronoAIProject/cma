use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_promotion_refuses_old_spares_but_preserves_owned_replay() {
    let (client, name, store) = database().await;
    let old = plan();
    let store = store.with_managed_runtime_version(old.warm.runtime_version.clone());
    store
        .reconcile_native_prepared_target(Some(&old), 1, NOW)
        .await
        .unwrap();
    let first = publish_ready(&store).await;
    let existing = admission(&store, &old, "owner", "before-promotion").await;
    let SandboxAdmissionResult::Created(owned) =
        admit_at(&store, existing.clone(), NOW).await.unwrap()
    else {
        panic!("fresh owned admission");
    };
    assert_eq!(owned.id, first.payload.id);

    store
        .reconcile_native_prepared_target(Some(&old), 1, NOW)
        .await
        .unwrap();
    let spare = publish_ready(&store).await;
    let mut next = old.clone();
    next.warm.runtime_version = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    next.validate().unwrap();
    let promoted = store
        .clone()
        .with_managed_runtime_version(next.warm.runtime_version.clone());
    // Model the interval before the new preparation worker's first target sweep.
    let request = admission(&promoted, &next, "owner", "during-promotion").await;
    let mut session = promoted.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let outcome = super::super::admission::claim_ready(
        &promoted,
        &mut request.sandbox.clone(),
        NOW,
        &mut session,
    )
    .await
    .map_err(|_| ())
    .unwrap();
    session.abort_transaction().await.unwrap();
    assert_eq!(
        outcome,
        ClaimOutcome::Miss("miss_released_runtime_mismatch")
    );
    let SandboxAdmissionResult::Created(fallback) =
        admit_at(&promoted, request, NOW).await.unwrap()
    else {
        panic!("fresh admission must continue through ordinary provisioning");
    };
    assert!(fallback.runtime.is_none());
    assert_ne!(fallback.id, spare.payload.id);
    let retained = promoted
        .native_prepared_record(&spare.payload.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained, spare);

    let SandboxAdmissionResult::Existing(replay) =
        admit_at(&promoted, existing, NOW).await.unwrap()
    else {
        panic!("previous ownership must remain replayable");
    };
    assert_eq!(replay, owned);
    assert_eq!(
        replay.runtime.unwrap().runtime_version,
        old.warm.runtime_version
    );

    promoted
        .reconcile_native_prepared_target(Some(&next), 1, NOW)
        .await
        .unwrap();
    let replacement = publish_ready(&promoted).await;
    let current = admission(&promoted, &next, "owner", "after-promotion").await;
    let SandboxAdmissionResult::Created(current) = admit_at(&promoted, current, NOW).await.unwrap()
    else {
        panic!("current prepared capacity must still be used");
    };
    assert_eq!(current.id, replacement.payload.id);
    assert_eq!(
        current.runtime.unwrap().runtime_version,
        next.warm.runtime_version
    );
    assert_eq!(
        promoted
            .native_prepared_record(&spare.payload.id)
            .await
            .unwrap()
            .unwrap()
            .payload
            .state,
        WarmSandboxState::Retiring
    );
    client.database(&name).drop().await.unwrap();
}
