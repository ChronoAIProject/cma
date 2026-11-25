use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::profiles::store::{
    InMemoryProfileStore, ProfilePage, ProfilePageRequest, ProfileStoreError, ProfileUseRecorder,
    SandboxProfileStore, WorkspaceProfileStore,
};
use crate::profiles::{
    AgentProvider, EnvironmentVariable, MarketplaceLocator, ModelAccess, ProfileCapacityBounds,
    ProfileVisibility, SandboxProfile, SandboxProfileContent, SandboxProfileRef,
    SandboxProfileRevision, WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent,
    WorkspaceProfileRef, WorkspaceProfileRevision,
};
use crate::sandbox::store::{
    PageRequest, SandboxAdmission, SandboxAdmissionStore, SandboxEventStore, SandboxStore as _,
    SandboxTransaction, SandboxTransactionStore, SandboxWorkerStore as _,
};
use crate::{
    Authorization, Caller, Capability, SandboxId, SandboxProfileId, ServiceId, SubjectId,
    WorkspaceProfileId,
};

use super::service::{DEFAULT_INACTIVITY_PAUSE_AFTER_SECONDS, DEFAULT_RETENTION_GRACE};
use super::*;
use tokio::sync::Notify;

#[path = "service_creation_tests.rs"]
mod creation_tests;
#[path = "service_credential_tests.rs"]
mod credential_prerequisite_tests;
#[path = "service_lifecycle_tests.rs"]
mod lifecycle_tests;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";

struct MutablePolicy(Mutex<SandboxPolicy>);

#[async_trait::async_trait]
impl SandboxPolicySource for MutablePolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, crate::StoreError> {
        Ok(self.0.lock().expect("policy lock").clone())
    }
}

struct UnavailablePolicy;

#[async_trait::async_trait]
impl SandboxPolicySource for UnavailablePolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, crate::StoreError> {
        Err(crate::StoreError::Unavailable("test policy failure".into()))
    }
}

#[derive(Default)]
struct RacingStore {
    inner: InMemorySandboxStore,
    admitted_classes: Mutex<Vec<crate::sandbox::store::SandboxLimitClass>>,
    block_next_commit: AtomicBool,
    commit_entered: Notify,
    release_commit: Notify,
}

impl RacingStore {
    fn block_next_commit(&self) {
        self.block_next_commit.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy)]
enum RevisionCorruption {
    MissingRevision,
    AggregateProfileId,
    RevisionProfileId,
    RevisionNumber,
    RevisionDigest,
}

struct CorruptingWorkspaceProfiles {
    inner: Arc<InMemoryProfileStore>,
    corruption: RevisionCorruption,
}

#[async_trait::async_trait]
impl WorkspaceProfileStore for CorruptingWorkspaceProfiles {
    async fn create_workspace_profile(
        &self,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError> {
        self.inner.create_workspace_profile(profile).await
    }

    async fn workspace_profile(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<Option<WorkspaceProfile>, ProfileStoreError> {
        let mut stored = self.inner.workspace_profile(id).await?;
        if matches!(self.corruption, RevisionCorruption::AggregateProfileId) {
            if let Some(stored) = &mut stored {
                stored.id = WorkspaceProfileId("wkp_wrong_aggregate".into());
            }
        }
        Ok(stored)
    }

    async fn save_workspace_draft(
        &self,
        expected_draft_version: u64,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError> {
        self.inner
            .save_workspace_draft(expected_draft_version, profile)
            .await
    }

    async fn publish_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        expected_draft_version: u64,
        published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        self.inner
            .publish_workspace_profile(id, expected_draft_version, published_at)
            .await
    }

    async fn workspace_profile_revision(
        &self,
        id: &WorkspaceProfileId,
        revision: u32,
    ) -> Result<Option<WorkspaceProfileRevision>, ProfileStoreError> {
        let mut stored = self.inner.workspace_profile_revision(id, revision).await?;
        if matches!(self.corruption, RevisionCorruption::MissingRevision) {
            return Ok(None);
        }
        if let Some(stored) = &mut stored {
            match self.corruption {
                RevisionCorruption::MissingRevision => unreachable!(),
                RevisionCorruption::AggregateProfileId => {}
                RevisionCorruption::RevisionProfileId => {
                    stored.profile_id = WorkspaceProfileId("wkp_wrong".into());
                }
                RevisionCorruption::RevisionNumber => stored.revision += 1,
                RevisionCorruption::RevisionDigest => stored.digest = "wrong-digest".into(),
            }
        }
        Ok(stored)
    }

    async fn list_workspace_profile_revisions(
        &self,
        id: &WorkspaceProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfileRevision>, ProfileStoreError> {
        self.inner.list_workspace_profile_revisions(id, page).await
    }

    async fn list_workspace_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfile>, ProfileStoreError> {
        self.inner
            .list_workspace_profiles(owner, public_only, page)
            .await
    }

    async fn retire_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError> {
        self.inner
            .retire_workspace_profile(id, owner, retired_at)
            .await
    }
}

struct CorruptingSandboxProfiles {
    inner: Arc<InMemoryProfileStore>,
    corruption: RevisionCorruption,
}

#[async_trait::async_trait]
impl SandboxProfileStore for CorruptingSandboxProfiles {
    async fn sandbox_profile(
        &self,
        id: &SandboxProfileId,
    ) -> Result<Option<SandboxProfile>, ProfileStoreError> {
        let mut stored = self.inner.sandbox_profile(id).await?;
        if matches!(self.corruption, RevisionCorruption::AggregateProfileId) {
            if let Some(stored) = &mut stored {
                stored.id = SandboxProfileId("sbp_wrong_aggregate".into());
            }
        }
        Ok(stored)
    }

    async fn create_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        self.inner.create_sandbox_profile(profile).await
    }

    async fn save_sandbox_draft(
        &self,
        expected_draft_version: u64,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        self.inner
            .save_sandbox_draft(expected_draft_version, profile)
            .await
    }

    async fn publish_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        expected_draft_version: u64,
        published_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn crate::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        self.inner
            .publish_sandbox_profile(id, expected_draft_version, published_at, bounds, scanner)
            .await
    }

    async fn sandbox_profile_revision(
        &self,
        id: &SandboxProfileId,
        revision: u32,
    ) -> Result<Option<SandboxProfileRevision>, ProfileStoreError> {
        let mut stored = self.inner.sandbox_profile_revision(id, revision).await?;
        if matches!(self.corruption, RevisionCorruption::MissingRevision) {
            return Ok(None);
        }
        if let Some(stored) = &mut stored {
            match self.corruption {
                RevisionCorruption::MissingRevision => unreachable!(),
                RevisionCorruption::AggregateProfileId => {}
                RevisionCorruption::RevisionProfileId => {
                    stored.profile_id = SandboxProfileId("sbp_wrong".into());
                }
                RevisionCorruption::RevisionNumber => stored.revision += 1,
                RevisionCorruption::RevisionDigest => stored.digest = "wrong-digest".into(),
            }
        }
        Ok(stored)
    }

    async fn list_sandbox_profile_revisions(
        &self,
        id: &SandboxProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfileRevision>, ProfileStoreError> {
        self.inner.list_sandbox_profile_revisions(id, page).await
    }

    async fn list_sandbox_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfile>, ProfileStoreError> {
        self.inner
            .list_sandbox_profiles(owner, public_only, page)
            .await
    }

    async fn retire_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError> {
        self.inner
            .retire_sandbox_profile(id, owner, retired_at)
            .await
    }
}

struct FailingUseRecorder;

#[async_trait::async_trait]
impl ProfileUseRecorder for FailingUseRecorder {
    async fn record_profile_use(
        &self,
        _locator: &MarketplaceLocator,
    ) -> Result<(), ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "test recorder failure".into(),
        ))
    }
}

#[async_trait::async_trait]
impl crate::sandbox::store::SandboxStore for RacingStore {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.inner.create_sandbox(sandbox).await
    }

    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        self.inner.get_sandbox(id).await
    }

    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        self.inner.save_sandbox(expected_revision, sandbox).await
    }

    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.inner.list_sandboxes(owner, page).await
    }
}

#[async_trait::async_trait]
impl SandboxEventStore for RacingStore {
    async fn append_sandbox_event(
        &self,
        sandbox_id: &SandboxId,
        appended_at: String,
        kind: SandboxEventKind,
    ) -> Result<SandboxEvent, SandboxStoreError> {
        self.inner
            .append_sandbox_event(sandbox_id, appended_at, kind)
            .await
    }

    async fn sandbox_events(
        &self,
        sandbox_id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError> {
        self.inner.sandbox_events(sandbox_id, after, limit).await
    }
}

#[async_trait::async_trait]
impl SandboxAdmissionStore for RacingStore {
    async fn sandbox_admission_replay(
        &self,
        owner: &SubjectId,
        origin: &SandboxOrigin,
    ) -> Result<Option<crate::sandbox::store::SandboxAdmissionReplay>, SandboxStoreError> {
        self.inner.sandbox_admission_replay(owner, origin).await
    }

    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<crate::sandbox::store::SandboxAdmissionResult, SandboxStoreError> {
        self.admitted_classes
            .lock()
            .unwrap()
            .push(admission.limit_class);
        self.inner.admit_sandbox(admission).await
    }
}

#[async_trait::async_trait]
impl SandboxTransactionStore for RacingStore {
    async fn commit(&self, transaction: SandboxTransaction) -> Result<(), SandboxStoreError> {
        if self.block_next_commit.swap(false, Ordering::SeqCst) {
            self.commit_entered.notify_one();
            self.release_commit.notified().await;
        }
        self.inner.commit(transaction).await
    }
}

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 32_768,
    }
}

fn profile(
    id: &str,
    owner: &str,
    name: &str,
    visibility: ProfileVisibility,
    unsafe_content: bool,
) -> SandboxProfile {
    SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: SandboxProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: name.into(),
        description: "Sandbox service test profile".into(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 1_000,
            memory_mib: 2_048,
            nyxid_services: vec![ServiceId::new("profile-service")],
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: Vec::new(),
            software_install_commands: if unsafe_content {
                vec!["apt-get update".into()]
            } else {
                Vec::new()
            },
            environment_variables: if unsafe_content {
                vec![EnvironmentVariable {
                    name: "PROJECT_MODE".into(),
                    value: "test".into(),
                }]
            } else {
                Vec::new()
            },
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn workspace_profile(id: &str, owner: &str, setup_commands: Vec<String>) -> WorkspaceProfile {
    WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "workspace profile".into(),
        description: "Sandbox service workspace profile".into(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "project".into(),
            },
            name: "project".into(),
            setup_commands,
            agent_profiles: Vec::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

async fn publish(profiles: &InMemoryProfileStore, profile: SandboxProfile) -> SandboxProfileRef {
    let id = profile.id.clone();
    profiles.create_sandbox_profile(profile).await.unwrap();
    let revision = profiles
        .publish_sandbox_profile(&id, 1, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    SandboxProfileRef::new(id, revision.revision)
}

async fn publish_workspace(
    profiles: &InMemoryProfileStore,
    profile: WorkspaceProfile,
) -> WorkspaceProfileRef {
    let id = profile.id.clone();
    profiles.create_workspace_profile(profile).await.unwrap();
    let revision = profiles
        .publish_workspace_profile(&id, 1, NOW.into())
        .await
        .unwrap();
    WorkspaceProfileRef::new(id, revision.revision)
}

async fn sandbox_digest(profiles: &InMemoryProfileStore, reference: &SandboxProfileRef) -> String {
    profiles
        .sandbox_profile_revision(&reference.id, reference.revision)
        .await
        .unwrap()
        .unwrap()
        .digest
}

async fn sandbox_use_count(profiles: &InMemoryProfileStore, id: &SandboxProfileId) -> u64 {
    profiles
        .sandbox_profile(id)
        .await
        .unwrap()
        .unwrap()
        .uses_count
}

fn caller(subject: &str, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(SubjectId(subject.into()), Authorization::new(capabilities))
}

fn request(profile: SandboxProfileRef) -> CreateSandbox {
    CreateSandbox {
        profile,
        name: None,
        confirmed_digest: None,
        origin: SandboxOrigin::Manual,
    }
}

fn policy(owner: u16, visitor: u16) -> SandboxPolicy {
    SandboxPolicy {
        max_sandboxes_per_owner: owner,
        max_sandboxes_per_visitor: visitor,
        checkpoint_interval_seconds: crate::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
        inactivity_pause_after_seconds: DEFAULT_INACTIVITY_PAUSE_AFTER_SECONDS,
        retention_grace: std::time::Duration::from_secs(7 * 24 * 60 * 60),
        baseline_services: vec![
            ServiceId::new("baseline"),
            ServiceId::new("profile-service"),
        ],
    }
}

#[tokio::test]
async fn trust_confirmation_is_required_only_for_unsafe_foreign_profiles() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let own = publish(
        &profiles,
        profile(
            "sbp_own",
            "caller",
            "own unsafe",
            ProfileVisibility::Private,
            true,
        ),
    )
    .await;
    let public_clean = publish(
        &profiles,
        profile(
            "sbp_clean",
            "publisher",
            "clean",
            ProfileVisibility::Public,
            false,
        ),
    )
    .await;
    let public_unsafe = publish(
        &profiles,
        profile(
            "sbp_unsafe",
            "publisher",
            "unsafe",
            ProfileVisibility::Public,
            true,
        ),
    )
    .await;
    let expected_digest = profiles
        .sandbox_profile_revision(&public_unsafe.id, public_unsafe.revision)
        .await
        .unwrap()
        .unwrap()
        .digest;
    let unsafe_id = public_unsafe.id.clone();
    let service = SandboxService::new(
        store,
        profiles.clone(),
        policy(10, 10),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = caller("caller", [Capability::CreateSandbox]);

    service.create(&caller, request(own)).await.unwrap();
    service
        .create(&caller, request(public_clean))
        .await
        .unwrap();
    assert_eq!(
        service
            .create(&caller, request(public_unsafe.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::TrustConfirmationRequired {
            commands_digest: expected_digest.clone(),
        }
    );
    let mut stale = request(public_unsafe.clone());
    stale.confirmed_digest = Some("stale-digest".into());
    assert_eq!(
        service.create(&caller, stale).await.unwrap_err(),
        SandboxServiceError::TrustConfirmationRequired {
            commands_digest: expected_digest.clone(),
        }
    );
    assert_eq!(sandbox_use_count(&profiles, &unsafe_id).await, 0);
    let mut confirmed = request(public_unsafe);
    confirmed.confirmed_digest = Some(expected_digest);
    service.create(&caller, confirmed).await.unwrap();
    assert_eq!(sandbox_use_count(&profiles, &unsafe_id).await, 1);
}

#[tokio::test]
async fn foreign_nested_setup_requires_the_outer_digest_for_an_owned_safe_parent() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let mut workspace =
        workspace_profile("wkp_nested_trust", "publisher", vec!["cargo fetch".into()]);
    workspace.visibility = ProfileVisibility::Public;
    let workspace = publish_workspace(&profiles, workspace).await;
    let mut parent = profile(
        "sbp_nested_trust",
        "consumer",
        "owned safe parent",
        ProfileVisibility::Private,
        false,
    );
    parent.draft.workspace_profiles = vec![workspace.clone()];
    let parent = publish(&profiles, parent).await;
    let digest = sandbox_digest(&profiles, &parent).await;
    profiles
        .retire_workspace_profile(&workspace.id, &SubjectId::new("publisher"), LATER.into())
        .await
        .unwrap();
    let service = SandboxService::new(
        Arc::new(InMemorySandboxStore::default()),
        profiles.clone(),
        policy(10, 10),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let consumer = caller("consumer", [Capability::CreateSandbox]);

    assert_eq!(
        service
            .create(&consumer, request(parent.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::TrustConfirmationRequired {
            commands_digest: digest.clone(),
        }
    );
    let mut stale = request(parent.clone());
    stale.confirmed_digest = Some("digest-from-an-older-revision".into());
    assert_eq!(
        service.create(&consumer, stale).await.unwrap_err(),
        SandboxServiceError::TrustConfirmationRequired {
            commands_digest: digest.clone(),
        }
    );
    assert_eq!(sandbox_use_count(&profiles, &parent.id).await, 0);

    let mut confirmed = request(parent.clone());
    confirmed.confirmed_digest = Some(digest);
    service.create(&consumer, confirmed).await.unwrap();
    assert_eq!(sandbox_use_count(&profiles, &parent.id).await, 1);
}

#[tokio::test]
async fn missing_or_corrupt_nested_revisions_fail_closed_without_recording_use() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let mut workspace = workspace_profile(
        "wkp_nested_corrupt",
        "publisher",
        vec!["cargo fetch".into()],
    );
    workspace.visibility = ProfileVisibility::Public;
    let workspace = publish_workspace(&profiles, workspace).await;
    let mut parent = profile(
        "sbp_nested_corrupt",
        "consumer",
        "nested corrupt",
        ProfileVisibility::Private,
        false,
    );
    parent.draft.workspace_profiles = vec![workspace];
    let parent = publish(&profiles, parent).await;
    let digest = sandbox_digest(&profiles, &parent).await;
    let consumer = caller("consumer", [Capability::CreateSandbox]);

    for corruption in [
        RevisionCorruption::MissingRevision,
        RevisionCorruption::AggregateProfileId,
        RevisionCorruption::RevisionProfileId,
        RevisionCorruption::RevisionNumber,
        RevisionCorruption::RevisionDigest,
    ] {
        let sandbox_profiles: Arc<dyn SandboxProfileStore> = profiles.clone();
        let workspace_profiles: Arc<dyn WorkspaceProfileStore> =
            Arc::new(CorruptingWorkspaceProfiles {
                inner: profiles.clone(),
                corruption,
            });
        let profile_uses: Arc<dyn ProfileUseRecorder> = profiles.clone();
        let policy_source: Arc<dyn SandboxPolicySource> = Arc::new(policy(10, 10));
        let service = SandboxService::with_dependencies(
            Arc::new(InMemorySandboxStore::default()),
            sandbox_profiles,
            workspace_profiles,
            profile_uses,
            policy_source,
            Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        );
        let mut create = request(parent.clone());
        create.confirmed_digest = Some(digest.clone());
        assert!(matches!(
            service.create(&consumer, create).await,
            Err(SandboxServiceError::Corrupt(_))
        ));
    }
    assert_eq!(sandbox_use_count(&profiles, &parent.id).await, 0);
}

#[tokio::test]
async fn corrupt_outer_profile_aggregate_id_fails_closed_before_owner_is_trusted() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let parent = publish(
        &profiles,
        profile(
            "sbp_outer_corrupt",
            "owner",
            "outer corrupt",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let sandbox_profiles: Arc<dyn SandboxProfileStore> = Arc::new(CorruptingSandboxProfiles {
        inner: profiles.clone(),
        corruption: RevisionCorruption::AggregateProfileId,
    });
    let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles.clone();
    let profile_uses: Arc<dyn ProfileUseRecorder> = profiles.clone();
    let policy_source: Arc<dyn SandboxPolicySource> = Arc::new(policy(10, 10));
    let service = SandboxService::with_dependencies(
        Arc::new(InMemorySandboxStore::default()),
        sandbox_profiles,
        workspace_profiles,
        profile_uses,
        policy_source,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );

    assert!(matches!(
        service
            .create(
                &caller("owner", [Capability::CreateSandbox]),
                request(parent.clone()),
            )
            .await,
        Err(SandboxServiceError::Corrupt(_))
    ));
    assert_eq!(sandbox_use_count(&profiles, &parent.id).await, 0);
}

#[tokio::test]
async fn recorder_failure_does_not_fail_an_admitted_sandbox() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let parent = publish(
        &profiles,
        profile(
            "sbp_recorder_failure",
            "owner",
            "recorder failure",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let sandbox_profiles: Arc<dyn SandboxProfileStore> = profiles.clone();
    let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles.clone();
    let profile_uses: Arc<dyn ProfileUseRecorder> = Arc::new(FailingUseRecorder);
    let policy_source: Arc<dyn SandboxPolicySource> = Arc::new(policy(10, 10));
    let service = SandboxService::with_dependencies(
        Arc::new(InMemorySandboxStore::default()),
        sandbox_profiles,
        workspace_profiles,
        profile_uses,
        policy_source,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );

    let admitted = service
        .create(
            &caller("owner", [Capability::CreateSandbox]),
            request(parent.clone()),
        )
        .await
        .unwrap();
    assert_eq!(admitted.profile.id, parent.id);
    assert_eq!(sandbox_use_count(&profiles, &parent.id).await, 0);
}

#[tokio::test]
async fn private_foreign_and_retired_profiles_are_hidden() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let private = publish(
        &profiles,
        profile(
            "sbp_private",
            "publisher",
            "private",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let retired = publish(
        &profiles,
        profile(
            "sbp_retired",
            "caller",
            "retired",
            ProfileVisibility::Public,
            false,
        ),
    )
    .await;
    profiles
        .retire_sandbox_profile(&retired.id, &SubjectId::new("caller"), LATER.into())
        .await
        .unwrap();
    let service = SandboxService::new(
        store,
        profiles,
        policy(10, 10),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = caller("caller", [Capability::CreateSandbox]);

    for reference in [private, retired] {
        assert_eq!(
            service
                .create(&caller, request(reference))
                .await
                .unwrap_err(),
            SandboxServiceError::ProfileNotFound
        );
    }
}

#[tokio::test]
async fn trigger_replay_precedes_zero_quota_and_changed_security_input_conflicts() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let first_reference = publish(
        &profiles,
        profile(
            "sbp_replay_first",
            "owner",
            "replay first",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let mut second_profile = profile(
        "sbp_replay_changed",
        "owner",
        "replay changed",
        ProfileVisibility::Private,
        false,
    );
    second_profile.draft.nyxid_services = vec![ServiceId::new("changed-security-reach")];
    let second_reference = publish(&profiles, second_profile).await;
    let store = Arc::new(InMemorySandboxStore::default());
    let source = Arc::new(MutablePolicy(Mutex::new(policy(3, 1))));
    let policy_source: Arc<dyn SandboxPolicySource> = source.clone();
    let service = SandboxService::with_policy_source(
        store.clone(),
        profiles.clone(),
        policy_source,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let launcher = caller("owner", [Capability::LaunchTriggers]);
    let trigger = |profile| CreateSandbox {
        profile,
        name: None,
        confirmed_digest: None,
        origin: SandboxOrigin::Trigger {
            launch_id: "launch-replay-zero".into(),
        },
    };

    let created = service
        .create_for_launch(
            &launcher,
            Capability::LaunchTriggers,
            trigger(first_reference.clone()),
        )
        .await
        .unwrap();
    let mut advanced = created.clone();
    advanced.status = SandboxStatus::Active;
    advanced.updated_at = LATER.into();
    advanced.revision = 2;
    store.save_sandbox(1, advanced.clone()).await.unwrap();
    source
        .0
        .lock()
        .expect("policy lock")
        .max_sandboxes_per_visitor = 0;

    let mut replay = trigger(first_reference.clone());
    replay.confirmed_digest = Some("confirmation-policy-drift".into());
    assert_eq!(
        service
            .create_for_launch(&launcher, Capability::LaunchTriggers, replay)
            .await
            .unwrap(),
        advanced
    );
    let mut changed_name_mode = trigger(first_reference.clone());
    changed_name_mode.name = Some(advanced.name.clone());
    assert_eq!(
        service
            .create_for_launch(&launcher, Capability::LaunchTriggers, changed_name_mode,)
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxOriginConflict
    );
    assert_eq!(
        service
            .create_for_launch(
                &launcher,
                Capability::LaunchTriggers,
                trigger(second_reference.clone()),
            )
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxOriginConflict
    );
    let mut changed_revision = first_reference.clone();
    changed_revision.revision += 1;
    assert_eq!(
        service
            .create_for_launch(
                &launcher,
                Capability::LaunchTriggers,
                trigger(changed_revision),
            )
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxOriginConflict
    );
    assert_eq!(sandbox_use_count(&profiles, &first_reference.id).await, 1);
    assert_eq!(sandbox_use_count(&profiles, &second_reference.id).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_trigger_replay_returns_one_sandbox_and_records_one_use() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_concurrent_replay",
            "owner",
            "concurrent replay",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let profile_id = reference.id.clone();
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profiles.clone(),
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let launcher = caller("owner", [Capability::LaunchTriggers]);
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let service = service.clone();
        let launcher = launcher.clone();
        let mut request = request(reference.clone());
        request.origin = SandboxOrigin::Trigger {
            launch_id: "launch-concurrent-replay".into(),
        };
        tasks.push(tokio::spawn(async move {
            service
                .create_for_launch(&launcher, Capability::LaunchTriggers, request)
                .await
        }));
    }

    let mut sandboxes = Vec::new();
    for task in tasks {
        sandboxes.push(task.await.unwrap().unwrap());
    }
    assert!(sandboxes
        .iter()
        .all(|sandbox| sandbox.id == sandboxes[0].id));
    assert_eq!(sandbox_use_count(&profiles, &profile_id).await, 1);
    assert_eq!(
        store
            .list_sandboxes(&SubjectId::new("owner"), &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .sandbox_events(&sandboxes[0].id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn frozen_launch_profiles_survive_outer_and_nested_retirement_with_exact_trust() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let mut nested_profile = workspace_profile(
        "wkp_launch_retired",
        "nested-publisher",
        vec!["cargo fetch".into()],
    );
    nested_profile.visibility = ProfileVisibility::Public;
    let nested = publish_workspace(&profiles, nested_profile).await;
    let mut outer = profile(
        "sbp_launch_retired",
        "outer-publisher",
        "retired launch",
        ProfileVisibility::Private,
        true,
    );
    outer.draft.workspace_profiles = vec![nested.clone()];
    let outer = publish(&profiles, outer).await;
    let digest = sandbox_digest(&profiles, &outer).await;
    profiles
        .retire_workspace_profile(
            &nested.id,
            &SubjectId::new("nested-publisher"),
            LATER.into(),
        )
        .await
        .unwrap();
    profiles
        .retire_sandbox_profile(&outer.id, &SubjectId::new("outer-publisher"), LATER.into())
        .await
        .unwrap();
    let service = SandboxService::new(
        Arc::new(InMemorySandboxStore::default()),
        profiles.clone(),
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let launcher = caller("consumer", [Capability::LaunchTriggers]);
    let launch = |confirmed_digest| CreateSandbox {
        profile: outer.clone(),
        name: None,
        confirmed_digest,
        origin: SandboxOrigin::Trigger {
            launch_id: "launch-retired-profiles".into(),
        },
    };

    assert_eq!(
        service
            .create_for_launch(
                &launcher,
                Capability::LaunchTriggers,
                launch(Some("stale-digest".into())),
            )
            .await
            .unwrap_err(),
        SandboxServiceError::TrustConfirmationRequired {
            commands_digest: digest.clone()
        }
    );
    let created = service
        .create_for_launch(
            &launcher,
            Capability::LaunchTriggers,
            launch(Some(digest.clone())),
        )
        .await
        .unwrap();
    assert_eq!(created.profile.id, outer.id);
    assert_eq!(created.profile.revision, outer.revision);
    assert_eq!(created.profile.digest, digest);
    assert_eq!(sandbox_use_count(&profiles, &outer.id).await, 1);
}

#[tokio::test]
async fn launch_exact_outer_revision_corruption_fails_closed() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_launch_corrupt",
            "owner",
            "launch corrupt",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let launcher = caller("owner", [Capability::CreateSandbox]);

    for corruption in [
        RevisionCorruption::MissingRevision,
        RevisionCorruption::AggregateProfileId,
        RevisionCorruption::RevisionProfileId,
        RevisionCorruption::RevisionNumber,
        RevisionCorruption::RevisionDigest,
    ] {
        let sandbox_profiles: Arc<dyn SandboxProfileStore> = Arc::new(CorruptingSandboxProfiles {
            inner: profiles.clone(),
            corruption,
        });
        let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles.clone();
        let profile_uses: Arc<dyn ProfileUseRecorder> = profiles.clone();
        let policy_source: Arc<dyn SandboxPolicySource> = Arc::new(policy(3, 1));
        let service = SandboxService::with_dependencies(
            Arc::new(InMemorySandboxStore::default()),
            sandbox_profiles,
            workspace_profiles,
            profile_uses,
            policy_source,
            Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        );
        let mut create = request(reference.clone());
        create.origin = SandboxOrigin::Trigger {
            launch_id: format!("launch-corrupt-{corruption:?}"),
        };
        let error = service
            .create_for_launch(&launcher, Capability::CreateSandbox, create)
            .await
            .unwrap_err();
        if matches!(corruption, RevisionCorruption::MissingRevision) {
            assert_eq!(error, SandboxServiceError::ProfileNotFound);
        } else {
            assert!(matches!(error, SandboxServiceError::Corrupt(_)));
        }
    }
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 0);
}

#[tokio::test]
async fn launch_replay_precedes_failed_policy_retired_profile_and_sign_in_completion() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let mut own_credentials = profile(
        "sbp_launch_early_replay",
        "owner",
        "early replay",
        ProfileVisibility::Private,
        false,
    );
    own_credentials.draft.model_access = ModelAccess::OwnCredentials;
    let reference = publish(&profiles, own_credentials).await;
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profiles.clone(),
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let launcher = caller("owner", [Capability::LaunchTriggers]);
    let launch = || CreateSandbox {
        profile: reference.clone(),
        name: None,
        confirmed_digest: None,
        origin: SandboxOrigin::Trigger {
            launch_id: "launch-early-replay".into(),
        },
    };
    let created = service
        .create_for_launch(&launcher, Capability::LaunchTriggers, launch())
        .await
        .unwrap();
    assert_eq!(created.credential.sign_in, SandboxSignInState::Required);
    let mut advanced = created;
    advanced.credential.sign_in = SandboxSignInState::Complete;
    advanced.updated_at = LATER.into();
    advanced.revision = 2;
    store.save_sandbox(1, advanced.clone()).await.unwrap();
    profiles
        .retire_sandbox_profile(&reference.id, &SubjectId::new("owner"), LATER.into())
        .await
        .unwrap();

    let sandbox_profiles: Arc<dyn SandboxProfileStore> = Arc::new(CorruptingSandboxProfiles {
        inner: profiles.clone(),
        corruption: RevisionCorruption::AggregateProfileId,
    });
    let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles.clone();
    let profile_uses: Arc<dyn ProfileUseRecorder> = profiles.clone();
    let policy_source: Arc<dyn SandboxPolicySource> = Arc::new(UnavailablePolicy);
    let replay_service = SandboxService::with_dependencies(
        store,
        sandbox_profiles,
        workspace_profiles,
        profile_uses,
        policy_source,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let mut replay = launch();
    replay.confirmed_digest = Some("confirmation-no-longer-current".into());
    assert_eq!(
        replay_service
            .create_for_launch(&launcher, Capability::LaunchTriggers, replay)
            .await
            .unwrap(),
        advanced
    );
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 1);
}

#[tokio::test]
async fn create_reads_current_policy_for_each_admission() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_live_policy",
            "owner",
            "live policy",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let source = Arc::new(MutablePolicy(Mutex::new(SandboxPolicy {
        max_sandboxes_per_owner: 1,
        max_sandboxes_per_visitor: 1,
        checkpoint_interval_seconds: crate::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
        inactivity_pause_after_seconds: DEFAULT_INACTIVITY_PAUSE_AFTER_SECONDS,
        retention_grace: DEFAULT_RETENTION_GRACE,
        baseline_services: vec![ServiceId::new("baseline-one")],
    })));
    let policy_source: Arc<dyn SandboxPolicySource> = source.clone();
    let service = SandboxService::with_policy_source(
        store,
        profiles,
        policy_source,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);

    let first = service
        .create(&owner, request(reference.clone()))
        .await
        .unwrap();
    assert!(first
        .credential
        .reaches
        .contains(&ServiceId::new("baseline-one")));
    assert_eq!(
        service
            .create(&owner, request(reference.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxLimitReached { limit: 1, live: 1 }
    );

    *source.0.lock().expect("policy lock") = SandboxPolicy {
        max_sandboxes_per_owner: 2,
        max_sandboxes_per_visitor: 1,
        checkpoint_interval_seconds: crate::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
        inactivity_pause_after_seconds: DEFAULT_INACTIVITY_PAUSE_AFTER_SECONDS,
        retention_grace: DEFAULT_RETENTION_GRACE,
        baseline_services: vec![ServiceId::new("baseline-two")],
    };
    let second = service.create(&owner, request(reference)).await.unwrap();
    assert!(second
        .credential
        .reaches
        .contains(&ServiceId::new("baseline-two")));
    assert!(!second
        .credential
        .reaches
        .contains(&ServiceId::new("baseline-one")));
}

#[tokio::test]
async fn default_names_deduplicate_and_preserve_utf8_byte_bounds() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_names",
            "owner",
            "research",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let unicode = publish(
        &profiles,
        profile(
            "sbp_unicode",
            "owner",
            &"é".repeat(32),
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let service = SandboxService::new(
        store,
        profiles,
        policy(10, 10),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);

    let mut names = Vec::new();
    for _ in 0..3 {
        names.push(
            service
                .create(&owner, request(reference.clone()))
                .await
                .unwrap()
                .name,
        );
    }
    assert_eq!(names, ["research", "research-2", "research-3"]);

    let first = service
        .create(&owner, request(unicode.clone()))
        .await
        .unwrap();
    let second = service.create(&owner, request(unicode)).await.unwrap();
    assert_eq!(first.name.len(), 64);
    assert!(second.name.len() <= 64);
    assert!(second.name.ends_with("-2"));
    assert!(second.name.is_char_boundary(second.name.len()));
}

#[tokio::test]
async fn an_explicit_live_name_is_not_silently_renamed() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_explicit",
            "owner",
            "profile",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let profile_id = reference.id.clone();
    let service = SandboxService::new(
        store,
        profiles.clone(),
        policy(10, 10),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    let mut invalid = request(reference.clone());
    invalid.name = Some("invalid\0name".into());
    assert_eq!(
        service.create(&owner, invalid).await.unwrap_err(),
        SandboxServiceError::InvalidName
    );
    assert_eq!(sandbox_use_count(&profiles, &profile_id).await, 0);

    let mut create = request(reference);
    create.name = Some("chosen".into());
    assert_eq!(
        service.create(&owner, create.clone()).await.unwrap().name,
        "chosen"
    );
    assert_eq!(sandbox_use_count(&profiles, &profile_id).await, 1);
    assert_eq!(
        service.create(&owner, create).await.unwrap_err(),
        SandboxServiceError::NameConflict
    );
    assert_eq!(sandbox_use_count(&profiles, &profile_id).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_admission_never_exceeds_the_live_limit() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_concurrent",
            "owner",
            "parallel",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let profile_id = reference.id.clone();
    let service = SandboxService::new(
        store.clone(),
        profiles.clone(),
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let service = service.clone();
        let owner = owner.clone();
        let reference = reference.clone();
        tasks.push(tokio::spawn(async move {
            service.create(&owner, request(reference)).await
        }));
    }
    let mut admitted = Vec::new();
    let mut refused = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(sandbox) => admitted.push(sandbox),
            Err(SandboxServiceError::SandboxLimitReached { limit: 3, live: 3 }) => refused += 1,
            result => panic!("unexpected admission result: {result:?}"),
        }
    }
    assert_eq!(admitted.len(), 3);
    assert_eq!(refused, 17);
    assert_eq!(sandbox_use_count(&profiles, &profile_id).await, 3);
    admitted.sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(
        admitted
            .iter()
            .map(|sandbox| sandbox.name.as_str())
            .collect::<Vec<_>>(),
        ["parallel", "parallel-2", "parallel-3"]
    );
    assert_eq!(
        store
            .list_sandboxes(&SubjectId::new("owner"), &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        3
    );
}

#[tokio::test]
async fn admission_atomically_starts_with_created_and_snapshots_the_revision() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_snapshot",
            "owner",
            "snapshot",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let service = SandboxService::new(
        store.clone(),
        profiles,
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    let sandbox = service.create(&owner, request(reference)).await.unwrap();

    assert_eq!(sandbox.status, SandboxStatus::Provisioning);
    assert_eq!(sandbox.work.step, SandboxWorkStep::ProvisionRuntime);
    assert_eq!(sandbox.revision, 1);
    assert_eq!(
        sandbox
            .credential
            .reaches
            .iter()
            .map(ServiceId::as_str)
            .collect::<Vec<_>>(),
        ["baseline", "chrono-llm-public", "profile-service"]
    );
    let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[0].kind, SandboxEventKind::Created);
}

#[tokio::test]
async fn own_account_profiles_store_only_the_required_sign_in_state() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let mut own_account = profile(
        "sbp_own_account",
        "owner",
        "own account",
        ProfileVisibility::Private,
        false,
    );
    own_account.draft.model_access = ModelAccess::OwnCredentials;
    own_account
        .draft
        .nyxid_services
        .push(ServiceId::new("chrono-llm"));
    let reference = publish(&profiles, own_account).await;
    let mut subscription_policy = policy(3, 1);
    subscription_policy
        .baseline_services
        .push(ServiceId::new("chrono-llm-public"));
    let service = SandboxService::new(
        store,
        profiles,
        subscription_policy,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );

    let sandbox = service
        .create(
            &caller("owner", [Capability::CreateSandbox]),
            request(reference),
        )
        .await
        .unwrap();

    assert_eq!(sandbox.credential.sign_in, SandboxSignInState::Required);
    assert_eq!(sandbox.credential.key_generation, 0);
    assert!(sandbox.credential.current.is_none());
    assert!(sandbox.credential.pending.is_none());
    assert!(sandbox.credential.revoke_debt.is_empty());
    assert_eq!(
        sandbox.credential.reaches,
        ["baseline", "profile-service"].map(ServiceId::new)
    );
}

#[tokio::test]
async fn reads_require_capability_and_hide_another_owner() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_reads",
            "owner",
            "reads",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let service = SandboxService::new(
        store,
        profiles,
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let creator = caller("owner", [Capability::CreateSandbox]);
    let sandbox = service.create(&creator, request(reference)).await.unwrap();
    let owner = caller("owner", [Capability::UseOwnSandboxes]);
    let stranger = caller("stranger", [Capability::UseOwnSandboxes]);
    let unprivileged = caller("owner", []);

    assert_eq!(service.read(&owner, &sandbox.id).await.unwrap(), sandbox);
    assert_eq!(
        service
            .list_own(&owner, &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        service.events(&owner, &sandbox.id, 0, 10).await.unwrap()[0].kind,
        SandboxEventKind::Created
    );
    assert_eq!(
        service.read(&stranger, &sandbox.id).await.unwrap_err(),
        SandboxServiceError::NotFound
    );
    assert_eq!(
        service.read(&unprivileged, &sandbox.id).await.unwrap_err(),
        SandboxServiceError::Forbidden
    );
}

#[tokio::test]
async fn delete_is_idempotent_and_schedules_cleanup_from_every_live_status() {
    let statuses = [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
    ];
    for (index, status) in statuses.into_iter().enumerate() {
        let profiles = Arc::new(InMemoryProfileStore::default());
        let store = Arc::new(InMemorySandboxStore::default());
        let reference = publish(
            &profiles,
            profile(
                &format!("sbp_delete_{index}"),
                "owner",
                "delete",
                ProfileVisibility::Private,
                false,
            ),
        )
        .await;
        let service = SandboxService::new(
            store.clone(),
            profiles,
            policy(3, 1),
            Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        );
        let creator = caller("owner", [Capability::CreateSandbox]);
        let mut sandbox = service.create(&creator, request(reference)).await.unwrap();
        if status != SandboxStatus::Provisioning {
            let expected = sandbox.revision;
            sandbox.status = status;
            sandbox.failure = (status == SandboxStatus::Failed).then(|| TypedFailure {
                class: "terminal_validation".into(),
                detail: "fixture".into(),
            });
            sandbox.revision += 1;
            store.save_sandbox(expected, sandbox.clone()).await.unwrap();
        }
        let deleter = caller("owner", [Capability::DeleteOwnSandbox]);
        let retiring = service.delete(&deleter, &sandbox.id).await.unwrap();
        assert_eq!(retiring.status, SandboxStatus::Retiring, "from {status:?}");
        assert_eq!(retiring.work.step, SandboxWorkStep::StopAgents);
        assert!(retiring.work.lease.is_none());
        assert_eq!(
            service.delete(&deleter, &sandbox.id).await.unwrap(),
            retiring
        );
    }
}

#[tokio::test]
async fn concurrent_delete_returns_the_winning_retirement_state() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(RacingStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_concurrent_delete",
            "owner",
            "delete race",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let service = SandboxService::new(
        store.clone(),
        profiles,
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let sandbox = service
        .create(
            &caller("owner", [Capability::CreateSandbox]),
            request(reference),
        )
        .await
        .unwrap();
    let deleter = caller("owner", [Capability::DeleteOwnSandbox]);
    store.block_next_commit();
    let entered = store.commit_entered.notified();
    let first_service = service.clone();
    let first_deleter = deleter.clone();
    let id = sandbox.id.clone();
    let first = tokio::spawn(async move { first_service.delete(&first_deleter, &id).await });
    entered.await;

    let winner = service.delete(&deleter, &sandbox.id).await.unwrap();
    store.release_commit.notify_one();
    let raced = first.await.unwrap().unwrap();

    assert_eq!(winner.status, SandboxStatus::Retiring);
    assert_eq!(raced, winner);
}

#[tokio::test]
async fn lease_takeover_fences_a_stale_worker_commit() {
    const CLAIM_AT: &str = "2099-01-01T00:00:00Z";
    const TAKEOVER_AT: &str = "2099-01-01T00:01:00Z";
    const TAKEOVER_END: &str = "2099-01-01T00:02:00Z";

    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_fence",
            "owner",
            "fence",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let service = SandboxService::new(
        store.clone(),
        profiles,
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    let sandbox = service.create(&owner, request(reference)).await.unwrap();

    let mut first = store
        .claim_sandbox_work(&sandbox.id, "worker-a", CLAIM_AT.into(), TAKEOVER_AT.into())
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_sandbox_work(&sandbox.id, "worker-b", CLAIM_AT.into(), TAKEOVER_AT.into(),)
        .await
        .unwrap()
        .is_none());
    let second = store
        .claim_sandbox_work(
            &sandbox.id,
            "worker-b",
            TAKEOVER_AT.into(),
            TAKEOVER_END.into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(second.work.lease.as_ref().unwrap().fence > first.work.lease.as_ref().unwrap().fence);

    let stale_revision = first.revision;
    let stale_fence = first.work.lease.as_ref().unwrap().fence;
    first
        .work
        .advance(
            "worker-a",
            stale_fence,
            SandboxWorkStep::RunInstall,
            LATER.into(),
        )
        .unwrap();
    first.revision += 1;
    assert_eq!(
        store
            .commit(
                crate::sandbox::store::SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: None,
                    expected_revision: stale_revision,
                    sandbox: Box::new(first),
                    event_at: LATER.into(),
                    event: SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::ProvisionRuntime,
                    },
                }
            )
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(second));
}

#[test]
fn service_error_codes_are_stable_typed_refusals() {
    let cases = [
        (SandboxServiceError::Forbidden, "forbidden"),
        (
            SandboxServiceError::TrustConfirmationRequired {
                commands_digest: "digest".into(),
            },
            "trust_confirmation_required",
        ),
        (
            SandboxServiceError::SandboxLimitReached { limit: 3, live: 3 },
            "sandbox_limit_reached",
        ),
        (SandboxServiceError::NameConflict, "sandbox_name_conflict"),
        (
            SandboxServiceError::SandboxOriginConflict,
            "sandbox_origin_conflict",
        ),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code);
    }
}
