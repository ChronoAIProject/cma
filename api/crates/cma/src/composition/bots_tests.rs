use super::*;
use layer2_core::bots::{BotProfileResolver, InMemoryBotStore};
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::FixedSandboxCredentialPrerequisite;
use layer2_core::{Authorization, Caller, Capability};
use layer3_adapter::SandboxCapacity;

struct NoExternalResolver;
#[async_trait::async_trait]
impl ImageResolver for NoExternalResolver {
    async fn resolve(
        &self,
        _: Option<&str>,
        _: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        panic!("system default image needs no catalog resolution")
    }
}
#[async_trait::async_trait]
impl BotProfileResolver for NoExternalResolver {
    async fn resolve(&self, _: &SubjectId, _: &CreateBot) -> Result<BotProfiles, BotError> {
        panic!("inline consumers cannot use legacy profile selection")
    }
}

fn resolver(
    store: Arc<InMemoryProfileStore>,
    capacity: SandboxCapacity,
    image_resolver: Arc<dyn ImageResolver>,
) -> Arc<InlineBotProfileResolver> {
    Arc::new(InlineBotProfileResolver {
        profiles: PrivateProfileCreationService::new(
            store,
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8000,
                min_memory_mib: 128,
                max_memory_mib: 16384,
            },
            Arc::new([] as [String; 0]),
        ),
        image_resolver,
        connections: Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        capacity,
        default_model: "gpt-6".into(),
    })
}

fn input() -> CreateBot {
    serde_json::from_value(serde_json::json!({
        "name":"Consumer", "external_bot_id":uuid::Uuid::new_v4().to_string(),
        "profile":{"id":"__inline__","revision":1},
        "inline_agent":{},
        "admission":{"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"deny"},
        "reply":{"auto_final":true,"ack_after_ms":2000}
    })).unwrap()
}

#[tokio::test]
async fn consumer_defaults_match_native_pool_and_admitted_replay_keeps_original_capacity() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemoryBotStore::default());
    let original = resolver(
        profiles.clone(),
        SandboxCapacity::default(),
        Arc::new(NoExternalResolver),
    );
    let service = BotService::new(
        store.clone(),
        Arc::new(NoExternalResolver),
        "cma_codex".into(),
    )
    .with_inline_profiles(original);
    let caller = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::ManageOwnBots]),
    );
    let request = input();
    let first = service
        .create(&caller, "creation", request.clone())
        .await
        .unwrap();
    assert_eq!(
        (
            first.profiles.sandbox.content.cpu_millis,
            first.profiles.sandbox.content.memory_mib
        ),
        (1000, 1024)
    );
    assert_eq!(first.profiles.agent.content.model.as_deref(), Some("gpt-6"));
    assert_eq!(
        first.profiles.agent.content.thinking_effort,
        ThinkingEffort::High
    );
    assert_eq!(
        first.profiles.agent.content.permission_level,
        PermissionLevel::Autonomous
    );
    assert_eq!(
        first
            .profiles
            .sandbox
            .content
            .nyxid_services
            .iter()
            .map(layer2_core::ServiceId::as_str)
            .collect::<Vec<_>>(),
        ["chrono-llm-public", "ornn-api"]
    );
    assert_eq!(first.profiles.sandbox.content.max_workspaces, 16);
    assert_eq!(first.profiles.sandbox.content.max_agents_per_workspace, 16);
    let changed = resolver(
        profiles,
        SandboxCapacity {
            cpu_millis: 2000,
            memory_mib: 4096,
        },
        Arc::new(NoExternalResolver),
    );
    let service = BotService::new(store, Arc::new(NoExternalResolver), "cma_codex".into())
        .with_inline_profiles(changed.clone());
    assert_eq!(
        service
            .create(&caller, "creation", request.clone())
            .await
            .unwrap(),
        first
    );
    // A pre-admission partial materialization has no runtime. Its retry can
    // safely receive new default capacity without conflicting with old profiles.
    let next = changed
        .resolve_inline(&caller.subject, &request, None, "creation")
        .await
        .unwrap();
    assert_ne!(next.sandbox.profile_id, first.profiles.sandbox.profile_id);
    assert_eq!(
        (
            next.sandbox.content.cpu_millis,
            next.sandbox.content.memory_mib
        ),
        (2000, 4096)
    );
    assert_eq!(
        changed
            .resolve_inline(&caller.subject, &request, None, "creation")
            .await
            .unwrap(),
        next
    );
}

struct PublishedImage(String);

#[async_trait::async_trait]
impl ImageResolver for PublishedImage {
    async fn resolve(
        &self,
        _: Option<&str>,
        _: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn retirement_refuses_fresh_bot_images_and_replays_an_admitted_pin() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemoryBotStore::default());
    let caller = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::ManageOwnBots]),
    );
    let reference = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let mut request = input();
    request.image = Some(layer2_core::bots::BotImageReference {
        id: "fixture-image".into(),
        version: Some("fixture-version".into()),
    });
    let old = BotService::new(
        store.clone(),
        Arc::new(NoExternalResolver),
        "cma_codex".into(),
    )
    .with_inline_profiles(resolver(
        profiles.clone(),
        SandboxCapacity::default(),
        Arc::new(PublishedImage(reference.clone())),
    ));
    let admitted = old
        .create(&caller, "admitted", request.clone())
        .await
        .unwrap();
    assert_eq!(
        admitted.profiles.sandbox.content.runtime_image.as_deref(),
        Some(reference.as_str())
    );

    let retired =
        crate::agent_lifecycle::ImageManagerResolver::new("http://127.0.0.1:1".into(), true)
            .unwrap();
    let new = BotService::new(store, Arc::new(NoExternalResolver), "cma_codex".into())
        .with_inline_profiles(resolver(profiles, SandboxCapacity::default(), retired));
    assert_eq!(
        new.create(&caller, "admitted", request.clone())
            .await
            .unwrap(),
        admitted
    );
    assert!(matches!(
        new.create(&caller, "fresh", request).await,
        Err(BotError::ImageSelectionUnsupported)
    ));
}
