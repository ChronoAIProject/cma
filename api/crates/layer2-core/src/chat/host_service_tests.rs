use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    ContentBlock, ConversationInput, KernelAdapter, KernelCapabilities, RuntimePermission,
};
use tokio::sync::Mutex;

use super::*;
use crate::conversation_support::until_sent;
use crate::store::{
    ChatHostStore, Page, PageCursor, PageRequest, PlatformSettingsStore, SessionStore, StoreError,
};
use crate::test_support::{
    fixture_skill_ports, spec, user, FakeAdapter, FakeProfileStore, FakeStore, FixedCredential,
    NullScreen,
};
use crate::{
    AgentConfiguration, Authorization, Baseline, Capability, CoreError, EnvironmentProfileService,
    KernelId, PlatformSettings, PlatformSettingsService, ServiceId, SessionBriefing, SkillId,
    SoftwarePackage, StoredSettings, SubjectId,
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
    sessions: Arc<FakeStore>,
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
    let service = host_service(
        Arc::clone(&settings),
        Arc::clone(&adapter),
        Arc::clone(&hosts),
        Arc::clone(&sessions),
        environments,
        compose_host_store,
    );
    HostFixture {
        service,
        adapter,
        hosts,
        sessions,
        settings,
    }
}

fn host_service(
    settings: Arc<MutableSettings>,
    adapter: Arc<FakeAdapter>,
    hosts: Arc<MemoryChatHosts>,
    sessions: Arc<FakeStore>,
    environments: Arc<EnvironmentProfileService>,
    compose_host_store: bool,
) -> Arc<crate::SessionService> {
    let settings_service = Arc::new(PlatformSettingsService::new(
        settings as Arc<dyn PlatformSettingsStore>,
    ));
    let (skills, _) = fixture_skill_ports(&sessions);
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("fake"), adapter.clone());
    let screen: crate::ScreenFactory = Arc::new(|geometry| Box::new(NullScreen(geometry)));
    crate::SessionService::new(
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
    )
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

fn chat_spec() -> crate::SessionSpec {
    crate::SessionSpec {
        surface: Some(crate::SessionSurface::Chat),
        ..spec()
    }
}

fn operator() -> crate::Caller {
    crate::Caller::new(
        SubjectId("chat-operator".into()),
        Authorization::from_capabilities([Capability::ControlAnySession]),
    )
}

fn prompt_text(input: &ConversationInput) -> Option<&str> {
    let ConversationInput::Prompt(content) = input else {
        return None;
    };
    match content.as_slice() {
        [ContentBlock::Text(text)] => Some(text.text.as_str()),
        _ => None,
    }
}

#[tokio::test]
async fn chat_prompt_composition_reaches_runtime_without_rewriting_owner_history() {
    let default_prefixes = ChatSettings::default().message_prefixes;
    let default_expected = super::threads::compose_chat_message(&default_prefixes, "original");
    let cases = [
        ("default", default_prefixes, "original", default_expected),
        (
            "multiple-multiline",
            vec!["a".into(), "b".into()],
            "first\nsecond",
            "a b first\nsecond".into(),
        ),
        ("empty", Vec::new(), "unchanged", "unchanged".into()),
    ];

    for (name, prefixes, original, expected) in cases {
        let mut chat = enabled_chat();
        chat.message_prefixes = prefixes;
        let fixture = fixture(
            platform_with(Some(chat)),
            chat_capabilities(),
            Baseline::default(),
        );
        let caller = user(&format!("owner-prefix-{name}"));
        let session = fixture
            .service
            .create_session(&caller, chat_spec())
            .await
            .expect("the chat thread is created");

        fixture
            .service
            .send_prompt(&caller, &session.id, original.into())
            .await
            .expect("the chat prompt is delivered");

        let sent = until_sent(&fixture.adapter, |input| {
            prompt_text(input) == Some(expected.as_str())
        })
        .await;
        assert_eq!(
            sent.iter().filter_map(prompt_text).collect::<Vec<_>>(),
            [expected.as_str()],
            "the {name} case delivers exactly the effective text"
        );
        let history = fixture
            .service
            .events_since(&caller, &session.id, None)
            .await
            .expect("chat history is readable");
        assert_eq!(
            history
                .iter()
                .filter_map(|event| match &event.kind {
                    crate::EventKind::OwnerMessage { content } => Some(content.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [original],
            "the {name} case retains the owner's exact text"
        );
    }
}

#[tokio::test]
async fn chat_prompt_and_steer_keep_the_thread_hosts_pinned_prefix() {
    let mut chat = enabled_chat();
    chat.message_prefixes = vec!["pinned".into(), "prefix".into()];
    let mut capabilities = chat_capabilities();
    capabilities.steer = true;
    let fixture = fixture(platform_with(Some(chat)), capabilities, Baseline::default());
    let caller = user("owner-pinned-prefix");
    let session = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .expect("the chat thread is created");

    let mut changed = enabled_chat();
    changed.message_prefixes = vec!["later".into()];
    fixture.settings.replace(platform_with(Some(changed))).await;

    fixture
        .service
        .send_prompt(&caller, &session.id, "initial text".into())
        .await
        .expect("the first chat prompt is delivered");
    fixture
        .service
        .send_prompt(&caller, &session.id, "steer text".into())
        .await
        .expect("the mid-turn steer is delivered");

    let sent = until_sent(&fixture.adapter, |input| {
        prompt_text(input) == Some("pinned prefix steer text")
    })
    .await;
    assert_eq!(
        sent.iter().filter_map(prompt_text).collect::<Vec<_>>(),
        ["pinned prefix initial text", "pinned prefix steer text"]
    );
    let history = fixture
        .service
        .events_since(&caller, &session.id, None)
        .await
        .expect("chat history is readable");
    assert_eq!(
        history
            .iter()
            .filter_map(|event| match &event.kind {
                crate::EventKind::OwnerMessage { content } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["initial text", "steer text"]
    );
}

#[test]
fn composing_chat_messages_keeps_exact_spacing_and_empty_text() {
    assert_eq!(
        super::threads::compose_chat_message(&["a".into(), "b".into()], ""),
        "a b "
    );
    assert_eq!(
        super::threads::compose_chat_message(&[], "first\nsecond"),
        "first\nsecond"
    );
}

#[tokio::test]
async fn chat_threads_share_one_host_and_pin_each_worktree_and_conversation() {
    let mut chat = enabled_chat();
    chat.host.max_threads = 3;
    chat.workspace.branch = "release-chat".into();
    chat.agent.model = Some("chat-model".into());
    chat.agent.reasoning_effort = crate::ReasoningEffort::Medium;
    chat.agent.record_tool_bodies = true;
    let fixture = fixture(
        platform_with(Some(chat)),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-threads");

    let first = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .expect("the first chat thread is created");
    let second = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .expect("the second chat thread reuses the host");

    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
    assert_eq!(fixture.hosts.creates.load(Ordering::SeqCst), 1);
    assert_eq!(first.chat.as_ref().unwrap().worktree_name, "chat-1");
    assert_eq!(
        first.chat.as_ref().unwrap().worktree_path,
        "/workspace/chat-1"
    );
    assert_eq!(second.chat.as_ref().unwrap().worktree_name, "chat-2");
    assert_eq!(
        second.chat.as_ref().unwrap().worktree_path,
        "/workspace/chat-2"
    );
    for session in [&first, &second] {
        assert_eq!(session.options.mode, crate::SessionMode::Conversation);
        assert_eq!(session.options.model.as_deref(), Some("chat-model"));
        assert_eq!(
            session.options.permission,
            crate::PermissionMode::Unrestricted
        );
        assert_eq!(session.options.credential, crate::CredentialMode::Delegated);
        assert_eq!(
            session.options.reasoning_effort,
            crate::ReasoningEffort::Medium
        );
        assert!(session.options.record_tool_bodies);
        assert!(session.capabilities.conversation);
        assert!(!session.capabilities.terminal);
        assert!(session.environment_profile.is_none());
        assert!(session.pricing.is_none());
    }

    let trees = fixture.adapter.workspace_trees.lock().await.clone();
    assert_eq!(trees.len(), 2);
    assert_eq!(trees[0].1.name, "chat-1");
    assert_eq!(trees[0].1.base, "release-chat");
    assert_eq!(trees[1].1.name, "chat-2");
    assert_eq!(trees[1].1.base, "release-chat");
    assert_eq!(trees[0].0, trees[1].0, "both trees use one runtime");

    let options = fixture.adapter.conversation_options.lock().await.clone();
    assert_eq!(options.len(), 2);
    assert_eq!(
        options[0].working_directory.as_deref(),
        Some("/workspace/chat-1")
    );
    assert_eq!(
        options[1].working_directory.as_deref(),
        Some("/workspace/chat-2")
    );
    assert_eq!(
        options[0].conversation_slot.as_deref(),
        Some(first.id.0.as_str())
    );
    assert_eq!(
        options[1].conversation_slot.as_deref(),
        Some(second.id.0.as_str())
    );
    assert!(options
        .iter()
        .all(|options| options.resume_conversation.is_none()));

    let records = fixture.sessions.sessions.lock().await;
    for session in [&first, &second] {
        let record = records.get(&session.id.0).expect("chat origin is durable");
        let origin = record.chat_origin.as_ref().expect("private origin exists");
        assert_eq!(record.runtime_id.as_deref(), Some(trees[0].0.as_str()));
        assert_eq!(
            origin.worktree_name,
            session.chat.as_ref().unwrap().worktree_name
        );
        assert_eq!(
            origin.worktree_path,
            session.chat.as_ref().unwrap().worktree_path
        );
    }
    drop(records);

    let all = fixture
        .service
        .list_sessions(&caller, &crate::SessionListQuery::default())
        .await
        .unwrap();
    let chats = fixture
        .service
        .list_sessions(
            &caller,
            &crate::SessionListQuery::default()
                .with_surface(Some(crate::SessionSurfaceFilter::Chat)),
        )
        .await
        .unwrap();
    let agents = fixture
        .service
        .list_sessions(
            &caller,
            &crate::SessionListQuery::default()
                .with_surface(Some(crate::SessionSurfaceFilter::Agent)),
        )
        .await
        .unwrap();
    assert_eq!(all.items.len(), 2);
    assert_eq!(chats.items.len(), 2);
    assert!(agents.items.is_empty());
    assert!(chats.items.iter().all(|session| session.chat.is_some()));
    assert_eq!(
        fixture
            .service
            .session_allowance(&caller)
            .await
            .unwrap()
            .live,
        0
    );
    assert_eq!(
        fixture.sessions.reserved_capacity().await.unwrap().sessions,
        0
    );
}

#[tokio::test]
async fn chat_thread_limit_refuses_before_provider_or_session_side_effects() {
    let mut chat = enabled_chat();
    chat.host.max_threads = 1;
    let fixture = fixture(
        platform_with(Some(chat)),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-thread-limit");
    fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    let trees_before = fixture.adapter.workspace_trees.lock().await.len();
    let opens_before = fixture.adapter.rejoined.lock().await.len();
    let credentials_before = fixture.adapter.credentials.lock().await.len();

    let error = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .expect_err("the second live thread exceeds the pinned host cap");

    assert_eq!(error, CoreError::ChatThreadLimit { limit: 1, live: 1 });
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.adapter.workspace_trees.lock().await.len(),
        trees_before
    );
    assert_eq!(fixture.adapter.rejoined.lock().await.len(), opens_before);
    assert_eq!(
        fixture.adapter.credentials.lock().await.len(),
        credentials_before
    );
    assert_eq!(fixture.sessions.sessions.lock().await.len(), 1);
    let host = stored_host(&fixture, &caller.subject).await;
    assert_eq!(host.threads.len(), 1);
    assert_eq!(host.threads_created, 1);
}

#[tokio::test]
async fn chat_environment_profile_is_refused_before_host_provisioning() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-chat-profile");
    let mut requested = chat_spec();
    requested.environment_profile = Some(crate::EnvironmentProfileId("envp_chat".into()));

    let error = fixture
        .service
        .create_session(&caller, requested)
        .await
        .expect_err("chat configuration is deployment-owned");

    assert!(matches!(error, CoreError::InvalidRequest(_)));
    assert!(fixture.hosts.records().await.is_empty());
    assert!(fixture.sessions.sessions.lock().await.is_empty());
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_conversation_open_rolls_back_only_its_thread_and_keeps_the_host_retryable() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    fixture
        .adapter
        .refuse_conversation_open
        .store(true, Ordering::SeqCst);
    let caller = user("owner-thread-rollback");

    let error = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .expect_err("the refused conversation does not create a chat row");

    assert!(matches!(error, CoreError::RuntimeRefused(_)));
    assert!(fixture.sessions.sessions.lock().await.is_empty());
    let host = stored_host(&fixture, &caller.subject).await;
    assert_eq!(host.status, ChatHostStatus::Active);
    assert_eq!(host.threads_created, 1);
    assert_eq!(host.threads.len(), 1);
    assert!(host.threads[0].removed);
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    assert_eq!(
        fixture
            .adapter
            .removed_workspace_trees
            .lock()
            .await
            .as_slice(),
        &[(host.runtime_id.unwrap(), "chat-1".into())]
    );

    fixture
        .adapter
        .refuse_conversation_open
        .store(false, Ordering::SeqCst);
    let retry = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .expect("a later allocation reuses the clean host");
    assert_eq!(retry.chat.unwrap().worktree_name, "chat-2");
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn owner_lifecycle_and_terminal_operations_name_chat_policy() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-chat-policy");
    let session = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();

    for error in [
        fixture
            .service
            .stop_session(&caller, &session.id)
            .await
            .unwrap_err(),
        fixture
            .service
            .terminate_session(&caller, &session.id)
            .await
            .unwrap_err(),
        fixture
            .service
            .revive_session(&caller, &session.id)
            .await
            .unwrap_err(),
        fixture
            .service
            .change_plane(&caller, &session.id, crate::SessionMode::Terminal)
            .await
            .unwrap_err(),
    ] {
        let CoreError::InvalidState {
            detail: Some(detail),
        } = error
        else {
            panic!("chat lifecycle refusal was not detailed: {error:?}");
        };
        assert!(detail.contains("chat"), "{detail}");
        assert!(detail.contains("platform"), "{detail}");
    }
    let terminal = fixture
        .service
        .attach_terminal(
            &caller,
            &session.id,
            crate::TerminalGeometry::default(),
            crate::AttachmentClaim::Exclusive,
        )
        .await
        .err()
        .expect("chat has no terminal capability");
    assert_eq!(
        terminal,
        CoreError::CapabilityUnsupported("terminal".into())
    );
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
    assert!(fixture.adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn operator_controls_only_the_thread_and_owner_delete_retires_the_last_host() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-chat-abuse-control");
    let session = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    let host_before = stored_host(&fixture, &caller.subject).await;
    let runtime = host_before.runtime_id.clone().unwrap();

    assert_eq!(
        fixture
            .service
            .stop_any_session(&operator(), &session.id)
            .await
            .unwrap(),
        caller.subject
    );
    assert_eq!(
        fixture
            .service
            .get_session(&caller, &session.id)
            .await
            .unwrap()
            .status,
        crate::SessionStatus::Idle
    );
    assert_eq!(fixture.adapter.running.lock().await.len(), 1);
    assert!(fixture.adapter.terminated.lock().await.is_empty());

    fixture
        .service
        .terminate_any_session(&operator(), &session.id)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .get_session(&caller, &session.id)
            .await
            .unwrap()
            .status,
        crate::SessionStatus::Terminated
    );
    assert!(fixture.adapter.running.lock().await.contains_key(&runtime));
    assert!(fixture.adapter.terminated.lock().await.is_empty());

    fixture
        .service
        .delete_session(&caller, &session.id)
        .await
        .expect("the owner may delete an operator-ended chat thread");
    assert!(!fixture
        .sessions
        .sessions
        .lock()
        .await
        .contains_key(&session.id.0));
    let retired = stored_host(&fixture, &caller.subject).await;
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert!(retired.threads[0].removed);
    assert!(retired.runtime_id.is_none());
    assert!(fixture.adapter.running.lock().await.is_empty());
    assert_eq!(
        fixture.adapter.terminated.lock().await.as_slice(),
        &[runtime]
    );
    assert!(
        fixture
            .adapter
            .removed_workspace_trees
            .lock()
            .await
            .is_empty(),
        "retiring the whole sandbox does not need a per-tree mutation"
    );
}

#[tokio::test]
async fn deleting_from_a_shared_host_removes_only_worktrees_that_stay_live() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-shared-chat-delete");
    let first = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    let second = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    let host = stored_host(&fixture, &caller.subject).await;
    let runtime = host.runtime_id.clone().unwrap();

    fixture
        .service
        .delete_session(&caller, &first.id)
        .await
        .expect("one thread can leave a shared host running");

    assert_eq!(
        fixture
            .adapter
            .removed_workspace_trees
            .lock()
            .await
            .as_slice(),
        &[(runtime.clone(), "chat-1".into())]
    );
    assert!(fixture.adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        stored_host(&fixture, &caller.subject).await.status,
        ChatHostStatus::Active
    );

    let mut pausing = stored_host(&fixture, &caller.subject).await;
    pausing.status = ChatHostStatus::Pausing;
    pausing.last_transition_claim = Some("chro_stuck_legacy_checkpoint".into());
    pausing.transition_started_at = Some("2026-09-02T20:04:31Z".into());
    pausing.revive_cause = Some(ChatReviveCause::ImageDrift);
    pausing.checkpoint_sequence += 1;
    fixture
        .hosts
        .update_if_revision(pausing.revision, &pausing)
        .await
        .unwrap()
        .unwrap();

    fixture
        .service
        .delete_session(&caller, &second.id)
        .await
        .expect("the last thread supersedes a stuck image-rotation claim");

    assert_eq!(
        fixture
            .adapter
            .removed_workspace_trees
            .lock()
            .await
            .as_slice(),
        &[(runtime.clone(), "chat-1".into())],
        "the final worktree disappears with its sandbox"
    );
    assert!(!fixture.adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        stored_host(&fixture, &caller.subject).await.status,
        ChatHostStatus::Retired
    );
}

#[tokio::test]
async fn failed_final_host_retirement_leaves_the_thread_row_for_cleanup_retry() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-chat-retirement-retry");
    let session = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    fixture.adapter.fail_terminations.store(1, Ordering::SeqCst);

    let error = fixture
        .service
        .delete_session(&caller, &session.id)
        .await
        .expect_err("unconfirmed host cleanup keeps the deletion retryable");
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::Cleanup,
            retryable: true,
            ..
        })
    ));
    assert!(fixture
        .sessions
        .sessions
        .lock()
        .await
        .contains_key(&session.id.0));
    let pending = stored_host(&fixture, &caller.subject).await;
    assert_eq!(pending.status, ChatHostStatus::Pausing);
    assert!(pending.threads[0].removed);
    assert!(pending.runtime_id.is_some());

    fixture
        .service
        .delete_session(&caller, &session.id)
        .await
        .expect("retry completes host retirement and row deletion");
    assert!(!fixture
        .sessions
        .sessions
        .lock()
        .await
        .contains_key(&session.id.0));
    let retired = stored_host(&fixture, &caller.subject).await;
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert!(retired.runtime_id.is_none());
}

#[tokio::test]
async fn paused_thread_delete_drops_its_restore_reference_without_provider_work() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-paused-chat-delete");
    let session = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    let mut paused = stored_host(&fixture, &caller.subject).await;
    paused.status = ChatHostStatus::Paused;
    paused.runtime_id = None;
    paused.checkpoint = Some(checkpoint_for(&session.id));
    fixture
        .hosts
        .update_if_revision(paused.revision, &paused)
        .await
        .unwrap()
        .unwrap();
    fixture.adapter.running.lock().await.clear();
    fixture.adapter.generations.lock().await.clear();
    fixture.adapter.chat_hosts.lock().await.clear();
    let removals_before = fixture.adapter.removed_workspace_trees.lock().await.len();

    fixture
        .service
        .delete_session(&caller, &session.id)
        .await
        .expect("a paused chat thread is deleted from its restore set");

    assert_eq!(
        fixture.adapter.removed_workspace_trees.lock().await.len(),
        removals_before,
        "a paused host has no live provider worktree to remove"
    );
    let retired = stored_host(&fixture, &caller.subject).await;
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert!(retired.threads[0].removed);
    assert_eq!(retired.checkpoint, None);
    assert!(!fixture
        .sessions
        .sessions
        .lock()
        .await
        .contains_key(&session.id.0));
}

#[tokio::test]
async fn restart_readopts_the_exact_chat_slot_cwd_and_conversation_id() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let caller = user("owner-chat-restart");
    let session = fixture
        .service
        .create_session(&caller, chat_spec())
        .await
        .unwrap();
    assert!(fixture.sessions.events.lock().await.iter().any(|event| {
        event.session_id == session.id
            && matches!(
                &event.kind,
                crate::StoredEventKind::Known(crate::EventKind::ConversationOpened {
                    conversation_id
                }) if conversation_id == "conv-fixture"
            )
    }));
    let runtime_calls = fixture.adapter.provision_calls.load(Ordering::SeqCst);
    let tree_calls = fixture.adapter.workspace_trees.lock().await.len();
    let restarted = host_service(
        Arc::clone(&fixture.settings),
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(fixture.service.environments()),
        true,
    );

    restarted.reconcile().await;

    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        runtime_calls
    );
    assert_eq!(
        fixture.adapter.workspace_trees.lock().await.len(),
        tree_calls
    );
    let options = fixture.adapter.conversation_options.lock().await.clone();
    let resumed = options
        .last()
        .expect("the restarted service readopts the chat");
    assert_eq!(
        resumed.working_directory.as_deref(),
        Some("/workspace/chat-1")
    );
    assert_eq!(
        resumed.conversation_slot.as_deref(),
        Some(session.id.0.as_str())
    );
    assert_eq!(resumed.resume_conversation.as_deref(), Some("conv-fixture"));
    assert_eq!(
        restarted
            .get_session(&caller, &session.id)
            .await
            .unwrap()
            .chat,
        session.chat
    );
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
async fn nonzero_setup_persists_the_bounded_failure_evidence_and_cleans_runtime() {
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
    assert_eq!(
        failure.output_excerpt.as_deref(),
        Some("bounded test failure")
    );
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
    assert!(spec
        .reservation_token
        .as_deref()
        .is_some_and(|token| token.starts_with("chcl_") && token.len() == 37));
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
    drop(sources);
    assert_eq!(
        *fixture.adapter.chat_workspace_events.lock().await,
        ["workspace_source", "workspace_setup"],
        "initial provisioning must run setup only after the repository clone"
    );
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
async fn chat_surface_capability_follows_the_live_deployment_switch() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    assert!(fixture.service.chat_surface_enabled().await);

    let mut disabled = enabled_chat();
    disabled.enabled = false;
    fixture
        .settings
        .replace(platform_with(Some(disabled)))
        .await;
    assert!(!fixture.service.chat_surface_enabled().await);

    fixture.settings.replace(platform_with(None)).await;
    assert!(!fixture.service.chat_surface_enabled().await);
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
    checkpoint_for(&crate::SessionId("sesn-checkpoint-fixture".into()))
}

fn checkpoint_for(session_id: &crate::SessionId) -> ChatCheckpointRef {
    ChatCheckpointRef::seal(
        "objects/workspace",
        &layer3_adapter::WorkspaceCheckpointPackage {
            epoch: 1,
            version: 3,
            source_revision: "d".repeat(40),
            manifest_digest: "b".repeat(64),
            archive_sha256: "a".repeat(64),
            entries: Vec::new(),
            total_bytes: 0,
            quiesced_writers: 1,
            writer_state: layer3_adapter::WorkspaceWriterState::Resumed,
            writer_resume_token: None,
            includes_git_metadata: false,
            archive: Vec::new(),
        },
        vec![crate::ChatCheckpointConversation {
            session_id: session_id.clone(),
            object_reference: format!("sessions/{session_id}/handover.snapshot"),
            conversation_id: "conv-fixture".into(),
            snapshot: b"conversation checkpoint".to_vec(),
        }],
        "2026-09-01T01:00:00Z",
    )
    .unwrap()
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
async fn credential_maintenance_recognizes_only_the_current_chat_incarnation() {
    let baseline = Baseline::new(
        std::iter::empty::<SoftwarePackage>(),
        std::iter::empty::<SkillId>(),
        [ServiceId::new("model-service")],
    );
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        baseline,
    );
    let owner = SubjectId("owner-credential-maintenance".into());
    let host = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .unwrap();
    let credential_id = host.credential_session_id();

    assert!(fixture
        .service
        .session_claims_credential(&credential_id)
        .await
        .unwrap());
    assert_eq!(
        fixture
            .service
            .session_credential_context(&credential_id)
            .await
            .unwrap(),
        Some((owner.clone(), vec![ServiceId::new("model-service")]))
    );
    assert!(!fixture
        .service
        .can_reclaim_untracked_credential(&credential_id, &["tracked"])
        .await
        .unwrap());

    let stale_id = host
        .id
        .credential_session_id(host.credential_generation.saturating_add(1));
    assert!(!fixture
        .service
        .session_claims_credential(&stale_id)
        .await
        .unwrap());
    assert_eq!(
        fixture
            .service
            .session_credential_context(&stale_id)
            .await
            .unwrap(),
        None
    );
    assert!(fixture
        .service
        .can_reclaim_untracked_credential(&stale_id, &[])
        .await
        .unwrap());

    fixture
        .service
        .retire_chat_host_if_empty(&host.id)
        .await
        .unwrap();
    assert!(!fixture
        .service
        .session_claims_credential(&credential_id)
        .await
        .unwrap());
    assert!(fixture
        .service
        .can_reclaim_untracked_credential(&credential_id, &[])
        .await
        .unwrap());
}

#[tokio::test]
async fn recurring_credential_delivery_targets_the_exact_active_chat_runtime() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let host = fixture
        .service
        .ensure_chat_host(&SubjectId("owner-chat-renewal".into()), None)
        .await
        .unwrap();
    let credential_id = host.credential_session_id();
    let expected_runtime = host.runtime_id.clone().unwrap();
    let prepare_runtime = expected_runtime.clone();
    let commit_runtime = expected_runtime.clone();

    assert!(fixture
        .service
        .push_session_credential_with(
            &credential_id,
            "renewed-chat-credential",
            move |target| async move {
                assert_eq!(target.runtime_id, prepare_runtime);
                assert_eq!(target.runtime_generation, 1);
                Ok(true)
            },
            move |target| async move {
                assert_eq!(target.runtime_id, commit_runtime);
                assert_eq!(target.runtime_generation, 1);
                Ok(true)
            },
        )
        .await
        .unwrap());
    assert_eq!(
        *fixture.adapter.credentials.lock().await,
        ["renewed-chat-credential"]
    );

    fixture
        .service
        .retire_chat_host_if_empty(&host.id)
        .await
        .unwrap();
    assert!(!fixture
        .service
        .push_session_credential_with(
            &credential_id,
            "must-not-deliver",
            |_target| async { Ok(true) },
            |_target| async { Ok(true) },
        )
        .await
        .unwrap());
    assert_eq!(fixture.adapter.credentials.lock().await.len(), 1);
}

#[tokio::test]
async fn legacy_permanent_credential_failure_moves_to_one_fresh_incarnation_once() {
    let fixture = fixture(
        platform_with(Some(enabled_chat())),
        chat_capabilities(),
        Baseline::default(),
    );
    let owner = SubjectId("owner-legacy-credential-recovery".into());
    let mut failed = ChatHostRecord::new(
        owner.clone(),
        ChatSettingsSnapshot::new(1, enabled_chat()).unwrap(),
        "Pinned platform briefing\n".into(),
        KernelId::new("fake"),
        "2026-09-02T07:39:32Z".into(),
        "claim-from-failed-process".into(),
    );
    let legacy_credential_id = failed.credential_session_id();
    failed.last_transition_claim = None;
    failed.transition_started_at = None;
    failed.last_failure = Some(ChatHostFailure {
        phase: ChatProvisioningPhase::Credential,
        command_index: None,
        output_excerpt: None,
        retryable: false,
        cleanup_pending: false,
        occurred_at: "2026-09-02T07:40:55Z".into(),
    });
    fixture.hosts.create_if_absent(&failed).await.unwrap();

    let recovered = fixture
        .service
        .ensure_chat_host(&owner, None)
        .await
        .expect("the one legacy credential failure is recovered");
    assert_eq!(recovered.status, ChatHostStatus::Active);
    assert_eq!(recovered.credential_generation, 1);
    assert_ne!(recovered.credential_session_id(), legacy_credential_id);
    assert!(recovered.credential_session_id().as_str().ends_with("_cg1"));

    let mut failed_again = recovered.clone();
    failed_again.status = ChatHostStatus::Provisioning;
    failed_again.runtime_id = None;
    failed_again.last_transition_claim = None;
    failed_again.transition_started_at = None;
    failed_again.last_failure = failed.last_failure;
    fixture
        .hosts
        .update_if_revision(recovered.revision, &failed_again)
        .await
        .unwrap()
        .unwrap();
    let calls_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);

    assert!(matches!(
        fixture.service.ensure_chat_host(&owner, None).await,
        Err(CoreError::ChatUnavailable(
            ChatUnavailableCause::ProvisioningFailed {
                phase: ChatProvisioningPhase::Credential,
                retryable: false,
                ..
            }
        ))
    ));
    let retained = stored_host(&fixture, &owner).await;
    assert_eq!(retained.credential_generation, 1);
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        calls_before
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
