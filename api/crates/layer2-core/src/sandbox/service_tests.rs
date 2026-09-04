use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::profiles::store::{InMemoryProfileStore, SandboxProfileStore as _};
use crate::profiles::{
    AgentProvider, EnvironmentVariable, ModelAccess, ProfileCapacityBounds, ProfileVisibility,
    SandboxProfile, SandboxProfileContent, SandboxProfileRef,
};
use crate::sandbox::store::{
    PageRequest, SandboxAdmission, SandboxAdmissionStore, SandboxEventStore, SandboxStore as _,
    SandboxTransaction, SandboxTransactionStore, SandboxWorkerStore as _,
};
use crate::{Authorization, Caller, Capability, SandboxId, SandboxProfileId, ServiceId, SubjectId};

use super::service::DEFAULT_RETENTION_GRACE;
use super::*;
use tokio::sync::Notify;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const LEASE_END: &str = "2026-09-05T00:02:00Z";

struct MutablePolicy(Mutex<SandboxPolicy>);

#[async_trait::async_trait]
impl SandboxPolicySource for MutablePolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, crate::StoreError> {
        Ok(self.0.lock().expect("policy lock").clone())
    }
}

#[derive(Default)]
struct RacingStore {
    inner: InMemorySandboxStore,
    block_next_commit: AtomicBool,
    commit_entered: Notify,
    release_commit: Notify,
}

impl RacingStore {
    fn block_next_commit(&self) {
        self.block_next_commit.store(true, Ordering::SeqCst);
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
    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<Sandbox, SandboxStoreError> {
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
        id: SandboxProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: name.into(),
        description: "Sandbox service test profile".into(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            cpu_millis: 1_000,
            memory_mib: 2_048,
            nyxid_services: vec![ServiceId::new("profile-service")],
            agent_provider: AgentProvider::Codex,
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

async fn publish(profiles: &InMemoryProfileStore, profile: SandboxProfile) -> SandboxProfileRef {
    let id = profile.id.clone();
    profiles.create_sandbox_profile(profile).await.unwrap();
    let revision = profiles
        .publish_sandbox_profile(&id, 1, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    SandboxProfileRef::new(id, revision.revision)
}

fn caller(subject: &str, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(SubjectId(subject.into()), Authorization::new(capabilities))
}

fn request(profile: SandboxProfileRef) -> CreateSandbox {
    CreateSandbox {
        profile,
        name: None,
        trust_confirmed: false,
        origin: SandboxOrigin::Manual,
    }
}

fn policy(owner: u16, visitor: u16) -> SandboxPolicy {
    SandboxPolicy {
        max_sandboxes_per_owner: owner,
        max_sandboxes_per_visitor: visitor,
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
    let service = SandboxService::new(store, profiles, policy(10, 10));
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
        SandboxServiceError::TrustConfirmationRequired
    );
    let mut confirmed = request(public_unsafe);
    confirmed.trust_confirmed = true;
    service.create(&caller, confirmed).await.unwrap();
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
    let service = SandboxService::new(store, profiles, policy(10, 10));
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
async fn create_uses_origin_specific_capabilities_and_limits() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_origin",
            "owner",
            "origin",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;

    let manual_store = Arc::new(InMemorySandboxStore::default());
    let manual_service = SandboxService::new(manual_store, profiles.clone(), policy(3, 1));
    let manual = caller("owner", [Capability::CreateSandbox]);
    for _ in 0..3 {
        manual_service
            .create(&manual, request(reference.clone()))
            .await
            .unwrap();
    }
    assert_eq!(
        manual_service
            .create(&manual, request(reference.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxLimitReached { limit: 3, live: 3 }
    );
    let mut trigger_request = request(reference.clone());
    trigger_request.origin = SandboxOrigin::Trigger {
        launch_id: "launch-1".into(),
    };
    assert_eq!(
        manual_service
            .create(&manual, trigger_request)
            .await
            .unwrap_err(),
        SandboxServiceError::Forbidden
    );

    let visitor_store = Arc::new(InMemorySandboxStore::default());
    let visitor_service = SandboxService::new(visitor_store, profiles, policy(3, 1));
    let launcher = caller("owner", [Capability::LaunchTriggers]);
    let mut trigger_request = request(reference.clone());
    trigger_request.origin = SandboxOrigin::Trigger {
        launch_id: "launch-1".into(),
    };
    visitor_service
        .create(&launcher, trigger_request.clone())
        .await
        .unwrap();
    assert_eq!(
        visitor_service
            .create(&launcher, trigger_request)
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxLimitReached { limit: 1, live: 1 }
    );
    assert_eq!(
        visitor_service
            .create(&launcher, request(reference))
            .await
            .unwrap_err(),
        SandboxServiceError::Forbidden
    );
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
        retention_grace: DEFAULT_RETENTION_GRACE,
        baseline_services: vec![ServiceId::new("baseline-one")],
    })));
    let policy_source: Arc<dyn SandboxPolicySource> = source.clone();
    let service = SandboxService::with_policy_source(store, profiles, policy_source);
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
    let service = SandboxService::new(store, profiles, policy(10, 10));
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
    let service = SandboxService::new(store, profiles, policy(10, 10));
    let owner = caller("owner", [Capability::CreateSandbox]);
    let mut create = request(reference);
    create.name = Some("chosen".into());
    assert_eq!(
        service.create(&owner, create.clone()).await.unwrap().name,
        "chosen"
    );
    assert_eq!(
        service.create(&owner, create).await.unwrap_err(),
        SandboxServiceError::NameConflict
    );
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
    let service = SandboxService::new(store.clone(), profiles, policy(3, 1));
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
    let service = SandboxService::new(store.clone(), profiles, policy(3, 1));
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
        ["baseline", "profile-service"]
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
    let reference = publish(&profiles, own_account).await;
    let service = SandboxService::new(store, profiles, policy(3, 1));

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
    let service = SandboxService::new(store, profiles, policy(3, 1));
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
        let service = SandboxService::new(store.clone(), profiles, policy(3, 1));
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
    let service = SandboxService::new(store.clone(), profiles, policy(3, 1));
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
    let service = SandboxService::new(store.clone(), profiles, policy(3, 1));
    let owner = caller("owner", [Capability::CreateSandbox]);
    let sandbox = service.create(&owner, request(reference)).await.unwrap();

    let mut first = store
        .claim_sandbox_work(&sandbox.id, "worker-a", NOW.into(), LATER.into())
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_sandbox_work(&sandbox.id, "worker-b", NOW.into(), LATER.into())
        .await
        .unwrap()
        .is_none());
    let second = store
        .claim_sandbox_work(&sandbox.id, "worker-b", LATER.into(), LEASE_END.into())
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
            SandboxServiceError::TrustConfirmationRequired,
            "trust_confirmation_required",
        ),
        (
            SandboxServiceError::SandboxLimitReached { limit: 3, live: 3 },
            "sandbox_limit_reached",
        ),
        (SandboxServiceError::NameConflict, "sandbox_name_conflict"),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code);
    }
}
