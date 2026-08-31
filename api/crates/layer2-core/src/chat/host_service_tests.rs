use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{KernelAdapter, KernelCapabilities, RuntimePermission};
use tokio::sync::Mutex;

use super::*;
use crate::store::{
    ChatHostStore, Page, PageCursor, PageRequest, PlatformSettingsStore, StoreError,
};
use crate::test_support::{
    fixture_skill_ports, FakeAdapter, FakeProfileStore, FakeStore, FixedCredential, NullScreen,
};
use crate::{
    AgentConfiguration, Baseline, CoreError, EnvironmentProfileService, KernelId, PlatformSettings,
    PlatformSettingsService, ServiceId, SessionBriefing, SkillId, SoftwarePackage, StoredSettings,
    SubjectId,
};

#[derive(Default)]
struct MemoryChatHosts {
    records: Mutex<HashMap<String, ChatHostRecord>>,
    creates: AtomicUsize,
    fail_updates: AtomicUsize,
}

impl MemoryChatHosts {
    async fn records(&self) -> Vec<ChatHostRecord> {
        self.records.lock().await.values().cloned().collect()
    }
}

#[async_trait::async_trait]
impl ChatHostStore for MemoryChatHosts {
    async fn create_if_absent(
        &self,
        candidate: &ChatHostRecord,
    ) -> Result<ChatHostRecord, StoreError> {
        candidate.validate().map_err(StoreError::Malformed)?;
        let mut records = self.records.lock().await;
        if let Some(existing) = records
            .values()
            .find(|record| record.owner == candidate.owner)
        {
            return Ok(existing.clone());
        }
        self.creates.fetch_add(1, Ordering::SeqCst);
        let mut inserted = candidate.clone();
        inserted.revision = 1;
        records.insert(inserted.id.0.clone(), inserted.clone());
        Ok(inserted)
    }

    async fn get(&self, id: &ChatHostId) -> Result<Option<ChatHostRecord>, StoreError> {
        Ok(self.records.lock().await.get(id.as_str()).cloned())
    }

    async fn get_for_owner(&self, owner: &SubjectId) -> Result<Option<ChatHostRecord>, StoreError> {
        Ok(self
            .records
            .lock()
            .await
            .values()
            .find(|record| &record.owner == owner)
            .cloned())
    }

    async fn update_if_revision(
        &self,
        expected: u64,
        replacement: &ChatHostRecord,
    ) -> Result<Option<ChatHostRecord>, StoreError> {
        if self
            .fail_updates
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable("injected host-store loss".into()));
        }
        replacement.validate().map_err(StoreError::Malformed)?;
        if replacement.revision != expected {
            return Err(StoreError::Malformed(
                "replacement revision does not equal its CAS expectation".into(),
            ));
        }
        let mut records = self.records.lock().await;
        let Some(current) = records.get(replacement.id.as_str()) else {
            return Ok(None);
        };
        if current.revision != expected || current.owner != replacement.owner {
            return Ok(None);
        }
        let mut stored = replacement.clone();
        stored.revision = expected
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("revision overflow".into()))?;
        records.insert(stored.id.0.clone(), stored.clone());
        Ok(Some(stored))
    }

    async fn list(&self, query: &ChatHostListQuery) -> Result<Page<ChatHostRecord>, StoreError> {
        let mut records = self.records().await;
        if !query.statuses.is_empty() {
            records.retain(|record| query.statuses.contains(&record.status));
        }
        if let Some(cursor) = query.page.cursor() {
            records.retain(|record| {
                record.created_at < cursor.created_at
                    || (record.created_at == cursor.created_at && record.id.0 < cursor.id)
            });
        }
        records.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.0.cmp(&left.id.0))
        });
        let has_more = records.len() > query.page.limit() as usize;
        records.truncate(query.page.limit() as usize);
        let next = has_more.then(|| {
            let last = records.last().expect("a bounded page has a last record");
            PageCursor::new(&last.created_at, last.id.as_str())
        });
        Ok(Page {
            items: records,
            next,
        })
    }

    async fn list_candidates(
        &self,
        query: &ChatHostListQuery,
    ) -> Result<Page<ChatHostRecord>, StoreError> {
        self.list(query).await
    }
}

struct MutableSettings(Mutex<StoredSettings>);

impl MutableSettings {
    fn new(settings: PlatformSettings) -> Self {
        Self(Mutex::new(StoredSettings {
            settings,
            revision: 1,
            previous: None,
        }))
    }

    async fn replace(&self, settings: PlatformSettings) {
        let mut stored = self.0.lock().await;
        stored.previous = Some(stored.settings.clone());
        stored.settings = settings;
        stored.revision += 1;
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for MutableSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Ok(Some(self.0.lock().await.clone()))
    }

    async fn save_if_unchanged(
        &self,
        expected: u64,
        settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        let mut stored = self.0.lock().await;
        if stored.revision != expected {
            return Ok(None);
        }
        stored.previous = Some(stored.settings.clone());
        stored.settings = settings.clone();
        stored.revision += 1;
        Ok(Some(stored.clone()))
    }
}

struct HostFixture {
    service: Arc<crate::SessionService>,
    adapter: Arc<FakeAdapter>,
    hosts: Arc<MemoryChatHosts>,
    settings: Arc<MutableSettings>,
}

fn enabled_chat() -> ChatSettings {
    ChatSettings {
        enabled: true,
        setup: ChatSetup {
            commands: vec!["prepare one".into(), "prepare two".into()],
        },
        ..ChatSettings::default()
    }
}

fn platform_with(chat: Option<ChatSettings>) -> PlatformSettings {
    PlatformSettings {
        chat,
        briefing: SessionBriefing {
            before: vec!["Pinned platform briefing".into()],
            ..SessionBriefing::default()
        },
        ..PlatformSettings::default()
    }
}

fn fixture(
    settings: PlatformSettings,
    capabilities: KernelCapabilities,
    baseline: Baseline,
) -> HostFixture {
    fixture_with_host_store(settings, capabilities, baseline, true)
}

fn fixture_with_host_store(
    settings: PlatformSettings,
    capabilities: KernelCapabilities,
    baseline: Baseline,
    compose_host_store: bool,
) -> HostFixture {
    let settings = Arc::new(MutableSettings::new(settings));
    let settings_service = Arc::new(PlatformSettingsService::new(
        Arc::clone(&settings) as Arc<dyn PlatformSettingsStore>
    ));
    let environments = EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline,
        None,
        None,
        AgentConfiguration {
            kernel: KernelId::new("fake"),
            ..AgentConfiguration::default()
        },
        Arc::clone(&settings_service),
    );
    let adapter = Arc::new(FakeAdapter {
        capabilities,
        created_by: Some("chat-test-deployment".into()),
        ..FakeAdapter::default()
    });
    let hosts = Arc::new(MemoryChatHosts::default());
    let sessions = Arc::new(FakeStore::default());
    let (skills, _) = fixture_skill_ports(&sessions);
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("fake"), adapter.clone());
    let screen: crate::ScreenFactory = Arc::new(|geometry| Box::new(NullScreen(geometry)));
    let service = crate::SessionService::new(
        adapters,
        screen,
        sessions,
        crate::SessionPorts {
            credentials: Some(Arc::new(FixedCredential("host-credential".into()))),
            skills: Some(skills),
            settings: Some(settings_service),
            chat_hosts: compose_host_store.then(|| Arc::clone(&hosts) as Arc<dyn ChatHostStore>),
            ..crate::SessionPorts::default()
        },
        environments,
        crate::service::Deployment {
            runtime_version: Some("runtime-chat-v1".into()),
            identity: Some("chat-test-deployment".into()),
            adopts_untagged: false,
            ..crate::service::Deployment::default()
        },
    );
    HostFixture {
        service,
        adapter,
        hosts,
        settings,
    }
}

fn chat_capabilities() -> KernelCapabilities {
    KernelCapabilities {
        conversation: true,
        snapshot: true,
        ..KernelCapabilities::default()
    }
}

async fn stored_host(fixture: &HostFixture, owner: &SubjectId) -> ChatHostRecord {
    fixture
        .hosts
        .get_for_owner(owner)
        .await
        .unwrap()
        .expect("chat host is durable")
}

#[tokio::test]
async fn concurrent_first_requests_create_one_record_and_one_runtime() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    fixture
        .adapter
        .pause_next_provision
        .store(true, Ordering::SeqCst);
    let owner = SubjectId("owner-concurrent".into());
    let first_service = Arc::clone(&fixture.service);
    let first_owner = owner.clone();
    let first =
        tokio::spawn(async move { first_service.ensure_chat_host(&first_owner, None).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.provision_paused.notified(),
    )
    .await
    .expect("first provider create starts");
    let second_service = Arc::clone(&fixture.service);
    let second_owner = owner.clone();
    let second =
        tokio::spawn(async move { second_service.ensure_chat_host(&second_owner, None).await });
    fixture.adapter.provision_resume.notify_one();

    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.status, ChatHostStatus::Active);
    assert_eq!(fixture.hosts.creates.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.hosts.records().await.len(), 1);
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
}

#[tokio::test]
async fn absent_and_disabled_settings_touch_neither_store_nor_provider() {
    for settings in [
        platform_with(None),
        platform_with(Some(ChatSettings::default())),
    ] {
        let fixture = fixture(settings, chat_capabilities(), Baseline::default());
        let error = fixture
            .service
            .ensure_chat_host(&SubjectId("owner-dark".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            CoreError::ChatUnavailable(
                ChatUnavailableCause::NotConfigured | ChatUnavailableCause::Disabled
            )
        ));
        assert!(fixture.hosts.records().await.is_empty());
        assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn missing_host_store_is_a_typed_refusal_and_does_not_affect_the_provider() {
    let fixture = fixture_with_host_store(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
        false,
    );
    let error = fixture
        .service
        .ensure_chat_host(&SubjectId("owner-no-host-store".into()), None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::HostStoreUnavailable)
    ));
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_conversation_or_snapshot_is_refused_before_any_write() {
    for capabilities in [
        KernelCapabilities {
            conversation: false,
            snapshot: true,
            ..KernelCapabilities::default()
        },
        KernelCapabilities {
            conversation: true,
            snapshot: false,
            ..KernelCapabilities::default()
        },
    ] {
        let fixture = fixture(
            platform_with(Some(enabled_chat())),
            capabilities,
            Baseline::default(),
        );
        let error = fixture
            .service
            .ensure_chat_host(&SubjectId("owner-unsupported".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            CoreError::ChatUnavailable(ChatUnavailableCause::CapabilityUnsupported(_))
        ));
        assert!(fixture.hosts.records().await.is_empty());
        assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn workspace_clone_failure_deletes_exact_runtime_and_is_retryable() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    fixture
        .adapter
        .fail_workspace_sources
        .store(1, Ordering::SeqCst);
    let owner = SubjectId("owner-clone-failure".into());
    let error = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::WorkspaceSource,
            command_index: None,
            retryable: true,
        })
    ));
    let host = stored_host(&fixture, &owner).await;
    assert_eq!(host.status, ChatHostStatus::Provisioning);
    assert!(host.runtime_id.is_none());
    assert_eq!(host.last_transition_claim, None);
    assert!(!host.last_failure.unwrap().cleanup_pending);
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
    assert!(fixture.adapter.running.lock().await.is_empty());
}

#[tokio::test]
async fn a_clean_clone_failure_can_be_retried_without_an_orphan() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    fixture
        .adapter
        .fail_workspace_sources
        .store(1, Ordering::SeqCst);
    let owner = SubjectId("owner-clone-retry".into());
    fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap_err();

    let recovered = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap();

    assert_eq!(recovered.status, ChatHostStatus::Active);
    assert!(recovered.last_failure.is_none());
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
}

#[tokio::test]
async fn runtime_id_store_loss_attempts_exact_cleanup() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    fixture.hosts.fail_updates.store(1, Ordering::SeqCst);
    let owner = SubjectId("owner-id-store-loss".into());
    let error = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::HostStoreUnavailable)
    ));
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
    assert!(fixture.adapter.running.lock().await.is_empty());
    let host = stored_host(&fixture, &owner).await;
    assert!(host.runtime_id.is_none());
}

#[tokio::test]
async fn failed_exact_cleanup_retains_runtime_authority_for_reconciliation() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    fixture
        .adapter
        .fail_workspace_sources
        .store(1, Ordering::SeqCst);
    fixture.adapter.fail_terminations.store(1, Ordering::SeqCst);
    let owner = SubjectId("owner-cleanup-debt".into());
    fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap_err();
    let host = stored_host(&fixture, &owner).await;
    assert!(host.runtime_id.is_some());
    assert!(host.last_failure.unwrap().cleanup_pending);
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
}

#[tokio::test]
async fn nonzero_setup_preserves_only_the_failing_command_index_and_cleans_runtime() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    *fixture.adapter.workspace_setup_failure_index.lock().await = Some(1);
    let owner = SubjectId("owner-setup-failure".into());
    let error = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::WorkspaceSetup,
            command_index: Some(1),
            retryable: true,
        })
    ));
    let host = stored_host(&fixture, &owner).await;
    let failure = host.last_failure.unwrap();
    assert_eq!(failure.command_index, Some(1));
    assert!(host.runtime_id.is_none());
    assert!(fixture.adapter.running.lock().await.is_empty());
}

#[tokio::test]
async fn runtime_spec_carries_pinned_chat_policy_and_deployment_baseline() {
    let mut chat = enabled_chat();
    chat.agent.model = Some("chat-model".into());
    chat.agent.reasoning_effort = crate::ReasoningEffort::Medium;
    chat.agent.instructions = "Owner-independent chat instructions".into();
    chat.host.max_threads = 7;
    let baseline = Baseline::new(
        [SoftwarePackage::new("ripgrep")],
        [SkillId::new("review-skill")],
        [ServiceId::new("model-service")],
    );
    let fixture = fixture(
        platform_with(Some(chat.clone())),
        chat_capabilities(),
        baseline,
    );
    let host = fixture
        .service
        .ensure_chat_host(&SubjectId("owner-spec".into()), None)
        .await
        .unwrap();
    let spec = fixture.adapter.last_spec.lock().await.clone().unwrap();
    assert_eq!(spec.session_id, host.id.0);
    assert_eq!(spec.chat_host.as_deref(), Some(host.id.as_str()));
    assert_eq!(spec.generation.unwrap().get(), 1);
    assert!(spec.workspace_source_required);
    assert_eq!(spec.conversation_slots_required, Some(7));
    assert_eq!(spec.options.model.as_deref(), Some("chat-model"));
    assert_eq!(spec.options.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(spec.options.permission, RuntimePermission::Unrestricted);
    assert_eq!(
        spec.environment.agent_notes,
        "Owner-independent chat instructions"
    );
    assert_eq!(spec.environment.packages, ["ripgrep"]);
    assert_eq!(spec.environment.skills, ["review-skill"]);
    assert_eq!(spec.reaches, ["model-service"]);
    assert_eq!(spec.skills.len(), 1);
    assert!(spec.platform_briefing.contains("Pinned platform briefing"));
    assert!(spec.platform_briefing.contains("model-service"));
    let sources = fixture.adapter.workspace_sources.lock().await;
    assert_eq!(sources[0].1.repository_url, chat.workspace.repository_url);
    assert_eq!(sources[0].1.reference, chat.workspace.branch);
}

#[tokio::test]
async fn settings_edits_do_not_change_an_existing_pinned_host() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let owner = SubjectId("owner-pinned".into());
    let original = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap();
    let mut changed = enabled_chat();
    changed.enabled = false;
    changed.agent.model = Some("later-model".into());
    changed.agent.instructions = "later instructions".into();
    changed.workspace.branch = "later-branch".into();
    fixture.settings.replace(platform_with(Some(changed))).await;

    let existing = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap();
    assert_eq!(existing, original);
    assert_eq!(existing.pinned_settings.settings_revision, 1);
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restart_reconciliation_adopts_metadata_runtime_and_replays_setup() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let owner = SubjectId("owner-restart".into());
    let candidate = ChatHostRecord::new(
        owner.clone(),
        ChatSettingsSnapshot::new(1, enabled_chat()).unwrap(),
        "Pinned platform briefing\n".into(),
        KernelId::new("fake"),
        "2026-09-01T00:00:00Z".into(),
        "claim-from-dead-process".into(),
    );
    let host = fixture.hosts.create_if_absent(&candidate).await.unwrap();
    let runtime = format!("runtime-adopted-for-{}", host.id);
    fixture
        .adapter
        .running
        .lock()
        .await
        .insert(runtime.clone(), Some(host.id.0.clone()));
    fixture
        .adapter
        .generations
        .lock()
        .await
        .insert(runtime.clone(), std::num::NonZeroU64::new(1).unwrap());
    fixture
        .adapter
        .chat_hosts
        .lock()
        .await
        .insert(runtime.clone(), host.id.0.clone());

    fixture.service.reconcile().await;

    let adopted = stored_host(&fixture, &owner).await;
    assert_eq!(adopted.status, ChatHostStatus::Active);
    assert_eq!(adopted.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.adapter.workspace_sources.lock().await.len(), 1);
    assert_eq!(fixture.adapter.workspace_setups.lock().await.len(), 1);
}

#[tokio::test]
async fn reconciliation_keeps_the_recorded_runtime_and_removes_a_duplicate() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let owner = SubjectId("owner-duplicate-runtime".into());
    let host = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap();
    let authoritative = host.runtime_id.clone().unwrap();
    let duplicate = format!("runtime-duplicate-for-{}", host.id);
    fixture
        .adapter
        .running
        .lock()
        .await
        .insert(duplicate.clone(), Some(host.id.0.clone()));
    fixture
        .adapter
        .generations
        .lock()
        .await
        .insert(duplicate.clone(), std::num::NonZeroU64::new(1).unwrap());
    fixture
        .adapter
        .chat_hosts
        .lock()
        .await
        .insert(duplicate.clone(), host.id.0.clone());

    fixture.service.reconcile().await;

    let settled = stored_host(&fixture, &owner).await;
    assert_eq!(settled.runtime_id.as_deref(), Some(authoritative.as_str()));
    let running = fixture.adapter.running.lock().await;
    assert!(running.contains_key(&authoritative));
    assert!(!running.contains_key(&duplicate));
    assert!(fixture.adapter.terminated.lock().await.contains(&duplicate));
}

fn checkpoint() -> ChatCheckpointRef {
    ChatCheckpointRef {
        workspace_object_reference: "objects/workspace".into(),
        workspace_archive_sha256: "a".repeat(64),
        workspace_manifest_digest: "b".repeat(64),
        conversations: Vec::new(),
        captured_at: "2026-09-01T01:00:00Z".into(),
        checkpoint_version: 3,
        pair_sha256: "c".repeat(64),
    }
}

#[tokio::test]
async fn trusted_runtime_loss_parks_hosts_without_inventing_or_altering_checkpoints() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let owner_with = SubjectId("owner-checkpoint".into());
    let owner_without = SubjectId("owner-no-checkpoint".into());
    let with = fixture
        .service
        .ensure_chat_host(&owner_with, None)
        .await
        .unwrap();
    fixture
        .service
        .ensure_chat_host(&owner_without, None)
        .await
        .unwrap();
    let mut with_checkpoint = fixture.hosts.get(&with.id).await.unwrap().unwrap();
    with_checkpoint.checkpoint = Some(checkpoint());
    fixture
        .hosts
        .update_if_revision(with_checkpoint.revision, &with_checkpoint)
        .await
        .unwrap()
        .unwrap();
    fixture.adapter.running.lock().await.clear();
    fixture.adapter.generations.lock().await.clear();
    fixture.adapter.chat_hosts.lock().await.clear();

    fixture.service.reconcile().await;

    let with = stored_host(&fixture, &owner_with).await;
    let without = stored_host(&fixture, &owner_without).await;
    assert_eq!(with.status, ChatHostStatus::Paused);
    assert_eq!(with.checkpoint, Some(checkpoint()));
    assert_eq!(without.status, ChatHostStatus::Paused);
    assert_eq!(without.checkpoint, None);
    for host in [with, without] {
        assert_eq!(host.runtime_id, None);
        assert_eq!(
            host.last_failure.unwrap().phase,
            ChatProvisioningPhase::RuntimeLost
        );
    }
}

#[tokio::test]
async fn credential_delivery_is_bound_to_the_active_host_generation() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let host = fixture
        .service
        .ensure_chat_host(&SubjectId("owner-credential".into()), None)
        .await
        .unwrap();
    fixture
        .service
        .install_chat_host_credential(&host.id, None)
        .await
        .unwrap();
    assert_eq!(
        *fixture.adapter.credentials.lock().await,
        ["host-credential"]
    );
}

#[tokio::test]
async fn empty_host_retirement_removes_exact_runtime_and_marks_terminal_state() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let host = fixture
        .service
        .ensure_chat_host(&SubjectId("owner-retire".into()), None)
        .await
        .unwrap();
    let retired = fixture
        .service
        .retire_chat_host_if_empty(&host.id)
        .await
        .unwrap();
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert!(retired.runtime_id.is_none());
    assert!(retired.retired_at.is_some());
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
    assert!(fixture.adapter.running.lock().await.is_empty());
}

#[test]
fn bounded_memory_store_query_matches_contract() {
    let query = ChatHostListQuery::new(
        PageRequest::new(1, Some(PageCursor::new("2026-09-01T00:00:00Z", "last"))),
        vec![ChatHostStatus::Active],
    );
    assert_eq!(query.page.limit(), 1);
    assert_eq!(query.statuses, [ChatHostStatus::Active]);
}
