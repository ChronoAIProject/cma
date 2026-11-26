//! Public-owner qualification uses live settings and the existing admission service.
//! The shared in-memory store proves the service contract, not Mongo fleet isolation.

#[path = "../src/sandbox/policy.rs"]
mod policy;

#[path = "public_owner_sandbox_limit/visitor.rs"]
mod visitor;

use std::sync::{Arc, Mutex};

use layer2_core::{
    profiles::{
        InMemoryProfileStore, ModelAccess, ProfileCapacityBounds, ProfileVisibility,
        SandboxProfile, SandboxProfileContent, SandboxProfileRef, SandboxProfileStore,
    },
    sandbox::{
        CreateSandbox, FixedSandboxCredentialPrerequisite, InMemorySandboxStore, PageRequest,
        Sandbox, SandboxOrigin, SandboxPolicySource, SandboxService, SandboxServiceError,
        SandboxStatus, SandboxWorkStep,
    },
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Caller,
    Capability, Page, PlatformSettings, PlatformSettingsService, PlatformSettingsStore,
    SandboxProfileId, StoreError, StoredPlatformSettings, SubjectId,
};
use policy::LiveSandboxPolicy;

const NOW: &str = "2026-01-01T00:00:00Z";

struct Settings(Mutex<StoredPlatformSettings>);

impl Settings {
    fn set_owner_limit(&self, limit: u32) {
        let mut stored = self.0.lock().unwrap();
        stored.revision += 1;
        stored.settings.limits.max_sandboxes_per_user = limit;
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        panic!("sandbox admission must not mutate platform settings")
    }
}

struct NoActivityWrites;

#[async_trait::async_trait]
impl ActivityStore for NoActivityWrites {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        panic!("reading live policy must not write settings activity")
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("reading live policy must not query activity")
    }
}

struct Fixture {
    settings: Arc<Settings>,
    policy: Arc<LiveSandboxPolicy>,
    profiles: Arc<InMemoryProfileStore>,
    store: Arc<InMemorySandboxStore>,
    reference: SandboxProfileRef,
}

impl Fixture {
    async fn new() -> Self {
        let settings = Arc::new(Settings(Mutex::new(StoredPlatformSettings {
            revision: 1,
            settings: PlatformSettings::default(),
        })));
        let policy = Arc::new(LiveSandboxPolicy::new(
            Arc::new(PlatformSettingsService::new(
                settings.clone(),
                ActivityService::new(Arc::new(NoActivityWrites)),
            )),
            Vec::new(),
        ));
        let profiles = Arc::new(InMemoryProfileStore::default());
        let id = SandboxProfileId("sbp_public_limit_fixture".into());
        profiles
            .create_sandbox_profile(SandboxProfile {
                id: id.clone(),
                owner: SubjectId::new("synthetic-publisher"),
                name: "Public limit fixture".into(),
                description: "Synthetic admission qualification".into(),
                internal: false,
                availability: Default::default(),
                visibility: ProfileVisibility::Public,
                forked_from: None,
                uses_count: 0,
                draft: SandboxProfileContent {
                    runtime_image: None,
                    cpu_millis: 1_000,
                    memory_mib: 2_048,
                    nyxid_services: Vec::new(),
                    agent_provider: Default::default(),
                    model_access: ModelAccess::ChronoaiManaged,
                    max_workspaces: 1,
                    max_agents_per_workspace: 1,
                    workspace_profiles: Vec::new(),
                    software_install_commands: Vec::new(),
                    environment_variables: Vec::new(),
                },
                draft_version: 1,
                created_at: NOW.into(),
                updated_at: NOW.into(),
                retired_at: None,
            })
            .await
            .unwrap();
        let revision = profiles
            .publish_sandbox_profile(
                &id,
                1,
                NOW.into(),
                ProfileCapacityBounds {
                    min_cpu_millis: 100,
                    max_cpu_millis: 8_000,
                    min_memory_mib: 128,
                    max_memory_mib: 32_768,
                },
                &[],
            )
            .await
            .unwrap();
        Self {
            settings,
            policy,
            profiles,
            store: Arc::new(InMemorySandboxStore::default()),
            reference: SandboxProfileRef::new(id, revision.revision),
        }
    }

    fn service(&self) -> Arc<SandboxService> {
        SandboxService::with_policy_source(
            self.store.clone(),
            self.profiles.clone(),
            self.policy.clone(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        )
    }

    fn request(&self) -> CreateSandbox {
        CreateSandbox {
            profile: self.reference.clone(),
            name: None,
            confirmed_digest: None,
            origin: SandboxOrigin::Manual,
        }
    }

    async fn uses(&self) -> u64 {
        self.profiles
            .sandbox_profile(&self.reference.id)
            .await
            .unwrap()
            .unwrap()
            .uses_count
    }
}

fn owner(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::CreateSandbox, Capability::UseOwnSandboxes]),
    )
}

fn assert_limit(result: Result<Sandbox, SandboxServiceError>, live: u16) {
    let error = result.expect_err("the configured owner limit must refuse admission");
    assert_eq!(error.code(), "sandbox_limit_reached");
    assert_eq!(
        error,
        SandboxServiceError::SandboxLimitReached {
            limit: 2,
            live,
            usage: Default::default(),
        }
    );
}

#[tokio::test]
async fn configured_two_refuses_third_without_changing_accepted_sandboxes() {
    let fixture = Fixture::new().await;
    let defaults = fixture.policy.current_sandbox_policy().await.unwrap();
    assert_eq!(defaults.max_sandboxes_per_owner, 10);
    assert_eq!(defaults.max_sandboxes_per_visitor, 1);
    fixture.settings.set_owner_limit(2);
    let service = fixture.service();
    let owner = owner("synthetic-owner");
    let first = service.create(&owner, fixture.request()).await.unwrap();
    let second = service.create(&owner, fixture.request()).await.unwrap();
    let before = service
        .list_own(&owner, &PageRequest::default())
        .await
        .unwrap();
    for _ in 0..3 {
        assert_limit(service.create(&owner, fixture.request()).await, 2);
    }
    assert_eq!(
        service
            .list_own(&owner, &PageRequest::default())
            .await
            .unwrap(),
        before
    );
    assert_eq!(fixture.uses().await, 2);
    for sandbox in [first, second] {
        assert_eq!(sandbox.status, SandboxStatus::Provisioning);
        assert_eq!(sandbox.work.step, SandboxWorkStep::ProvisionRuntime);
        assert!(sandbox.runtime.is_none());
        assert_eq!(service.read(&owner, &sandbox.id).await.unwrap(), sandbox);
        assert_eq!(
            service
                .events(&owner, &sandbox.id, 0, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_services_share_one_owner_allowance_under_concurrent_creation() {
    let fixture = Fixture::new().await;
    fixture.settings.set_owner_limit(2);
    let services = [fixture.service(), fixture.service()];
    let barrier = Arc::new(tokio::sync::Barrier::new(32));
    let mut tasks = Vec::new();
    for index in 0..32 {
        let service = services[index % 2].clone();
        let request = fixture.request();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            service.create(&owner("synthetic-owner"), request).await
        }));
    }
    let mut admitted = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            Ok(sandbox) => admitted.push(sandbox.id),
            refusal => assert_limit(refusal, 2),
        }
    }
    assert_eq!(admitted.len(), 2);
    assert_ne!(admitted[0], admitted[1]);
    assert_eq!(fixture.uses().await, 2);
    for service in services {
        assert_eq!(
            service
                .list_own(&owner("synthetic-owner"), &PageRequest::default())
                .await
                .unwrap()
                .items
                .len(),
            2
        );
        service
            .create(&owner("synthetic-other-owner"), fixture.request())
            .await
            .unwrap();
    }
    assert_eq!(fixture.uses().await, 4);
}

#[tokio::test]
async fn live_limit_reduction_preserves_homes_and_increase_resumes_admission() {
    let fixture = Fixture::new().await;
    fixture.settings.set_owner_limit(3);
    let service = fixture.service();
    let owner = owner("synthetic-owner");
    for _ in 0..3 {
        service.create(&owner, fixture.request()).await.unwrap();
    }
    let before = service
        .list_own(&owner, &PageRequest::default())
        .await
        .unwrap();
    fixture.settings.set_owner_limit(2);
    assert_limit(service.create(&owner, fixture.request()).await, 3);
    assert_eq!(
        service
            .list_own(&owner, &PageRequest::default())
            .await
            .unwrap(),
        before
    );
    assert_eq!(fixture.uses().await, 3);
    fixture.settings.set_owner_limit(4);
    service.create(&owner, fixture.request()).await.unwrap();
    assert_eq!(fixture.uses().await, 4);
}
