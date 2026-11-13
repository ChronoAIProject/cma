use super::*;

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

pub(super) async fn reconcile(store: &MongoSandboxStore, profile: &SandboxProfile) {
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

pub(super) async fn make_ready(store: &MongoSandboxStore) -> SandboxId {
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
