use super::*;
use std::time::{Duration, SystemTime};

pub(super) fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 16000,
        min_memory_mib: 128,
        max_memory_mib: 32768,
    }
}

pub(super) async fn fixture(
    count: u8,
) -> (
    Client,
    String,
    MongoSandboxStore,
    MongoProfileStore,
    SandboxProfile,
    FixtureClock,
) {
    let (client, name) = database("warmup").await;
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.warm_runtime_version = Some(IMAGE.into());
    let profiles = MongoProfileStore::open(&client, &name).await.unwrap();
    // Claim eligibility uses Mongo's $$NOW. Anchor each fixture on that
    // server so an ahead worker clock cannot seed an ineligible warm row.
    let hello = client
        .database(&name)
        .run_command(doc! {"hello":1})
        .await
        .unwrap();
    let clock = FixtureClock(hello.get_datetime("localTime").unwrap().to_system_time());
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
        created_at: clock.stamp(0).as_str().into(),
        updated_at: clock.stamp(0).as_str().into(),
        retired_at: None,
    };
    profiles
        .create_sandbox_profile(profile.clone())
        .await
        .unwrap();
    let scanner: [String; 0] = [];
    profiles
        .publish_sandbox_profile(
            &profile.id,
            1,
            clock.stamp(0).as_str().into(),
            bounds(),
            &scanner,
        )
        .await
        .unwrap();
    profile.availability.warmup_enabled = true;
    profile.availability.warmup_count = count;
    profile.draft_version = 2;
    profiles
        .save_sandbox_draft(1, profile.clone())
        .await
        .unwrap();
    (client, name, store, profiles, profile, clock)
}

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    profile: &SandboxProfile,
    clock: FixtureClock,
) {
    store
        .reconcile_warmup_target(
            &profile.id,
            IMAGE,
            &[],
            ProvisioningExecution::default(),
            clock.stamp(0).as_str(),
        )
        .await
        .unwrap();
}

pub(super) async fn records(store: &MongoSandboxStore) -> Vec<WarmSandbox> {
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

pub(super) async fn make_ready(store: &MongoSandboxStore, clock: FixtureClock) -> SandboxId {
    let claim = store
        .claim_warmup_work(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .unwrap();
    let mut next = claim.record.clone();
    next.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{}", next.id),
        runtime_version: IMAGE.into(),
        provision_generation: Some(1),
        bound_at: clock.stamp(0).as_str().into(),
    });
    next.installed_request_sha256 = Some("a".repeat(64));
    next.state = WarmSandboxState::Ready;
    let id = next.id.clone();
    store
        .finish_warmup_work(
            &claim,
            next,
            clock.stamp(0).as_str(),
            clock.stamp(60).as_str(),
        )
        .await
        .unwrap();
    id
}

#[derive(Clone, Copy)]
pub(super) struct FixtureClock(SystemTime);

impl FixtureClock {
    pub(super) fn stamp(self, seconds: u64) -> String {
        layer2_core::rfc3339_at(self.0 + Duration::from_secs(seconds))
    }
}

pub(super) fn admission(
    id: &str,
    launch: &str,
    limit: u16,
    clock: FixtureClock,
) -> SandboxAdmission {
    let mut request = super::super::admission(id, launch, limit);
    request.event_at = clock.stamp(0);
    request
}

pub(super) async fn expire(store: &MongoSandboxStore, id: &SandboxId) {
    store.warm_sandboxes.update_one(doc! {"_id":id.as_str()},
        doc! {"$set":{"lease_until":"1970-01-01T00:00:00Z", "check_after":"1970-01-01T00:00:00Z"}})
        .await.unwrap();
}
