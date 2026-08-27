//! Ways of assembling a service for a test.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit, on the division that was already there: that file says what stands
//! in for a dependency, and this says how the service under test is wired out
//! of those stand-ins.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::KernelAdapter;

use crate::ids::KernelId;
use crate::service::{Deployment, ScreenFactory, SessionService};

use crate::ids::SessionId;
use crate::test_support::{
    fixture_skill_ports, kernel, no_environments, unconfigured_settings, FakeAdapter, FakeStore,
    FixedCredential, NullScreen, RecordingScreen,
};

/// A fixed settings document for a session-service fixture.
struct StaticSettingsStore(crate::PlatformSettings);

#[async_trait::async_trait]
impl crate::store::PlatformSettingsStore for StaticSettingsStore {
    async fn load(&self) -> Result<Option<crate::StoredSettings>, crate::store::StoreError> {
        Ok(Some(crate::StoredSettings {
            settings: self.0.clone(),
            revision: 1,
            previous: None,
        }))
    }

    async fn save_if_unchanged(
        &self,
        _expected: u64,
        _settings: &crate::PlatformSettings,
    ) -> Result<Option<crate::StoredSettings>, crate::store::StoreError> {
        Err(crate::store::StoreError::Unavailable(
            "this session fixture exposes a fixed settings document".into(),
        ))
    }
}

/// A service over profiles and a concrete settings document.
pub(crate) fn service_with_environments_and_settings(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    environments: Arc<crate::environment_service::EnvironmentProfileService>,
    settings: crate::PlatformSettings,
) -> Arc<SessionService> {
    service_with_environments_and_settings_service(
        store,
        adapter,
        environments,
        Arc::new(crate::PlatformSettingsService::new(Arc::new(
            StaticSettingsStore(settings),
        ))),
    )
}

/// The same fixture over a settings service whose failure modes a test owns.
pub(crate) fn service_with_environments_and_settings_service(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    environments: Arc<crate::environment_service::EnvironmentProfileService>,
    settings: Arc<crate::PlatformSettingsService>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    let (skills, skill_packs) = fixture_skill_ports(&store);
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        Arc::clone(&store) as Arc<dyn crate::store::SessionStore>,
        crate::service::SessionPorts {
            context: None,
            handovers: Some(handovers),
            credentials: None,
            skills: Some(skills),
            skill_packs: Some(skill_packs),
            settings: Some(settings),
        },
        environments,
        owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that stores transcripts, over a screen model that keeps them.
pub(crate) fn service_with_context(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    context: Arc<dyn crate::store::ContextStore>,
) -> Arc<SessionService> {
    service_over_with_context(store, adapter, context)
}

/// A transcript-capable service over an adversarial session store.
pub(crate) fn service_over_with_context(
    store: Arc<dyn crate::store::SessionStore>,
    adapter: Arc<FakeAdapter>,
    context: Arc<dyn crate::store::ContextStore>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: Some(context),
            handovers: Some(handovers),
            credentials: None,
            skills: None,
            skill_packs: None,
            settings: None,
        },
        no_environments(),
        owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that expects a particular runtime version.
///
/// Two of these over one store is an upgrade: the second expects a version the
/// first's sessions are not on.
/// The same, with a source of skills and a baseline that mandates one.
///
/// Separate because most tests want neither: a baseline naming a skill makes
/// every session in that test fetch one, which is the point here and noise
/// everywhere else.
pub(crate) fn service_on_version_with_skills(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
    skills: Arc<dyn crate::store::SkillSource>,
) -> Arc<SessionService> {
    service_on_version_with_skill_ports(store, adapter, version, skills, None)
}

/// The skill fixture with durable pack custody enabled.
pub(crate) fn service_on_version_with_skill_store(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
    skills: Arc<dyn crate::store::SkillSource>,
    skill_packs: Arc<dyn crate::store::SkillPackStore>,
) -> Arc<SessionService> {
    service_on_version_with_skill_ports(store, adapter, version, skills, Some(skill_packs))
}

fn service_on_version_with_skill_ports(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
    skills: Arc<dyn crate::store::SkillSource>,
    skill_packs: Option<Arc<dyn crate::store::SkillPackStore>>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(kernel(), adapter);
    let environments = crate::environment_service::EnvironmentProfileService::new(
        Arc::new(crate::test_support::FakeProfileStore::default()),
        crate::environment::Baseline::new(
            [],
            [crate::environment::SkillId::new("a-mandated-skill")],
            [],
        ),
        None,
        None,
        crate::environment::AgentConfiguration {
            kernel: kernel(),
            ..Default::default()
        },
        unconfigured_settings(),
    );
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            skills: Some(skills),
            handovers: Some(handovers),
            skill_packs,
            ..Default::default()
        },
        environments,
        Deployment {
            runtime_version: Some(version.to_string()),
            ..owned()
        },
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

pub(crate) fn service_on_version(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            handovers: Some(handovers),
            credentials: None,
            skills: None,
            skill_packs: None,
            settings: None,
        },
        no_environments(),
        Deployment {
            runtime_version: Some(version.to_string()),
            ..owned()
        },
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A versioned service measured against a concrete settings document.
pub(crate) fn service_on_version_with_settings(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
    settings: crate::PlatformSettings,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(kernel(), adapter);
    let settings = Arc::new(crate::PlatformSettingsService::new(Arc::new(
        StaticSettingsStore(settings),
    )));
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            settings: Some(settings),
            handovers: Some(handovers),
            ..Default::default()
        },
        no_environments(),
        Deployment {
            runtime_version: Some(version.to_string()),
            ..owned()
        },
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A versioned service over a particular set of environment profiles.
///
/// Used where an upgrade must prove it carries a choice frozen from a profile
/// rather than resolving that profile again on the replacement runtime.
pub(crate) fn service_with_environments_on_version(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    environments: Arc<crate::environment_service::EnvironmentProfileService>,
    version: &str,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    let (skills, skill_packs) = fixture_skill_ports(&store);
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        Arc::clone(&store) as Arc<dyn crate::store::SessionStore>,
        crate::service::SessionPorts {
            handovers: Some(handovers),
            skills: Some(skills),
            skill_packs: Some(skill_packs),
            ..Default::default()
        },
        environments,
        Deployment {
            runtime_version: Some(version.to_string()),
            ..owned()
        },
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that can supply an owner's credential without the owner present.
pub(crate) fn service_with_credentials(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    credential: &str,
) -> Arc<SessionService> {
    service_with_credential_source(
        store,
        adapter,
        Arc::new(FixedCredential(credential.to_string())),
    )
}

pub(crate) fn service_with_credential_source(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    credentials: Arc<dyn crate::store::CredentialSource>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            handovers: Some(handovers),
            credentials: Some(credentials),
            skills: None,
            skill_packs: None,
            settings: None,
        },
        no_environments(),
        owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// Put bytes on a session's screen, as its runtime would.
pub(crate) async fn write_screen(svc: &Arc<SessionService>, id: &SessionId, bytes: &[u8]) {
    let entries = svc.entries.read().await;
    let entry = entries.get(id).expect("the session is live");
    entry.screen.lock().await.apply(bytes);
}

/// The credential a live session is currently holding.
pub(crate) async fn credential_on(svc: &Arc<SessionService>, id: &SessionId) -> Option<String> {
    svc.entries
        .read()
        .await
        .get(id)
        .and_then(|e| e.subject_credential.clone())
}

/// A service identifying itself as a particular deployment.
pub(crate) fn service_as(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    deployment: Deployment,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            handovers: Some(handovers),
            credentials: None,
            skills: None,
            skill_packs: None,
            settings: None,
        },
        no_environments(),
        deployment,
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A deployment that owns its provider, which is what most tests are about.
pub(crate) fn owned() -> Deployment {
    Deployment {
        adopts_untagged: true,
        ..Deployment::default()
    }
}
