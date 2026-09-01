use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    KernelAdapter, KernelCapabilities, WorkspaceCheckpointDisposition, WorkspaceCheckpointPackage,
    WorkspaceCheckpointRequest, WorkspaceEntryKind, WorkspaceEntryPackage, WorkspaceWriterState,
};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::*;
use crate::store::{
    ChatCheckpointObjectReceipt, ChatCheckpointStore, ChatHostStore, Page, PageCursor,
    PlatformSettingsStore, StoreError,
};
use crate::test_support::{
    fixture_skill_ports, spec, user, FakeAdapter, FakeProfileStore, FakeStore, FixedCredential,
    NullScreen,
};
use crate::{
    AgentConfiguration, Baseline, ChatCheckpointOutcome, ChatCheckpointSweep, CoreError,
    EnvironmentProfileService, KernelId, PlatformSettings, PlatformSettingsService, Session,
    SessionPorts, SessionService, SessionSpec, SessionStatus, SessionSurface, StoredSettings,
    SubjectId,
};

const CAPTURE_AT: &str = "2026-09-01T02:00:00Z";
const FIVE_MINUTES_LATER: &str = "2026-09-01T02:05:00Z";

#[derive(Default)]
pub(super) struct MemoryChatHosts {
    pub(super) records: Mutex<HashMap<String, ChatHostRecord>>,
    creates: AtomicUsize,
    pub(super) fail_updates: AtomicUsize,
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

#[derive(Default)]
pub(super) struct MemoryCheckpoints {
    objects: Mutex<HashMap<String, BTreeMap<String, Vec<u8>>>>,
    stages: Mutex<Vec<(String, Option<String>, String)>>,
    prunes: Mutex<Vec<(String, String)>>,
    fail_stages: AtomicUsize,
    tamper_next_read: AtomicBool,
}

impl MemoryCheckpoints {
    async fn stage(
        &self,
        key: String,
        retain_sha256: Option<&str>,
        payload: Vec<u8>,
    ) -> Result<ChatCheckpointObjectReceipt, StoreError> {
        if payload.is_empty() {
            return Err(StoreError::Malformed(
                "a checkpoint object must not be empty".into(),
            ));
        }
        if self
            .fail_stages
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable(
                "injected checkpoint storage loss".into(),
            ));
        }
        let sha256 = digest(&payload);
        let mut objects = self.objects.lock().await;
        let existing = objects.get(&key);
        let retained = match retain_sha256 {
            Some(retain_sha256) => Some(
                existing
                    .and_then(|generations| generations.get(retain_sha256))
                    .cloned()
                    .ok_or_else(|| {
                        StoreError::Unavailable(
                            "the committed checkpoint generation is missing".into(),
                        )
                    })?,
            ),
            None => None,
        };
        let mut generations = BTreeMap::new();
        if let (Some(retain_sha256), Some(retained)) = (retain_sha256, retained) {
            generations.insert(retain_sha256.to_string(), retained);
        }
        generations.insert(sha256.clone(), payload.clone());
        objects.insert(key.clone(), generations);
        drop(objects);
        self.stages.lock().await.push((
            key.clone(),
            retain_sha256.map(str::to_string),
            sha256.clone(),
        ));
        Ok(ChatCheckpointObjectReceipt {
            object_reference: key,
            sha256,
            bytes: u64::try_from(payload.len()).unwrap(),
        })
    }

    async fn read(&self, key: &str, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let mut found = self
            .objects
            .lock()
            .await
            .get(key)
            .and_then(|generations| generations.get(sha256))
            .cloned();
        if self.tamper_next_read.swap(false, Ordering::SeqCst) {
            if let Some(bytes) = found.as_mut() {
                bytes.push(0xff);
            }
        }
        Ok(found)
    }

    async fn prune(&self, key: String, keep_sha256: &str) -> Result<(), StoreError> {
        let mut objects = self.objects.lock().await;
        let generations = objects
            .get_mut(&key)
            .ok_or_else(|| StoreError::Unavailable("checkpoint object is missing".into()))?;
        let retained = generations
            .get(keep_sha256)
            .cloned()
            .ok_or_else(|| StoreError::Unavailable("checkpoint generation is missing".into()))?;
        generations.clear();
        generations.insert(keep_sha256.to_string(), retained);
        drop(objects);
        self.prunes
            .lock()
            .await
            .push((key, keep_sha256.to_string()));
        Ok(())
    }

    async fn generation_count(&self, key: &str) -> usize {
        self.objects.lock().await.get(key).map_or(0, BTreeMap::len)
    }
}

#[async_trait::async_trait]
impl ChatCheckpointStore for MemoryCheckpoints {
    async fn stage_conversation(
        &self,
        id: &crate::SessionId,
        retain_sha256: Option<&str>,
        snapshot: Vec<u8>,
    ) -> Result<ChatCheckpointObjectReceipt, StoreError> {
        self.stage(
            format!("sessions/{id}/handover.snapshot"),
            retain_sha256,
            snapshot,
        )
        .await
    }

    async fn read_conversation(
        &self,
        id: &crate::SessionId,
        object_reference: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let expected = format!("sessions/{id}/handover.snapshot");
        if object_reference != expected {
            return Err(StoreError::Malformed(
                "conversation checkpoint belongs to another session".into(),
            ));
        }
        self.read(object_reference, sha256).await
    }

    async fn prune_conversation(
        &self,
        id: &crate::SessionId,
        keep_sha256: &str,
    ) -> Result<(), StoreError> {
        self.prune(format!("sessions/{id}/handover.snapshot"), keep_sha256)
            .await
    }

    async fn stage_workspace(
        &self,
        host: &ChatHostId,
        retain_sha256: Option<&str>,
        archive: Vec<u8>,
    ) -> Result<ChatCheckpointObjectReceipt, StoreError> {
        self.stage(
            format!("chat-hosts/{host}/checkpoint-v1"),
            retain_sha256,
            archive,
        )
        .await
    }

    async fn read_workspace(
        &self,
        host: &ChatHostId,
        object_reference: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let expected = format!("chat-hosts/{host}/checkpoint-v1");
        if object_reference != expected {
            return Err(StoreError::Malformed(
                "workspace checkpoint belongs to another host".into(),
            ));
        }
        self.read(object_reference, sha256).await
    }

    async fn prune_workspace(
        &self,
        host: &ChatHostId,
        keep_sha256: &str,
    ) -> Result<(), StoreError> {
        self.prune(format!("chat-hosts/{host}/checkpoint-v1"), keep_sha256)
            .await
    }
}

struct FixedSettings(StoredSettings);

#[async_trait::async_trait]
impl PlatformSettingsStore for FixedSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Ok(Some(self.0.clone()))
    }

    async fn save_if_unchanged(
        &self,
        _expected_revision: u64,
        _settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable(
            "the checkpoint fixture settings are immutable".into(),
        ))
    }
}

pub(super) struct CheckpointFixture {
    pub(super) service: Arc<SessionService>,
    pub(super) adapter: Arc<FakeAdapter>,
    pub(super) hosts: Arc<MemoryChatHosts>,
    pub(super) sessions: Arc<FakeStore>,
    pub(super) checkpoints: Arc<MemoryCheckpoints>,
    pub(super) caller: crate::Caller,
    pub(super) threads: Vec<Session>,
}

fn enabled_settings() -> Arc<PlatformSettingsService> {
    let chat = ChatSettings {
        enabled: true,
        host: ChatHostSettings {
            max_threads: 8,
            ..ChatHostSettings::default()
        },
        ..ChatSettings::default()
    };
    Arc::new(PlatformSettingsService::new(Arc::new(FixedSettings(
        StoredSettings {
            settings: PlatformSettings {
                chat: Some(chat),
                ..PlatformSettings::default()
            },
            revision: 11,
            previous: None,
        },
    ))))
}

pub(super) fn build_service(
    adapter: Arc<FakeAdapter>,
    hosts: Arc<MemoryChatHosts>,
    sessions: Arc<FakeStore>,
    checkpoints: Arc<MemoryCheckpoints>,
) -> Arc<SessionService> {
    let settings = enabled_settings();
    let environments = EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        Baseline::default(),
        None,
        None,
        AgentConfiguration {
            kernel: KernelId::new("fake"),
            ..AgentConfiguration::default()
        },
        Arc::clone(&settings),
    );
    let (skills, _) = fixture_skill_ports(&sessions);
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("fake"), adapter);
    let screen: crate::ScreenFactory = Arc::new(|geometry| Box::new(NullScreen(geometry)));
    SessionService::new(
        adapters,
        screen,
        sessions,
        SessionPorts {
            credentials: Some(Arc::new(FixedCredential("checkpoint-credential".into()))),
            skills: Some(skills),
            settings: Some(settings),
            chat_hosts: Some(hosts),
            chat_checkpoints: Some(checkpoints),
            ..SessionPorts::default()
        },
        environments,
        crate::service::Deployment {
            runtime_version: Some("runtime-chat-v1".into()),
            identity: Some("checkpoint-test-deployment".into()),
            adopts_untagged: false,
            ..crate::service::Deployment::default()
        },
    )
}

fn chat_spec() -> SessionSpec {
    SessionSpec {
        surface: Some(SessionSurface::Chat),
        ..spec()
    }
}

pub(super) async fn fixture(owner: &str, thread_count: usize) -> CheckpointFixture {
    let adapter = Arc::new(FakeAdapter {
        capabilities: KernelCapabilities {
            conversation: true,
            snapshot: true,
            ..KernelCapabilities::default()
        },
        conversation_ids_from_slots: true,
        created_by: Some("checkpoint-test-deployment".into()),
        ..FakeAdapter::default()
    });
    let hosts = Arc::new(MemoryChatHosts::default());
    let sessions = Arc::new(FakeStore::default());
    let checkpoints = Arc::new(MemoryCheckpoints::default());
    let service = build_service(
        Arc::clone(&adapter),
        Arc::clone(&hosts),
        Arc::clone(&sessions),
        Arc::clone(&checkpoints),
    );
    let caller = user(owner);
    let mut threads = Vec::new();
    for index in 1..=thread_count {
        let thread = service
            .create_session(&caller, chat_spec())
            .await
            .expect("the chat thread is created");
        adapter.conversation_snapshots.lock().await.insert(
            format!("conversation-for-{}", thread.id),
            format!("snapshot-{index}").into_bytes(),
        );
        threads.push(thread);
    }
    CheckpointFixture {
        service,
        adapter,
        hosts,
        sessions,
        checkpoints,
        caller,
        threads,
    }
}

pub(super) async fn host(fixture: &CheckpointFixture) -> ChatHostRecord {
    fixture
        .hosts
        .get_for_owner(&fixture.caller.subject)
        .await
        .unwrap()
        .expect("the chat host is durable")
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(char::from(CHARS[usize::from(byte >> 4)]));
        out.push(char::from(CHARS[usize::from(byte & 0x0f)]));
    }
    out
}

fn hash_text(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap().to_be_bytes());
    digest.update(value.as_bytes());
}

fn manifest_digest(source_revision: &str, entries: &[WorkspaceEntryPackage]) -> String {
    let mut manifest = Sha256::new();
    manifest.update(b"cma-workspace-manifest-v1\0");
    manifest.update(b"complete-git-workspace-v1\0");
    hash_text(&mut manifest, source_revision);
    manifest.update(u64::try_from(entries.len()).unwrap().to_be_bytes());
    for entry in entries {
        hash_text(&mut manifest, &entry.path);
        manifest.update([match entry.kind {
            WorkspaceEntryKind::Regular => 1,
            WorkspaceEntryKind::Symlink => 2,
        }]);
        manifest.update(entry.mode.to_be_bytes());
        manifest.update(entry.size.to_be_bytes());
        hash_text(&mut manifest, &entry.content_sha256);
        manifest.update([0]);
        manifest.update([0]);
    }
    hex(&manifest.finalize())
}

fn package(host: &ChatHostRecord, version: u64, marker: &str) -> WorkspaceCheckpointPackage {
    let authority = host
        .workspace_authority
        .as_ref()
        .expect("provisioning sealed workspace authority");
    let mut entries = [
        (".git/HEAD", b"ref: refs/heads/dev\n".as_slice()),
        ("README.md", b"baseline\n".as_slice()),
        ("worktrees/chat-1/work.txt", marker.as_bytes()),
    ]
    .into_iter()
    .map(|(path, content)| WorkspaceEntryPackage {
        path: path.into(),
        kind: WorkspaceEntryKind::Regular,
        mode: 0o100644,
        size: u64::try_from(content.len()).unwrap(),
        content_sha256: digest(content),
        provider_object_id: None,
        symlink_target: None,
    })
    .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let archive = format!("complete workspace generation {marker}").into_bytes();
    WorkspaceCheckpointPackage {
        epoch: authority.workspace_epoch,
        version,
        source_revision: authority.source_revision.clone(),
        manifest_digest: manifest_digest(&authority.source_revision, &entries),
        archive_sha256: digest(&archive),
        total_bytes: entries.iter().map(|entry| entry.size).sum(),
        entries,
        quiesced_writers: 3,
        writer_state: WorkspaceWriterState::HeldForTeardown,
        writer_resume_token: Some(format!("resume-{version}")),
        includes_git_metadata: true,
        archive,
    }
}

pub(super) async fn use_package(fixture: &CheckpointFixture, version: u64, marker: &str) {
    *fixture.adapter.checkpoint.lock().await = Some(package(&host(fixture).await, version, marker));
}

async fn wait_for_checkpoint_fence(adapter: &FakeAdapter) {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("checkpoint reached the held writer fence")
    .expect("the fake adapter remains alive")
    .forget();
}

#[tokio::test]
async fn quiet_active_host_seals_every_durable_conversation_and_complete_git_workspace() {
    let fixture = fixture("checkpoint-complete", 2).await;
    let before = host(&fixture).await;
    let runtime = before
        .runtime_id
        .clone()
        .expect("an active host names its runtime");
    use_package(&fixture, 1, "first").await;
    let package = fixture.adapter.checkpoint.lock().await.clone().unwrap();
    let authority = before.workspace_authority.as_ref().unwrap();
    crate::workspace_checkpoint::validate_workspace_checkpoint_package(
        &WorkspaceCheckpointRequest {
            epoch: authority.workspace_epoch,
            version: 1,
            source_revision: authority.source_revision.clone(),
            baseline_manifest_digest: authority.baseline_manifest_digest.clone(),
            include_git_metadata: true,
            disposition: WorkspaceCheckpointDisposition::HoldForTeardown,
        },
        &package,
    )
    .expect("the fixture package is independently valid");

    let outcome = fixture
        .service
        .checkpoint_chat_host(&before.id, CAPTURE_AT)
        .await
        .expect("checkpoint attempt settles");
    let ChatCheckpointOutcome::Complete(reference) = outcome else {
        panic!(
            "quiet host did not complete: {outcome:?}; host={:?}; snapshots={:?}; stages={:?}; resumes={:?}",
            host(&fixture).await.last_operator_note,
            fixture.adapter.conversation_snapshot_calls.lock().await,
            fixture.checkpoints.stages.lock().await,
            fixture.adapter.workspace_resumes.lock().await,
        );
    };
    reference.validate().unwrap();
    assert_eq!(reference.captured_at, CAPTURE_AT);
    assert_eq!(reference.checkpoint_version, 1);
    assert_eq!(reference.conversations.len(), 2);
    assert_eq!(
        reference.workspace_object_reference,
        format!("chat-hosts/{}/checkpoint-v1", before.id)
    );
    for conversation in &reference.conversations {
        assert_eq!(
            conversation.object_reference,
            format!("sessions/{}/handover.snapshot", conversation.session_id)
        );
    }

    let committed = host(&fixture).await;
    assert_eq!(committed.status, ChatHostStatus::Active);
    assert_eq!(committed.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(committed.checkpoint, Some(reference.clone()));
    assert_eq!(committed.last_transition_claim, None);
    assert_eq!(committed.last_operator_note, None);
    assert_eq!(committed.revision, before.revision + 2);

    let requests = fixture.adapter.checkpoint_requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, runtime);
    assert_eq!(
        requests[0].1,
        WorkspaceCheckpointRequest {
            epoch: 1,
            version: 1,
            source_revision: "a".repeat(40),
            baseline_manifest_digest: "b".repeat(64),
            include_git_metadata: true,
            disposition: WorkspaceCheckpointDisposition::HoldForTeardown,
        }
    );
    drop(requests);
    let mut captured_ids = fixture
        .adapter
        .conversation_snapshot_calls
        .lock()
        .await
        .iter()
        .map(|(_, conversation)| conversation.clone())
        .collect::<Vec<_>>();
    captured_ids.sort();
    let mut expected_ids = fixture
        .threads
        .iter()
        .map(|thread| format!("conversation-for-{}", thread.id))
        .collect::<Vec<_>>();
    expected_ids.sort();
    assert_eq!(captured_ids, expected_ids);
    assert_eq!(
        *fixture.adapter.workspace_resumes.lock().await,
        [(runtime, "resume-1".into())]
    );
    assert_eq!(
        fixture
            .checkpoints
            .generation_count(&reference.workspace_object_reference)
            .await,
        1
    );
    assert_eq!(package.archive_sha256, reference.workspace_archive_sha256);
}

#[tokio::test]
async fn skip_conditions_do_not_churn_host_revision_or_touch_the_runtime() {
    let running = fixture("checkpoint-running", 1).await;
    use_package(&running, 1, "running").await;
    running
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(&running.threads[0].id.0)
        .unwrap()
        .session
        .status = SessionStatus::Running;
    let before = host(&running).await;
    assert_eq!(
        running
            .service
            .checkpoint_chat_host(&before.id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::NotEligible
    );
    assert_eq!(host(&running).await.revision, before.revision);
    assert_eq!(running.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);

    let claimed = fixture("checkpoint-claimed", 1).await;
    let mut held = host(&claimed).await;
    held.last_transition_claim = Some("another-transition".into());
    held.transition_started_at = Some(CAPTURE_AT.into());
    let held = claimed
        .hosts
        .update_if_revision(held.revision, &held)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed
            .service
            .checkpoint_chat_host(&held.id, FIVE_MINUTES_LATER)
            .await
            .unwrap(),
        ChatCheckpointOutcome::NotEligible
    );
    assert_eq!(host(&claimed).await.revision, held.revision);

    let paused = fixture("checkpoint-paused", 1).await;
    let mut inactive = host(&paused).await;
    inactive.status = ChatHostStatus::Paused;
    let inactive = paused
        .hosts
        .update_if_revision(inactive.revision, &inactive)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        paused
            .service
            .checkpoint_chat_host(&inactive.id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::NotEligible
    );
    assert_eq!(host(&paused).await.revision, inactive.revision);
}

#[tokio::test]
async fn local_host_or_input_contention_is_skipped_without_waiting_or_claiming() {
    let fixture = fixture("checkpoint-local-contention", 1).await;
    use_package(&fixture, 1, "local-contention").await;
    let before = host(&fixture).await;

    let owner_guard = fixture.service.chat_host_guards.lock(&before.owner).await;
    let outcome = tokio::time::timeout(
        Duration::from_millis(250),
        fixture.service.checkpoint_chat_host(&before.id, CAPTURE_AT),
    )
    .await
    .expect("checkpoint admission must not wait behind a local host mutation")
    .unwrap();
    assert_eq!(outcome, ChatCheckpointOutcome::Busy);
    drop(owner_guard);

    let input_guard = fixture
        .service
        .input_gates
        .gate(&fixture.threads[0].id)
        .read_owned()
        .await;
    let outcome = tokio::time::timeout(
        Duration::from_millis(250),
        fixture.service.checkpoint_chat_host(&before.id, CAPTURE_AT),
    )
    .await
    .expect("checkpoint admission must not wait behind accepted input")
    .unwrap();
    assert_eq!(outcome, ChatCheckpointOutcome::Busy);
    drop(input_guard);

    assert_eq!(host(&fixture).await.revision, before.revision);
    assert_eq!(fixture.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn checkpoint_age_is_due_at_five_minutes_but_not_one_second_before() {
    let fixture = fixture("checkpoint-age", 1).await;
    let id = host(&fixture).await.id;
    use_package(&fixture, 1, "first").await;
    assert!(matches!(
        fixture
            .service
            .checkpoint_chat_host(&id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::Complete(_)
    ));
    let after_first = host(&fixture).await;
    let calls = fixture.adapter.checkpoint_calls.load(Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .checkpoint_chat_host(&id, "2026-09-01T02:04:59Z")
            .await
            .unwrap(),
        ChatCheckpointOutcome::NotEligible
    );
    assert_eq!(host(&fixture).await.revision, after_first.revision);
    assert_eq!(
        fixture.adapter.checkpoint_calls.load(Ordering::SeqCst),
        calls
    );

    use_package(&fixture, 2, "second").await;
    assert!(matches!(
        fixture
            .service
            .checkpoint_chat_host(&id, FIVE_MINUTES_LATER)
            .await
            .unwrap(),
        ChatCheckpointOutcome::Complete(ChatCheckpointRef {
            checkpoint_version: 2,
            ..
        })
    ));
}

#[tokio::test]
async fn sweep_reports_completed_and_skipped_active_candidates() {
    let fixture = fixture("checkpoint-sweep-quiet", 1).await;
    let second_caller = user("checkpoint-sweep-running");
    let second = fixture
        .service
        .create_session(&second_caller, chat_spec())
        .await
        .unwrap();
    fixture.adapter.conversation_snapshots.lock().await.insert(
        format!("conversation-for-{}", second.id),
        b"second-owner-snapshot".to_vec(),
    );
    fixture
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(&second.id.0)
        .unwrap()
        .session
        .status = SessionStatus::Running;
    use_package(&fixture, 1, "sweep").await;

    assert_eq!(
        fixture
            .service
            .checkpoint_due_chat_hosts(CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointSweep {
            candidates: 2,
            completed: 1,
            skipped: 1,
            refused: 0,
        }
    );
    let hosts = fixture.hosts.records().await;
    assert_eq!(
        hosts
            .iter()
            .filter(|host| host.checkpoint.is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn capture_storage_and_resume_failures_preserve_the_active_host_and_add_a_note() {
    let missing = fixture("checkpoint-missing-conversation", 1).await;
    use_package(&missing, 1, "missing").await;
    missing.adapter.conversation_snapshots.lock().await.clear();
    let before = host(&missing).await;
    assert_eq!(
        missing
            .service
            .checkpoint_chat_host(&before.id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::Refused
    );
    assert_refused_without_state_loss(&before, &host(&missing).await, CAPTURE_AT);
    assert_eq!(missing.adapter.workspace_resumes.lock().await.len(), 1);

    let tampered = fixture("checkpoint-tampered-readback", 1).await;
    use_package(&tampered, 1, "tampered").await;
    tampered
        .checkpoints
        .tamper_next_read
        .store(true, Ordering::SeqCst);
    let before = host(&tampered).await;
    assert_eq!(
        tampered
            .service
            .checkpoint_chat_host(&before.id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::Refused
    );
    assert_refused_without_state_loss(&before, &host(&tampered).await, CAPTURE_AT);

    let resume = fixture("checkpoint-resume-refusal", 1).await;
    use_package(&resume, 1, "resume").await;
    resume
        .adapter
        .fail_workspace_resumes
        .store(1, Ordering::SeqCst);
    let before = host(&resume).await;
    assert_eq!(
        resume
            .service
            .checkpoint_chat_host(&before.id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::Refused
    );
    assert_refused_without_state_loss(&before, &host(&resume).await, CAPTURE_AT);
}

fn assert_refused_without_state_loss(
    before: &ChatHostRecord,
    after: &ChatHostRecord,
    observed_at: &str,
) {
    assert_eq!(after.status, before.status);
    assert_eq!(after.runtime_id, before.runtime_id);
    assert_eq!(after.runtime_version, before.runtime_version);
    assert_eq!(after.workspace_authority, before.workspace_authority);
    assert_eq!(after.checkpoint, before.checkpoint);
    assert_eq!(after.last_transition_claim, None);
    assert_eq!(after.transition_started_at, None);
    let note = after
        .last_operator_note
        .as_ref()
        .expect("checkpoint refusal is visible to operators");
    assert_eq!(note.code, "checkpoint_refused");
    assert_eq!(note.observed_at, observed_at);
    assert!(!note.detail.is_empty());
}

#[tokio::test]
async fn publication_failure_keeps_the_old_checkpoint_and_both_object_generations() {
    let fixture = fixture("checkpoint-publication-failure", 1).await;
    let id = host(&fixture).await.id;
    use_package(&fixture, 1, "first").await;
    let first = match fixture
        .service
        .checkpoint_chat_host(&id, CAPTURE_AT)
        .await
        .unwrap()
    {
        ChatCheckpointOutcome::Complete(reference) => reference,
        outcome => panic!("first checkpoint did not complete: {outcome:?}"),
    };
    fixture.adapter.conversation_snapshots.lock().await.insert(
        format!("conversation-for-{}", fixture.threads[0].id),
        b"replacement snapshot".to_vec(),
    );
    use_package(&fixture, 2, "second").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let checkpoint_id = id.clone();
    let attempt = tokio::spawn(async move {
        service
            .checkpoint_chat_host(&checkpoint_id, FIVE_MINUTES_LATER)
            .await
    });
    wait_for_checkpoint_fence(&fixture.adapter).await;
    fixture.hosts.fail_updates.store(1, Ordering::SeqCst);
    fixture.adapter.continue_checkpoint.add_permits(1);
    assert_eq!(
        attempt.await.unwrap().unwrap(),
        ChatCheckpointOutcome::Refused
    );

    let retained = host(&fixture).await;
    assert_refused_without_state_loss(
        &ChatHostRecord {
            last_operator_note: None,
            ..retained.clone()
        },
        &retained,
        FIVE_MINUTES_LATER,
    );
    assert_eq!(retained.checkpoint, Some(first.clone()));
    assert_eq!(
        fixture
            .checkpoints
            .read_workspace(
                &id,
                &first.workspace_object_reference,
                &first.workspace_archive_sha256,
            )
            .await
            .unwrap(),
        Some(b"complete workspace generation first".to_vec())
    );
    assert_eq!(
        fixture
            .checkpoints
            .generation_count(&first.workspace_object_reference)
            .await,
        2,
        "post-commit pruning cannot run before host CAS succeeds"
    );
    for conversation in &first.conversations {
        assert_eq!(
            fixture
                .checkpoints
                .generation_count(&conversation.object_reference)
                .await,
            2
        );
    }

    fixture.adapter.conversation_snapshots.lock().await.insert(
        format!("conversation-for-{}", fixture.threads[0].id),
        b"third snapshot after failed publication".to_vec(),
    );
    use_package(&fixture, 3, "third-after-failure").await;
    let recovered = fixture
        .service
        .checkpoint_chat_host(&id, "2026-09-01T02:10:00Z")
        .await
        .unwrap();
    assert!(matches!(
        recovered,
        ChatCheckpointOutcome::Complete(ChatCheckpointRef {
            checkpoint_version: 3,
            ..
        })
    ));
    assert_eq!(host(&fixture).await.checkpoint_sequence, 3);
}

#[tokio::test]
async fn concurrent_workers_publish_only_one_checkpoint_version() {
    let fixture = fixture("checkpoint-concurrent", 1).await;
    let id = host(&fixture).await.id;
    use_package(&fixture, 1, "concurrent").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let first_service = Arc::clone(&fixture.service);
    let first_id = id.clone();
    let first = tokio::spawn(async move {
        first_service
            .checkpoint_chat_host(&first_id, CAPTURE_AT)
            .await
    });
    wait_for_checkpoint_fence(&fixture.adapter).await;

    let second_service = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    assert_eq!(
        second_service
            .checkpoint_chat_host(&id, CAPTURE_AT)
            .await
            .unwrap(),
        ChatCheckpointOutcome::NotEligible
    );
    fixture.adapter.continue_checkpoint.add_permits(1);
    assert!(matches!(
        first.await.unwrap().unwrap(),
        ChatCheckpointOutcome::Complete(ChatCheckpointRef {
            checkpoint_version: 1,
            ..
        })
    ));
    assert_eq!(fixture.adapter.checkpoint_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        host(&fixture).await.checkpoint.unwrap().checkpoint_version,
        1
    );
}

#[tokio::test]
async fn checkpoint_claim_fences_new_prompt_admission_until_capture_settles() {
    let fixture = fixture("checkpoint-input-fence", 1).await;
    let id = host(&fixture).await.id;
    use_package(&fixture, 1, "fence").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let checkpoint_id = id.clone();
    let attempt = tokio::spawn(async move {
        service
            .checkpoint_chat_host(&checkpoint_id, CAPTURE_AT)
            .await
    });
    wait_for_checkpoint_fence(&fixture.adapter).await;

    let prompt = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.service.send_prompt(
            &fixture.caller,
            &fixture.threads[0].id,
            "must not cross the checkpoint fence".into(),
        ),
    )
    .await
    .expect("the claim produces a bounded admission refusal")
    .expect_err("a checkpoint claim refuses new input");
    assert!(matches!(
        prompt,
        CoreError::ChatUnavailable(ChatUnavailableCause::Provisioning)
    ));
    assert!(fixture
        .adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .unwrap()
        .sent()
        .await
        .is_empty());

    fixture.adapter.continue_checkpoint.add_permits(1);
    assert!(matches!(
        attempt.await.unwrap().unwrap(),
        ChatCheckpointOutcome::Complete(_)
    ));
}

#[tokio::test]
async fn invalid_sweep_timestamp_fails_before_reading_candidates() {
    let fixture = fixture("checkpoint-invalid-time", 1).await;
    assert!(matches!(
        fixture
            .service
            .checkpoint_due_chat_hosts("not-a-time")
            .await,
        Err(CoreError::InvalidRequest(_))
    ));
    assert_eq!(fixture.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);
}
