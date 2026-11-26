use super::*;
use crate::store::MongoProfileStore;
use futures_util::TryStreamExt;
use layer2_core::profiles::{
    MarketplaceLocator, MarketplaceStore, ProfileCapacityBounds, ProfileVisibility, SandboxProfile,
    SandboxProfileStore,
};
use layer2_core::sandbox::store::SandboxReconciliationStore;
use layer2_core::sandbox::{ProvisioningExecution, RuntimeBinding, WarmSandbox, WarmSandboxState};

const IMAGE: &str = "runtime-warm-test";

#[path = "home_policy_tests.rs"]
mod home_policy_tests;
#[path = "retirement_tests.rs"]
mod retirement_tests;

#[path = "test_fixture.rs"]
mod fixture;
use fixture::{admission, bounds, expire, fixture, make_ready, reconcile, records};
#[path = "lease_tests.rs"]
mod lease_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_replenishment_caps_reservations_and_claims_are_exclusive() {
    let (client, name, store, _, profile, clock) = fixture(5).await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (store, profile) = (store.clone(), profile.clone());
        tasks.spawn(async move { reconcile(&store, &profile, clock).await });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(records(&store).await.len(), 5);
    let mut prepared = std::collections::BTreeSet::new();
    for _ in 0..5 {
        prepared.insert(make_ready(&store, clock).await);
    }
    let mut tasks = tokio::task::JoinSet::new();
    for number in 0..10 {
        let store = store.clone();
        tasks.spawn(async move {
            let mut request = admission(
                &format!("sbx_cold_{number}"),
                &format!("launch_{number}"),
                20,
                clock,
            );
            request.sandbox.owner = SubjectId::new(format!("visitor_{number}"));
            let request = SandboxAdmission::new(
                request.sandbox,
                request.name,
                20,
                clock.stamp(0).as_str().into(),
            )
            .unwrap();
            store.admit_sandbox(request).await.unwrap()
        });
    }
    let mut owned = std::collections::BTreeSet::new();
    let mut warm_claims = 0;
    while let Some(result) = tasks.join_next().await {
        let SandboxAdmissionResult::Created(sandbox) = result.unwrap() else {
            panic!("unexpected replay")
        };
        assert!(owned.insert(sandbox.id.clone()));
        if sandbox.runtime.is_some() {
            warm_claims += 1;
            assert!(prepared.contains(&sandbox.id));
        }
        assert!(!sandbox.credential.delivered);
        assert!(sandbox.credential.current.is_none());
    }
    assert_eq!(warm_claims, 5);
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.state == WarmSandboxState::Claimed)
            .count(),
        5
    );
    reconcile(&store, &profile, clock).await;
    let remaining = records(&store).await;
    assert_eq!(
        remaining
            .iter()
            .filter(|r| r.state == WarmSandboxState::Preparing)
            .count(),
        5
    );
    assert!(remaining
        .iter()
        .filter(|r| r.state == WarmSandboxState::Preparing)
        .all(|r| !prepared.contains(&r.id)));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn warm_trigger_replay_precedes_quota_and_does_not_claim_again() {
    let (client, name, store, _, profile, clock) = fixture(2).await;
    reconcile(&store, &profile, clock).await;
    let first = make_ready(&store, clock).await;
    make_ready(&store, clock).await;
    let request = admission("sbx_cold", "launch-retry", 1, clock);
    let SandboxAdmissionResult::Created(created) =
        store.admit_sandbox(request.clone()).await.unwrap()
    else {
        panic!("expected creation")
    };
    assert_eq!(created.id, first);
    let mut retry = admission("sbx_different_candidate", "launch-retry", 0, clock);
    retry.admission_fingerprint = request.admission_fingerprint;
    let SandboxAdmissionResult::Existing(replayed) = store.admit_sandbox(retry).await.unwrap()
    else {
        panic!("expected replay")
    };
    assert_eq!(replayed, created);
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.state == WarmSandboxState::Ready)
            .count(),
        1
    );
    assert!(matches!(
        store
            .admit_sandbox(admission("sbx_overquota", "launch-other", 1, clock))
            .await,
        Err(SandboxStoreError::LimitReached { .. })
    ));
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.state == WarmSandboxState::Ready)
            .count(),
        1
    );
    assert_eq!(
        store
            .sandbox_events(&created.id, 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn private_copies_and_unshared_profiles_cannot_consume_source_pool() {
    let (client, name, store, profiles, profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let ready = make_ready(&store, clock).await;
    let locator = MarketplaceLocator::Sandbox(profile.id.clone());
    let layer2_core::profiles::ForkedProfile::Sandbox(fork) = profiles
        .fork_profile(
            &SubjectId::new("visitor"),
            &locator,
            1,
            clock.stamp(0).as_str().into(),
        )
        .await
        .unwrap()
    else {
        panic!("sandbox fork")
    };
    let scanner: [String; 0] = [];
    let revision = profiles
        .publish_sandbox_profile(
            &fork.id,
            1,
            clock.stamp(0).as_str().into(),
            bounds(),
            &scanner,
        )
        .await
        .unwrap();
    let mut copy = sandbox("sbx_copy", "copy");
    copy.profile = SandboxProfileSnapshot::from(&revision);
    let request = SandboxAdmission::new(
        copy,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        clock.stamp(0).as_str().into(),
    )
    .unwrap();
    let SandboxAdmissionResult::Created(copy) = store.admit_sandbox(request).await.unwrap() else {
        panic!("copy")
    };
    assert!(copy.runtime.is_none());
    assert_eq!(
        store.warm_sandbox(&ready).await.unwrap().unwrap().state,
        WarmSandboxState::Ready
    );
    profiles
        .set_profile_visibility(
            &profile.owner,
            &locator,
            ProfileVisibility::Private,
            clock.stamp(0).as_str().into(),
            bounds(),
            &scanner,
        )
        .await
        .unwrap();
    let SandboxAdmissionResult::Created(cold) = store
        .admit_sandbox(admission("sbx_unshared", "unshared", 10, clock))
        .await
        .unwrap()
    else {
        panic!("unshared")
    };
    assert!(cold.runtime.is_none());
    reconcile(&store, &profile, clock).await;
    assert_eq!(
        store.warm_sandbox(&ready).await.unwrap().unwrap().state,
        WarmSandboxState::Retiring
    );
    assert_eq!(records(&store).await.len(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn changing_environment_retires_old_pool_and_fences_inflight_workers() {
    let (client, name, store, profiles, mut profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let old = store
        .claim_warmup_work(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .unowned_runtime_authority(&old.record.id, 1, "future-runtime")
        .await
        .unwrap());
    profile.draft.memory_mib += 128;
    profile.draft_version = 3;
    profiles
        .save_sandbox_draft(2, profile.clone())
        .await
        .unwrap();
    let scanner: [String; 0] = [];
    profiles
        .publish_sandbox_profile(
            &profile.id,
            3,
            clock.stamp(0).as_str().into(),
            bounds(),
            &scanner,
        )
        .await
        .unwrap();
    reconcile(&store, &profile, clock).await;
    assert_eq!(
        store
            .warm_sandbox(&old.record.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        WarmSandboxState::Retiring
    );
    assert!(matches!(
        store
            .renew_warmup_work(&old, clock.stamp(60).as_str())
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(matches!(
        store
            .finish_warmup_work(
                &old,
                old.record.clone(),
                clock.stamp(0).as_str(),
                clock.stamp(0).as_str()
            )
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.state == WarmSandboxState::Preparing && r.plan.profile.revision == 2)
            .count(),
        0
    );
    // Replacement waits for exact teardown, so a provider outage cannot grow
    // an unbounded population of failed runtimes.
    expire(&store, &old.record.id).await;
    let cleanup = store
        .claim_warmup_work(clock.stamp(60).as_str(), clock.stamp(120).as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup.record.id, old.record.id);
    let mut retired = cleanup.record.clone();
    retired.state = WarmSandboxState::Retired;
    store
        .finish_warmup_work(
            &cleanup,
            retired,
            clock.stamp(60).as_str(),
            clock.stamp(60).as_str(),
        )
        .await
        .unwrap();
    reconcile(&store, &profile, clock).await;
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.state == WarmSandboxState::Preparing && r.plan.profile.revision == 2)
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claimed_reservation_transfers_reclaim_authority_to_owned_sandbox() {
    let (client, name, store, _, profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let id = make_ready(&store, clock).await;
    let binding = store
        .warm_sandbox(&id)
        .await
        .unwrap()
        .unwrap()
        .runtime
        .unwrap();
    assert!(store
        .unowned_runtime_authority(&id, 1, &binding.runtime_id)
        .await
        .unwrap());
    assert!(!store
        .unowned_runtime_authority(&id, 2, &binding.runtime_id)
        .await
        .unwrap());
    let SandboxAdmissionResult::Created(created) = store
        .admit_sandbox(admission("sbx_cold", "launch-handoff", 1, clock))
        .await
        .unwrap()
    else {
        panic!("created")
    };
    assert_eq!(created.id, id);
    assert!(!store
        .unowned_runtime_authority(&id, 1, &binding.runtime_id)
        .await
        .unwrap());
    assert_eq!(store.get_sandbox(&id).await.unwrap(), Some(created));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claimed_home_transfers_atomically_and_refuses_the_previous_warm_worker() {
    let (client, name, mut store, _, profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let preparing = store
        .claim_warmup_work(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .unwrap();
    let mut next = preparing.record.clone();
    let mut home = layer2_core::sandbox::SandboxHome::reserve(
        &next.id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-home".into(),
        storage_generation: "original-disk".into(),
    })
    .unwrap();
    store.warm_home_policy = Some(home.request.policy.clone());
    next.home = Some(Box::new(home.clone()));
    next.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{}", next.id),
        runtime_version: IMAGE.into(),
        provision_generation: Some(1),
        bound_at: clock.stamp(0).as_str().into(),
    });
    next.installed_request_sha256 = Some("a".repeat(64));
    next.state = WarmSandboxState::Ready;
    store
        .finish_warmup_work(
            &preparing,
            next,
            clock.stamp(0).as_str(),
            clock.stamp(0).as_str(),
        )
        .await
        .unwrap();
    assert!(store.unowned_home_authority(&home.request).await.unwrap());
    let expiry = clock.stamp(10);
    let expired_at = expiry.as_str();
    let stale = store
        .claim_warmup_work(clock.stamp(0).as_str(), expired_at)
        .await
        .unwrap()
        .unwrap();
    let mut request = admission("sbx_cold", "launch-home-handoff", 1, clock);
    request.event_at = expired_at.into();
    let SandboxAdmissionResult::Created(created) = store.admit_sandbox(request).await.unwrap()
    else {
        panic!("created")
    };
    assert_eq!(created.id, preparing.record.id);
    assert_eq!(created.lifecycle.home.as_deref(), Some(&home));
    assert!(!created.credential.delivered);
    assert!(created.credential.current.is_none());
    assert!(!store.unowned_home_authority(&home.request).await.unwrap());
    let durable = store.get_sandbox(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.lifecycle.home.as_deref(), Some(&home));
    assert!(matches!(
        store
            .finish_warmup_work(&stale, stale.record.clone(), expired_at, expired_at)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    client.database(&name).drop().await.unwrap();
}
