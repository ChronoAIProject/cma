use std::sync::{Arc, Mutex};

use crate::profiles::store::{
    InMemoryProfileStore, ProfilePage, ProfilePageRequest, ProfileStoreError, ProfileUseRecorder,
    WorkspaceProfileStore,
};
use crate::profiles::{
    content_digest, AgentProvider, MarketplaceLocator, ModelAccess, ProfileVisibility,
    SandboxProfileContent, SandboxProfileSnapshot, WorkspaceLocation, WorkspaceProfile,
    WorkspaceProfileContent, WorkspaceProfileRef, WorkspaceProfileRevision,
};
use crate::sandbox::store::{SandboxStore, WorkspaceStore};
use crate::{
    Authorization, Caller, Capability, SandboxId, SandboxProfileId, SubjectId, WorkspaceProfileId,
};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

#[path = "workspace_lifecycle_tests.rs"]
mod lifecycle;
#[path = "workspace_prepare_recovery_tests.rs"]
mod prepare_recovery;

#[derive(Clone, Copy, Debug)]
enum ProfileCorruption {
    AggregateId,
    RevisionProfileId,
    RevisionNumber,
    RevisionDigest,
    RevisionContent,
}

struct CorruptingWorkspaceProfiles {
    inner: Arc<InMemoryProfileStore>,
    corruption: ProfileCorruption,
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
        let mut profile = self.inner.workspace_profile(id).await?;
        if matches!(self.corruption, ProfileCorruption::AggregateId) {
            if let Some(profile) = &mut profile {
                profile.id = WorkspaceProfileId("wkp_wrong_aggregate".into());
            }
        }
        Ok(profile)
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
        if let Some(stored) = &mut stored {
            match self.corruption {
                ProfileCorruption::AggregateId => {}
                ProfileCorruption::RevisionProfileId => {
                    stored.profile_id = WorkspaceProfileId("wkp_wrong_revision".into());
                }
                ProfileCorruption::RevisionNumber => stored.revision += 1,
                ProfileCorruption::RevisionDigest => stored.digest = "wrong-digest".into(),
                ProfileCorruption::RevisionContent => {
                    stored.content.location = WorkspaceLocation::Directory {
                        name: "../invalid".into(),
                    };
                    stored.content.name = "../invalid".into();
                    stored.digest = content_digest(&stored.content).unwrap();
                }
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

struct RecordingUseRecorder {
    calls: Mutex<Vec<MarketplaceLocator>>,
    fail: bool,
}

impl RecordingUseRecorder {
    fn failing() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail: true,
        }
    }

    fn calls(&self) -> Vec<MarketplaceLocator> {
        self.calls.lock().expect("recorder lock").clone()
    }
}

#[async_trait::async_trait]
impl ProfileUseRecorder for RecordingUseRecorder {
    async fn record_profile_use(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<(), ProfileStoreError> {
        self.calls
            .lock()
            .expect("recorder lock")
            .push(locator.clone());
        if self.fail {
            Err(ProfileStoreError::Unavailable(
                "test recorder failure".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn launch_caller(subject: &str, authority: Capability) -> Caller {
    Caller::new(SubjectId(subject.into()), Authorization::new([authority]))
}

fn profile(
    id: &str,
    owner: &str,
    name: &str,
    visibility: ProfileVisibility,
    setup_commands: Vec<String>,
) -> WorkspaceProfile {
    WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: format!("{name} profile"),
        description: "Workspace service test profile".into(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory { name: name.into() },
            name: name.into(),
            setup_commands,
            agent_profiles: Vec::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

async fn publish(
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

async fn digest(profiles: &InMemoryProfileStore, reference: &WorkspaceProfileRef) -> String {
    profiles
        .workspace_profile_revision(&reference.id, reference.revision)
        .await
        .unwrap()
        .unwrap()
        .digest
}

async fn use_count(profiles: &InMemoryProfileStore, id: &WorkspaceProfileId) -> u64 {
    profiles
        .workspace_profile(id)
        .await
        .unwrap()
        .unwrap()
        .uses_count
}

fn sandbox(
    id: &str,
    status: SandboxStatus,
    workspace_profiles: Vec<WorkspaceProfileRef>,
    max_workspaces: u8,
) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId("consumer".into()),
        name: id.into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_workspace_service".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: Vec::new(),
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces,
                max_agents_per_workspace: 4,
                workspace_profiles,
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
        },
        status,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: Vec::new(),
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into()),
        limits: SandboxLimits {
            max_workspaces,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn request(profile: WorkspaceProfileRef, confirmed_digest: Option<String>) -> CreateWorkspace {
    CreateWorkspace {
        profile,
        confirmed_digest,
    }
}

#[tokio::test]
async fn manual_admission_requires_the_current_digest_and_records_use_once() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_manual_trust",
            "publisher",
            "manual-project",
            ProfileVisibility::Public,
            vec!["cargo fetch".into()],
        ),
    )
    .await;
    let expected_digest = digest(&profiles, &reference).await;
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_manual_trust", SandboxStatus::Active, Vec::new(), 4);
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles.clone());
    let consumer = caller("consumer");

    for confirmed_digest in [None, Some("stale-digest".into())] {
        assert_eq!(
            service
                .create(
                    &consumer,
                    &parent.id,
                    request(reference.clone(), confirmed_digest),
                )
                .await
                .unwrap_err(),
            WorkspaceServiceError::TrustConfirmationRequired {
                digest: expected_digest.clone(),
            }
        );
    }
    assert_eq!(use_count(&profiles, &reference.id).await, 0);

    let workspace = service
        .create(
            &consumer,
            &parent.id,
            request(reference.clone(), Some(expected_digest)),
        )
        .await
        .unwrap();
    assert_eq!(workspace.profile.id, reference.id);
    assert_eq!(use_count(&profiles, &reference.id).await, 1);

    assert_eq!(
        service
            .create(&consumer, &parent.id, request(reference.clone(), None))
            .await
            .unwrap_err(),
        WorkspaceServiceError::TrustConfirmationRequired {
            digest: workspace.profile.digest,
        }
    );
    assert_eq!(use_count(&profiles, &reference.id).await, 1);
}

#[tokio::test]
async fn inline_admission_does_not_read_or_record_a_workspace_profile() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_inline_workspace", SandboxStatus::Active, Vec::new(), 4);
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store, profiles.clone());

    let workspace = service
        .create_inline(
            &caller("consumer"),
            &parent.id,
            CreateInlineWorkspace {
                source: InlineWorkspaceSource::GitRepository {
                    url: "https://github.com/ChronoAIProject/cma".into(),
                    reference: Some("develop".into()),
                },
                name: None,
            },
        )
        .await
        .unwrap();

    assert!(workspace.profile.id.as_str().starts_with("wkp_"));
    assert_eq!(workspace.profile.revision, 1);
    assert_eq!(workspace.name, "cma");
    assert_eq!(
        workspace.source,
        WorkspaceSource::GitRepository {
            url: "https://github.com/ChronoAIProject/cma".into(),
            reference: Some("develop".into()),
        }
    );
    assert_eq!(
        profiles
            .list_workspace_profiles(None, false, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        0
    );
}

#[tokio::test]
async fn paused_manual_creation_checks_trust_before_durable_revival() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_paused_create",
            "publisher",
            "paused-project",
            ProfileVisibility::Public,
            vec!["mkdir -p local".into()],
        ),
    )
    .await;
    let expected_digest = digest(&profiles, &reference).await;
    let store = Arc::new(InMemorySandboxStore::default());
    let mut parent = sandbox("sbx_paused_create", SandboxStatus::Paused, Vec::new(), 4);
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    parent.lifecycle.checkpoints.latest = Some(super::lifecycle_test_support::checkpoint(1, NOW));
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles);
    assert!(matches!(
        service
            .create(
                &caller("consumer"),
                &parent.id,
                request(reference.clone(), None)
            )
            .await,
        Err(WorkspaceServiceError::TrustConfirmationRequired { .. })
    ));
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    assert_eq!(
        service
            .create(
                &caller("other-owner"),
                &parent.id,
                request(reference.clone(), Some(expected_digest.clone()))
            )
            .await
            .unwrap_err(),
        WorkspaceServiceError::NotFound
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    let workspace = service
        .create(
            &caller("consumer"),
            &parent.id,
            request(reference, Some(expected_digest)),
        )
        .await
        .unwrap();
    assert_eq!(workspace.status, WorkspaceStatus::Preparing);
    let reviving = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::WorkspaceCreation,
            ..
        })
    ));
}

#[tokio::test]
async fn launch_admission_replays_by_origin_and_rejects_changed_input() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_launch_origin",
            "consumer",
            "launch-origin",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let changed = publish(
        &profiles,
        profile(
            "wkp_launch_origin_changed",
            "consumer",
            "changed-origin",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_launch_origin", SandboxStatus::Active, Vec::new(), 4);
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles.clone());
    let creator = launch_caller("consumer", Capability::CreateSandbox);

    assert_eq!(
        service
            .create_for_launch(
                &caller("consumer"),
                Capability::LaunchTriggers,
                &parent.id,
                request(reference.clone(), None),
                "lch_unauthorized".into(),
            )
            .await,
        Err(WorkspaceServiceError::Forbidden)
    );
    assert_eq!(
        service
            .create_for_launch(
                &caller("consumer"),
                Capability::UseOwnSandboxes,
                &parent.id,
                request(reference.clone(), None),
                "lch_invalid_lane".into(),
            )
            .await,
        Err(WorkspaceServiceError::Forbidden)
    );
    assert_eq!(
        service
            .create_for_launch(
                &launch_caller("other", Capability::LaunchTriggers),
                Capability::LaunchTriggers,
                &parent.id,
                request(reference.clone(), None),
                "lch_wrong_owner".into(),
            )
            .await,
        Err(WorkspaceServiceError::NotFound)
    );

    let first = service
        .create_for_launch(
            &creator,
            Capability::CreateSandbox,
            &parent.id,
            request(reference.clone(), None),
            "lch_stable_origin".into(),
        )
        .await
        .unwrap();
    let profile_store: Arc<dyn WorkspaceProfileStore> = Arc::new(CorruptingWorkspaceProfiles {
        inner: profiles.clone(),
        corruption: ProfileCorruption::AggregateId,
    });
    let profile_uses: Arc<dyn ProfileUseRecorder> = profiles.clone();
    let replay_service = WorkspaceService::with_dependencies(store, profile_store, profile_uses);
    let replay = replay_service
        .create_for_launch(
            &creator,
            Capability::CreateSandbox,
            &parent.id,
            request(reference.clone(), None),
            "lch_stable_origin".into(),
        )
        .await
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        first.origin,
        WorkspaceOrigin::Trigger {
            launch_id: "lch_stable_origin".into()
        }
    );
    assert_eq!(use_count(&profiles, &reference.id).await, 1);

    assert_eq!(
        service
            .create_for_launch(
                &creator,
                Capability::CreateSandbox,
                &parent.id,
                request(changed, None),
                "lch_stable_origin".into(),
            )
            .await,
        Err(WorkspaceServiceError::OriginConflict)
    );
    assert_eq!(
        service
            .create_for_launch(
                &creator,
                Capability::CreateSandbox,
                &parent.id,
                request(reference, None),
                "invalid\norigin".into(),
            )
            .await,
        Err(WorkspaceServiceError::InvalidOrigin)
    );
}

#[tokio::test]
async fn frozen_launch_revision_survives_profile_retirement_with_exact_trust() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_frozen_launch",
            "publisher",
            "frozen-launch",
            ProfileVisibility::Public,
            vec!["cargo fetch".into()],
        ),
    )
    .await;
    let expected_digest = digest(&profiles, &reference).await;
    profiles
        .retire_workspace_profile(&reference.id, &SubjectId::new("publisher"), NOW.into())
        .await
        .unwrap();
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_frozen_launch", SandboxStatus::Active, Vec::new(), 4);
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store, profiles.clone());

    assert_eq!(
        service
            .create(
                &caller("consumer"),
                &parent.id,
                request(reference.clone(), Some(expected_digest.clone())),
            )
            .await,
        Err(WorkspaceServiceError::ProfileNotFound)
    );
    let visitor = launch_caller("consumer", Capability::LaunchTriggers);
    assert_eq!(
        service
            .create_for_launch(
                &visitor,
                Capability::LaunchTriggers,
                &parent.id,
                request(reference.clone(), Some("wrong".into())),
                "lch_frozen".into(),
            )
            .await,
        Err(WorkspaceServiceError::TrustConfirmationRequired {
            digest: expected_digest.clone()
        })
    );
    let created = service
        .create_for_launch(
            &visitor,
            Capability::LaunchTriggers,
            &parent.id,
            request(reference.clone(), Some(expected_digest.clone())),
            "lch_frozen".into(),
        )
        .await
        .unwrap();
    let replay = service
        .create_for_launch(
            &visitor,
            Capability::LaunchTriggers,
            &parent.id,
            request(reference.clone(), Some(expected_digest)),
            "lch_frozen".into(),
        )
        .await
        .unwrap();
    assert_eq!(replay, created);
    assert_eq!(use_count(&profiles, &reference.id).await, 1);
}

#[tokio::test]
async fn automatic_admission_never_prompts_and_concurrent_replays_record_once() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_automatic",
            "publisher",
            "automatic-project",
            ProfileVisibility::Private,
            vec!["cargo fetch".into()],
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox(
        "sbx_automatic",
        SandboxStatus::Installing,
        vec![reference.clone()],
        4,
    );
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles.clone());

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let service = service.clone();
        let parent = parent.clone();
        let reference = reference.clone();
        tasks.push(tokio::spawn(async move {
            service
                .create_profile_workspace(&parent, &reference, 0)
                .await
                .unwrap()
        }));
    }
    let mut admitted = Vec::new();
    for task in tasks {
        admitted.push(task.await.unwrap());
    }
    assert!(admitted
        .iter()
        .all(|workspace| workspace.id == admitted[0].id));
    assert_eq!(use_count(&profiles, &reference.id).await, 1);
    assert_eq!(
        store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    let replayed = service
        .create_profile_workspace(&parent, &reference, 0)
        .await
        .unwrap();
    assert_eq!(replayed.id, admitted[0].id);
    assert_eq!(use_count(&profiles, &reference.id).await, 1);
}

#[tokio::test]
async fn corrupt_aggregate_revision_or_content_fails_before_admission_or_use() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_corrupt",
            "consumer",
            "corrupt-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;

    for (index, corruption) in [
        ProfileCorruption::AggregateId,
        ProfileCorruption::RevisionProfileId,
        ProfileCorruption::RevisionNumber,
        ProfileCorruption::RevisionDigest,
        ProfileCorruption::RevisionContent,
    ]
    .into_iter()
    .enumerate()
    {
        let store = Arc::new(InMemorySandboxStore::default());
        let parent = sandbox(
            &format!("sbx_corrupt_{index}"),
            SandboxStatus::Active,
            Vec::new(),
            4,
        );
        store.create_sandbox(parent.clone()).await.unwrap();
        let profile_store: Arc<dyn WorkspaceProfileStore> = Arc::new(CorruptingWorkspaceProfiles {
            inner: profiles.clone(),
            corruption,
        });
        let recorder = Arc::new(RecordingUseRecorder {
            calls: Mutex::new(Vec::new()),
            fail: false,
        });
        let profile_uses: Arc<dyn ProfileUseRecorder> = recorder.clone();
        let service =
            WorkspaceService::with_dependencies(store.clone(), profile_store, profile_uses);

        assert!(matches!(
            service
                .create(
                    &caller("consumer"),
                    &parent.id,
                    request(reference.clone(), None),
                )
                .await,
            Err(WorkspaceServiceError::Corrupt(_))
        ));
        assert!(matches!(
            service
                .create_for_launch(
                    &launch_caller("consumer", Capability::LaunchTriggers),
                    Capability::LaunchTriggers,
                    &parent.id,
                    request(reference.clone(), None),
                    format!("lch_corrupt_{index}"),
                )
                .await,
            Err(WorkspaceServiceError::Corrupt(_))
        ));
        assert!(recorder.calls().is_empty(), "corruption: {corruption:?}");
        assert!(store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn recorder_failure_is_best_effort_after_successful_admission() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_recorder_failure",
            "consumer",
            "recorder-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_recorder_failure", SandboxStatus::Active, Vec::new(), 4);
    store.create_sandbox(parent.clone()).await.unwrap();
    let profile_store: Arc<dyn WorkspaceProfileStore> = profiles.clone();
    let recorder = Arc::new(RecordingUseRecorder::failing());
    let profile_uses: Arc<dyn ProfileUseRecorder> = recorder.clone();
    let service = WorkspaceService::with_dependencies(store, profile_store, profile_uses);

    let admitted = service
        .create(
            &caller("consumer"),
            &parent.id,
            request(reference.clone(), None),
        )
        .await
        .unwrap();
    assert_eq!(admitted.profile.id, reference.id);
    assert_eq!(
        recorder.calls(),
        vec![MarketplaceLocator::Workspace(reference.id.clone())]
    );
    assert_eq!(use_count(&profiles, &reference.id).await, 0);
}

#[tokio::test]
async fn name_and_limit_failures_do_not_record_an_additional_use() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let first = publish(
        &profiles,
        profile(
            "wkp_limit_first",
            "consumer",
            "first-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let second = publish(
        &profiles,
        profile(
            "wkp_limit_second",
            "consumer",
            "second-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_limit", SandboxStatus::Active, Vec::new(), 1);
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store, profiles.clone());
    let consumer = caller("consumer");

    service
        .create(&consumer, &parent.id, request(first.clone(), None))
        .await
        .unwrap();
    assert_eq!(
        service
            .create(&consumer, &parent.id, request(first.clone(), None))
            .await
            .unwrap_err(),
        WorkspaceServiceError::NameConflict
    );
    assert_eq!(use_count(&profiles, &first.id).await, 1);

    assert_eq!(
        service
            .create(&consumer, &parent.id, request(second.clone(), None))
            .await
            .unwrap_err(),
        WorkspaceServiceError::LimitReached { limit: 1, live: 1 }
    );
    assert_eq!(use_count(&profiles, &second.id).await, 0);
}

#[tokio::test]
async fn caller_snapshot_cannot_authorize_a_different_durable_parent_slot() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let durable = publish(
        &profiles,
        profile(
            "wkp_durable",
            "consumer",
            "durable-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let stale = publish(
        &profiles,
        profile(
            "wkp_stale",
            "consumer",
            "stale-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let durable_parent = sandbox(
        "sbx_stale_parent",
        SandboxStatus::Installing,
        vec![durable],
        4,
    );
    store.create_sandbox(durable_parent.clone()).await.unwrap();
    let mut caller_snapshot = durable_parent.clone();
    caller_snapshot.profile.content.workspace_profiles = vec![stale.clone()];
    let service = WorkspaceService::new(store.clone(), profiles.clone());

    assert_eq!(
        service
            .create_profile_workspace(&caller_snapshot, &stale, 0)
            .await
            .unwrap_err(),
        WorkspaceServiceError::OriginConflict
    );
    assert_eq!(use_count(&profiles, &stale.id).await, 0);
    assert!(store
        .list_workspaces(&durable_parent.id, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn durable_slot_change_rejects_an_obsolete_existing_replay() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let obsolete = publish(
        &profiles,
        profile(
            "wkp_obsolete",
            "consumer",
            "obsolete-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let current = publish(
        &profiles,
        profile(
            "wkp_current",
            "consumer",
            "current-project",
            ProfileVisibility::Private,
            Vec::new(),
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let stale_parent = sandbox(
        "sbx_changed_parent",
        SandboxStatus::Installing,
        vec![obsolete.clone()],
        4,
    );
    store.create_sandbox(stale_parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles.clone());
    let first = service
        .create_profile_workspace(&stale_parent, &obsolete, 0)
        .await
        .unwrap();
    assert_eq!(use_count(&profiles, &obsolete.id).await, 1);

    let mut durable_parent = stale_parent.clone();
    durable_parent.profile.content.workspace_profiles = vec![current];
    durable_parent.revision += 1;
    store.save_sandbox(1, durable_parent).await.unwrap();

    assert_eq!(
        service
            .create_profile_workspace(&stale_parent, &obsolete, 0)
            .await
            .unwrap_err(),
        WorkspaceServiceError::OriginConflict
    );
    assert_eq!(use_count(&profiles, &obsolete.id).await, 1);
    assert_eq!(
        store
            .list_workspaces(&stale_parent.id, &Default::default())
            .await
            .unwrap()
            .items,
        vec![first]
    );
}

#[tokio::test]
async fn qualified_public_source_profile_is_bound_to_its_original_lifecycle() {
    use crate::sandbox::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore};
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let mut public = profile(
        "wkp_public_source",
        "consumer",
        "workspace",
        ProfileVisibility::Private,
        Vec::new(),
    );
    public.draft.location = WorkspaceLocation::PublicRepositorySnapshot {
        url: "https://github.com/example/project".into(),
        commit: "a".repeat(40),
        operation_id: "original-operation".into(),
    };
    let reference = publish(&profiles, public).await;
    let mut original = sandbox(
        "sbx_public_source",
        SandboxStatus::Provisioning,
        vec![reference.clone()],
        4,
    );
    original.origin = SandboxOrigin::Lifecycle {
        operation_id: "original-operation".into(),
    };
    original.profile.content.runtime_image = Some(format!(
        "registry.example/runtime@sha256:{}",
        "a".repeat(64)
    ));
    let mut other = sandbox(
        "sbx_other_source",
        SandboxStatus::Active,
        vec![reference.clone()],
        4,
    );
    other.origin = SandboxOrigin::Lifecycle {
        operation_id: "different-operation".into(),
    };
    other.profile.content.runtime_image = Some(format!(
        "registry.example/runtime@sha256:{}",
        "b".repeat(64)
    ));
    original.profile.digest = content_digest(&original.profile.content).unwrap();
    other.profile.digest = content_digest(&other.profile.content).unwrap();
    other.status = SandboxStatus::Provisioning;
    for parent in [&original, &other] {
        store
            .admit_sandbox(
                SandboxAdmission::new(
                    parent.clone(),
                    SandboxAdmissionName::Exact(parent.name.clone()),
                    8,
                    NOW.into(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    original.revision += 1;
    original.status = SandboxStatus::Installing;
    original.work.step = SandboxWorkStep::RunInstall;
    store.save_sandbox(1, original.clone()).await.unwrap();
    other.revision += 1;
    other.status = SandboxStatus::Active;
    other.work.step = SandboxWorkStep::None;
    store.save_sandbox(1, other.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles);
    let accepted = service
        .create_profile_workspace(&original, &reference, 0)
        .await
        .unwrap();
    let replay = service
        .create_profile_workspace(&original, &reference, 0)
        .await
        .unwrap();
    assert_eq!(accepted.id, replay.id);
    assert!(matches!(
        accepted.source,
        WorkspaceSource::PublicRepositorySnapshot { .. }
    ));
    assert!(matches!(
        service
            .create_profile_workspace(&other, &reference, 0)
            .await,
        Err(WorkspaceServiceError::InvalidProfile(_))
    ));
    assert!(matches!(
        service
            .create(
                &caller("consumer"),
                &other.id,
                CreateWorkspace {
                    profile: reference,
                    confirmed_digest: None
                }
            )
            .await,
        Err(WorkspaceServiceError::InvalidProfile(_))
    ));
}
