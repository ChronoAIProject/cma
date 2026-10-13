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

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 16000,
        min_memory_mib: 128,
        max_memory_mib: 32768,
    }
}

async fn fixture(
    count: u8,
) -> (
    Client,
    String,
    MongoSandboxStore,
    MongoProfileStore,
    SandboxProfile,
) {
    let (client, name) = database("warmup").await;
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.warm_runtime_version = Some(IMAGE.into());
    let profiles = MongoProfileStore::open(&client, &name).await.unwrap();
    let source = sandbox("sbx_unused", "unused");
    let mut profile = SandboxProfile {
        internal: false,
        id: source.profile.id,
        owner: source.owner,
        name: "Warm environment".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        availability: Default::default(),
        forked_from: None,
        uses_count: 0,
        draft: source.profile.content,
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    profiles
        .create_sandbox_profile(profile.clone())
        .await
        .unwrap();
    let scanner: [String; 0] = [];
    profiles
        .publish_sandbox_profile(&profile.id, 1, NOW.into(), bounds(), &scanner)
        .await
        .unwrap();
    profile.availability.warmup_enabled = true;
    profile.availability.warmup_count = count;
    profile.draft_version = 2;
    profiles
        .save_sandbox_draft(1, profile.clone())
        .await
        .unwrap();
    (client, name, store, profiles, profile)
}

async fn reconcile(store: &MongoSandboxStore, profile: &SandboxProfile) {
    store
        .reconcile_warmup_target(
            &profile.id,
            IMAGE,
            &[],
            ProvisioningExecution::default(),
            NOW,
        )
        .await
        .unwrap();
}

async fn records(store: &MongoSandboxStore) -> Vec<WarmSandbox> {
    store
        .warm_sandboxes
        .find(doc! {})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .iter()
        .map(|document| crate::store::warm_sandboxes::read(document).unwrap())
        .collect()
}

async fn make_ready(store: &MongoSandboxStore) -> SandboxId {
    let claim = store.claim_warmup_work(NOW, LATER).await.unwrap().unwrap();
    let mut next = claim.record.clone();
    next.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{}", next.id),
        runtime_version: IMAGE.into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    next.installed_request_sha256 = Some("a".repeat(64));
    next.state = WarmSandboxState::Ready;
    let id = next.id.clone();
    store
        .finish_warmup_work(&claim, next, NOW, LATER)
        .await
        .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_replenishment_caps_reservations_and_claims_are_exclusive() {
    let (client, name, store, _, profile) = fixture(5).await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (store, profile) = (store.clone(), profile.clone());
        tasks.spawn(async move { reconcile(&store, &profile).await });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(records(&store).await.len(), 5);
    let mut prepared = std::collections::BTreeSet::new();
    for _ in 0..5 {
        prepared.insert(make_ready(&store).await);
    }
    let mut tasks = tokio::task::JoinSet::new();
    for number in 0..10 {
        let store = store.clone();
        tasks.spawn(async move {
            let mut request = admission(
                &format!("sbx_cold_{number}"),
                &format!("launch_{number}"),
                20,
            );
            request.sandbox.owner = SubjectId::new(format!("visitor_{number}"));
            let request =
                SandboxAdmission::new(request.sandbox, request.name, 20, NOW.into()).unwrap();
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
    reconcile(&store, &profile).await;
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
    let (client, name, store, _, profile) = fixture(2).await;
    reconcile(&store, &profile).await;
    let first = make_ready(&store).await;
    make_ready(&store).await;
    let request = admission("sbx_cold", "launch-retry", 1);
    let SandboxAdmissionResult::Created(created) =
        store.admit_sandbox(request.clone()).await.unwrap()
    else {
        panic!("expected creation")
    };
    assert_eq!(created.id, first);
    let mut retry = admission("sbx_different_candidate", "launch-retry", 0);
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
            .admit_sandbox(admission("sbx_overquota", "launch-other", 1))
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
    let (client, name, store, profiles, profile) = fixture(1).await;
    reconcile(&store, &profile).await;
    let ready = make_ready(&store).await;
    let locator = MarketplaceLocator::Sandbox(profile.id.clone());
    let layer2_core::profiles::ForkedProfile::Sandbox(fork) = profiles
        .fork_profile(&SubjectId::new("visitor"), &locator, 1, NOW.into())
        .await
        .unwrap()
    else {
        panic!("sandbox fork")
    };
    let scanner: [String; 0] = [];
    let revision = profiles
        .publish_sandbox_profile(&fork.id, 1, NOW.into(), bounds(), &scanner)
        .await
        .unwrap();
    let mut copy = sandbox("sbx_copy", "copy");
    copy.profile = SandboxProfileSnapshot::from(&revision);
    let request = SandboxAdmission::new(
        copy,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        NOW.into(),
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
            NOW.into(),
            bounds(),
            &scanner,
        )
        .await
        .unwrap();
    let SandboxAdmissionResult::Created(cold) = store
        .admit_sandbox(admission("sbx_unshared", "unshared", 10))
        .await
        .unwrap()
    else {
        panic!("unshared")
    };
    assert!(cold.runtime.is_none());
    reconcile(&store, &profile).await;
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
    let (client, name, store, profiles, mut profile) = fixture(1).await;
    reconcile(&store, &profile).await;
    let old = store.claim_warmup_work(NOW, LATER).await.unwrap().unwrap();
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
        .publish_sandbox_profile(&profile.id, 3, NOW.into(), bounds(), &scanner)
        .await
        .unwrap();
    reconcile(&store, &profile).await;
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
        store.renew_warmup_work(&old, LATER).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(matches!(
        store
            .finish_warmup_work(&old, old.record.clone(), NOW, NOW)
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
    let cleanup = store
        .claim_warmup_work(LATER, "2026-09-05T00:02:00Z")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup.record.id, old.record.id);
    let mut retired = cleanup.record.clone();
    retired.state = WarmSandboxState::Retired;
    store
        .finish_warmup_work(&cleanup, retired, LATER, LATER)
        .await
        .unwrap();
    reconcile(&store, &profile).await;
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
    let (client, name, store, _, profile) = fixture(1).await;
    reconcile(&store, &profile).await;
    let id = make_ready(&store).await;
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
        .admit_sandbox(admission("sbx_cold", "launch-handoff", 1))
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
    let (client, name, mut store, _, profile) = fixture(1).await;
    reconcile(&store, &profile).await;
    let preparing = store.claim_warmup_work(NOW, LATER).await.unwrap().unwrap();
    let mut next = preparing.record.clone();
    let mut home = layer2_core::sandbox::SandboxHome::reserve(
        &next.id,
        layer3_adapter::HomeVolumePolicy {
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
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
        bound_at: NOW.into(),
    });
    next.installed_request_sha256 = Some("a".repeat(64));
    next.state = WarmSandboxState::Ready;
    store
        .finish_warmup_work(&preparing, next, NOW, NOW)
        .await
        .unwrap();
    assert!(store.unowned_home_authority(&home.request).await.unwrap());
    let expired_at = "2026-09-05T00:00:10Z";
    let stale = store
        .claim_warmup_work(NOW, expired_at)
        .await
        .unwrap()
        .unwrap();
    let mut request = admission("sbx_cold", "launch-home-handoff", 1);
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
