use super::*;
use crate::profiles::store::{
    AgentProfileStore, InMemoryProfileStore, SandboxProfileStore, WorkspaceProfileStore,
};
use crate::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use crate::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use crate::sandbox::*;
use crate::{Authorization, Caller, Capability, CoreError, SandboxId, ServiceId, SubjectId};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

struct Registry;
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Err(AgentRuntimeError::NotFound)
    }
    async fn fence(&self, _: &crate::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}
struct Custody {
    store: Arc<InMemorySandboxStore>,
    keys: Mutex<HashMap<SandboxId, String>>,
    values: Mutex<HashMap<SandboxId, ProvidedCredential>>,
}
#[async_trait::async_trait]
impl BindingCredentialCustody for Custody {
    fn validate(&self, key: &ProvidedCredential) -> Result<(), CoreError> {
        if key.expose().starts_with("nyx_") {
            Ok(())
        } else {
            Err(CoreError::InvalidRequest(
                "invalid test credential format".into(),
            ))
        }
    }

    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn retain_binding(
        &self,
        binding: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        binding.require_credential_admission(key)?;
        assert_eq!(
            self.store
                .binding(&binding.binding_id)
                .await
                .unwrap()
                .as_ref(),
            Some(binding)
        );
        self.keys
            .lock()
            .unwrap()
            .insert(binding.sandbox_id.clone(), key.sha256());
        self.values
            .lock()
            .unwrap()
            .insert(binding.sandbox_id.clone(), key.clone());
        Ok(())
    }
    async fn recover_binding(
        &self,
        binding: &ProviderBinding,
    ) -> Result<ProviderBinding, CoreError> {
        let key = self
            .values
            .lock()
            .unwrap()
            .get(&binding.sandbox_id)
            .cloned()
            .ok_or_else(|| CoreError::InvalidState("missing custody".into()))?;
        self.retry_recovered_binding(binding, &key).await
    }
    async fn retry_recovered_binding(
        &self,
        binding: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<ProviderBinding, CoreError> {
        if self
            .store
            .binding(&binding.binding_id)
            .await
            .unwrap()
            .as_ref()
            != Some(binding)
            || self.keys.lock().unwrap().get(&binding.sandbox_id) != Some(&key.sha256())
        {
            return Err(CoreError::InvalidState("retry authority changed".into()));
        }
        let sandbox = self
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .ok_or_else(|| CoreError::InvalidState("missing sandbox".into()))?;
        let workspace = self
            .store
            .get_workspace(&binding.workspace_id)
            .await
            .unwrap()
            .ok_or_else(|| CoreError::InvalidState("missing workspace".into()))?;
        let next =
            binding.retry_recovered_hierarchy(&sandbox, &workspace, key, crate::now_rfc3339())?;
        self.store
            .save_binding(Some(binding.revision), next.clone())
            .await
            .map_err(|_| CoreError::InvalidState("retry changed".into()))?;
        Ok(next)
    }
    async fn forget_binding(
        &self,
        binding: &ProviderBinding,
        id: &SandboxId,
    ) -> Result<(), CoreError> {
        binding.require_credential_retirement(id)?;
        self.keys.lock().unwrap().remove(id);
        self.values.lock().unwrap().remove(id);
        Ok(())
    }
}
struct Fixture {
    service: Arc<BindingService>,
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<InMemoryProfileStore>,
    custody: Arc<Custody>,
    caller: Caller,
    key: ProvidedCredential,
}
fn profile(prompt: &str) -> serde_json::Value {
    json!({ "schema_version":1,
        "metadata":{"name":"binding-service", "owner":{"kind":"human", "subject":"owner"}, "provider":"registered"},
        "model":{"access":"managed", "service":AgentProvider::default().connection_services().managed,
            "api_type":"supported", "model":"test", "thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"}, "context":{"system_prompt":prompt},
        "environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}
    })
}
fn fixture() -> Fixture {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let custody = Arc::new(Custody {
        store: store.clone(),
        keys: Mutex::new(HashMap::new()),
        values: Mutex::new(HashMap::new()),
    });
    let policy = SandboxPolicy {
        baseline_services: vec![
            ServiceId::new("baseline"),
            ServiceId::new(AgentProvider::default().connection_services().managed),
        ],
        ..Default::default()
    };
    // A binding must not require an unrelated interactive owner's refresh token.
    let prerequisite = Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired);
    let resources = BindingResources {
        sandboxes: SandboxService::new(
            store.clone(),
            profiles.clone(),
            policy.clone(),
            prerequisite.clone(),
        ),
        workspaces: WorkspaceService::new(store.clone(), profiles.clone())
            .with_connections(prerequisite.clone()),
        agents: AgentService::new(
            store.clone(),
            profiles.clone(),
            Arc::new(Registry),
            crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]),
        )
        .with_connections(prerequisite),
    };
    let service = BindingService::new(
        store.clone(),
        resources,
        custody.clone(),
        BindingPolicy {
            provider: AgentProvider::default(),
            supported_model_api: "supported".into(),
            bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            sandbox: Arc::new(policy),
        },
    );
    Fixture {
        service,
        store,
        profiles,
        custody,
        caller: Caller::new(
            SubjectId::new("manager"),
            Authorization::new([
                Capability::ManageGatewayRecords,
                Capability::ManageProviderBindings,
            ]),
        ),
        key: ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap(),
    }
}
impl Fixture {
    async fn put(&self, id: &str, prompt: &str) -> ProviderBinding {
        let profile = profile(prompt);
        let digest = format!("sha256:{}", content_digest(&profile).unwrap());
        self.service
            .put(&self.caller, id, profile, &digest, &self.key)
            .await
            .unwrap()
    }
    async fn binding(&self) -> ProviderBinding {
        self.service
            .read(&self.caller, "binding-one")
            .await
            .unwrap()
    }
    async fn ready(&self) -> ProviderBinding {
        self.ready_with_tool_endpoint(None).await
    }
    async fn ready_with_tool_endpoint(&self, endpoint: Option<String>) -> ProviderBinding {
        self.ready_with_profile(profile("Original instruction"), endpoint)
            .await
    }
    async fn ready_with_profile(
        &self,
        profile: serde_json::Value,
        endpoint: Option<String>,
    ) -> ProviderBinding {
        let digest = format!("sha256:{}", content_digest(&profile).unwrap());
        let binding = self
            .service
            .put_with_tool_endpoint(
                &self.caller,
                "binding-one",
                profile,
                &digest,
                &self.key,
                endpoint,
            )
            .await
            .unwrap();
        self.service.reconcile_binding(&binding).await.unwrap();
        let mut sandbox = self
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = sandbox.revision;
        sandbox.status = SandboxStatus::Active;
        sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, crate::now_rfc3339());
        sandbox.credential.key_generation = 1;
        sandbox.credential.delivered = true;
        sandbox.credential.current = Some(SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: format!("provided-{}", binding.key_sha256),
            provider_state_version: 1,
            issued_at: crate::now_rfc3339(),
            expires_at: String::new(),
            grant_fingerprint: content_digest(&sandbox.credential.reaches).unwrap(),
        });
        sandbox.revision += 1;
        self.store.save_sandbox(revision, sandbox).await.unwrap();
        self.service.reconcile_binding(&binding).await.unwrap();
        let binding = self.binding().await;
        self.service.reconcile_binding(&binding).await.unwrap();
        let mut workspace = self
            .store
            .get_workspace(&binding.workspace_id)
            .await
            .unwrap()
            .unwrap();
        let revision = workspace.revision;
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("a".repeat(40));
        workspace.ready_at = Some(crate::now_rfc3339());
        workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, crate::now_rfc3339());
        workspace.revision += 1;
        self.store
            .save_workspace(revision, workspace)
            .await
            .unwrap();
        self.service.reconcile_binding(&binding).await.unwrap();
        let binding = self.binding().await;
        assert_eq!(binding.state, BindingState::Ready);
        binding
    }
}

mod inspection;
mod preparation;
mod readiness;
mod reset;
mod reset_eligibility;
mod retry;
mod tools;

#[tokio::test]
async fn binding_service_pins_profiles_without_catalogue_writes_and_replays_conversation_identity()
{
    let f = fixture();
    let binding = f.ready().await;
    let sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert!(sandbox
        .credential
        .reaches
        .contains(&ServiceId::new("baseline")));
    assert!(f
        .profiles
        .sandbox_profile(&sandbox.profile.id)
        .await
        .unwrap()
        .is_none());
    let pinned = f.service.profiles.sandbox(&binding).unwrap();
    assert_eq!(sandbox.profile.id, pinned.profile_id);
    assert_eq!(sandbox.profile.content, pinned.content);
    assert_eq!(pinned.published_at, binding.created_at);
    assert_eq!(pinned.revision, 1);
    let workspace = f
        .store
        .get_workspace(&binding.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert!(f
        .profiles
        .workspace_profile(&workspace.profile.id)
        .await
        .unwrap()
        .is_none());
    let pinned = f.service.profiles.workspace(&binding).unwrap();
    assert_eq!(workspace.profile.id, pinned.profile_id);
    assert_eq!(workspace.profile.content, pinned.content);
    assert_eq!(pinned.published_at, binding.created_at);
    assert_eq!(pinned.revision, 1);
    let replay = f.put("binding-one", "Original instruction").await;
    assert_eq!(replay, binding);
    let (a, b) = tokio::join!(
        f.service
            .ensure_conversation(&f.caller, "binding-one", "conversation_one"),
        f.service
            .ensure_conversation(&f.caller, "binding-one", "conversation_one")
    );
    let (a, created) = a.unwrap();
    let (b, other_created) = b.unwrap();
    assert_ne!(created, other_created);
    assert_eq!(a, b);
    f.service
        .reconcile_conversation(&binding, &a)
        .await
        .unwrap();
    f.service
        .reconcile_conversation(&binding, &a)
        .await
        .unwrap();
    let agent = f.store.get_agent(&a.agent_id).await.unwrap().unwrap();
    assert_eq!(agent.worktree_id, a.worktree_id);
    assert_eq!(agent.owner, binding.owner);
    assert!(f
        .profiles
        .agent_profile(&agent.profile.id)
        .await
        .unwrap()
        .is_none());
    let pinned = f.service.profiles.agent(&binding).unwrap();
    assert_eq!(agent.profile.id, pinned.profile_id);
    assert_eq!(agent.profile.content, pinned.content);
    assert_eq!(pinned.published_at, binding.created_at);
    assert_eq!(pinned.revision, 1);
    assert!(f
        .store
        .get_worktree(&a.worktree_id)
        .await
        .unwrap()
        .is_some());
    f.service
        .retire_conversation(&f.caller, "binding-one", "conversation_one")
        .await
        .unwrap();
    let retiring = f
        .store
        .binding_conversation("binding-one", 1, "conversation_one")
        .await
        .unwrap()
        .unwrap();
    f.service
        .reconcile_conversation(&binding, &retiring)
        .await
        .unwrap();
    let agent = f.store.get_agent(&a.agent_id).await.unwrap().unwrap();
    assert_eq!(
        agent.requested_stop_reason,
        Some(AgentStopReason::BindingRetired)
    );
    assert!(matches!(
        f.service
            .ensure_conversation(&f.caller, "binding-one", "conversation_one")
            .await,
        Err(BindingError::Conflict)
    ));
}

#[tokio::test]
async fn binding_preserves_selected_model_route_when_reserving_baseline_services() {
    let f = fixture();
    let mut value = profile("Use selected route");
    let routes = AgentProvider::default().connection_services();
    value["model"]["service"] = json!(routes.legacy_managed);
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    let binding = f
        .service
        .put(&f.caller, "legacy-route", value, &digest, &f.key)
        .await
        .unwrap();
    let services = &binding
        .profile
        .environment
        .as_ref()
        .unwrap()
        .sandbox
        .nyxid_services;
    assert!(services.contains(&ServiceId::new("baseline")));
    assert!(services.contains(&ServiceId::new(routes.legacy_managed)));
    assert!(!services.contains(&ServiceId::new(routes.managed)));
}

#[tokio::test]
async fn binding_rejects_invalid_credential_before_reserving_the_identifier() {
    let f = fixture();
    let value = profile("Instruction");
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    let key = ProvidedCredential::new("unsupported-format".into()).unwrap();
    assert!(matches!(
        f.service
            .put(&f.caller, "binding-one", value, &digest, &key)
            .await,
        Err(BindingError::InvalidRequest(_))
    ));
    assert!(f.store.binding("binding-one").await.unwrap().is_none());
    assert!(f.custody.keys.lock().unwrap().is_empty());
}

#[tokio::test]
async fn binding_rejects_missing_environment_before_reserving_or_retaining_credentials() {
    let f = fixture();
    for remove in [true, false] {
        let mut value = profile("Instruction");
        if remove {
            value.as_object_mut().unwrap().remove("environment");
        } else {
            value["environment"] = serde_json::Value::Null;
        }
        let digest = format!("sha256:{}", content_digest(&value).unwrap());
        assert!(matches!(
            f.service
                .put(&f.caller, "binding-one", value, &digest, &f.key)
                .await,
            Err(BindingError::InvalidRequest(_))
        ));
        assert!(f.store.binding("binding-one").await.unwrap().is_none());
        assert!(f.custody.keys.lock().unwrap().is_empty());
        assert!(f.custody.values.lock().unwrap().is_empty());
    }
    let binding = f.ready().await;
    assert_eq!(binding.state, BindingState::Ready);
}

mod access;
mod rotation;

mod resource_access;

mod command_tests;

mod event_tests;
mod existing;

mod continuity;

mod continuation_tests;
mod retirement;
