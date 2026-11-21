use super::*;
use crate::{bots::BotError, profiles::*, resource_access::ResourceOwner, SubjectId};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Default)]
struct Memory {
    records: Mutex<HashMap<String, BotSetup>>,
    fail_result_write: AtomicBool,
}
#[async_trait::async_trait]
impl BotSetupStore for Memory {
    async fn get(&self, actor: &ExternalActor) -> Result<Option<BotSetup>, BotError> {
        Ok(self.records.lock().unwrap().get(&actor.key()).cloned())
    }
    async fn insert(&self, setup: &BotSetup) -> Result<(), BotError> {
        let mut records = self.records.lock().unwrap();
        if records.contains_key(&setup.actor.key()) {
            return Err(BotError::Conflict);
        }
        records.insert(setup.actor.key(), setup.clone());
        Ok(())
    }
    async fn replace(&self, setup: &BotSetup, expected_revision: u64) -> Result<(), BotError> {
        if setup.bot_id.is_some() && self.fail_result_write.swap(false, Ordering::SeqCst) {
            return Err(BotError::Unavailable);
        }
        let mut records = self.records.lock().unwrap();
        let old = records.get(&setup.actor.key()).ok_or(BotError::NotFound)?;
        if old.revision != expected_revision {
            return Err(BotError::Conflict);
        }
        records.insert(setup.actor.key(), setup.clone());
        Ok(())
    }
}
struct Dependencies {
    events: Mutex<HashMap<String, SetupEvent>>,
    identities: Mutex<HashMap<String, SetupIdentity>>,
    resources: Mutex<HashMap<String, SetupResult>>,
    provision_calls: AtomicUsize,
    active: AtomicBool,
    organization_connected: AtomicBool,
    connections: Mutex<Vec<(ResourceOwner, String)>>,
    channel_ready: AtomicBool,
    channel_calls: Mutex<Vec<String>>,
    channel_cancellations: Mutex<Vec<String>>,
    channel_incarnation: AtomicUsize,
}
impl Dependencies {
    fn check(&self, actor: &ExternalActor, grant: &str) -> Result<(), BotError> {
        let identities = self.identities.lock().unwrap();
        if identities
            .get(&actor.key())
            .and_then(|id| id.grant_id.as_deref())
            != Some(grant)
        {
            return Err(BotError::Forbidden);
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl BotSetupDependencies for Dependencies {
    async fn event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        _activity: Option<&serde_json::Value>,
    ) -> Result<SetupEvent, BotError> {
        if binding != "binding" || credential != "binding-fixture" {
            return Err(BotError::Forbidden);
        }
        self.events
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .ok_or(BotError::Forbidden)
    }
    async fn identity(&self, actor: &ExternalActor) -> Result<SetupIdentity, BotError> {
        Ok(self
            .identities
            .lock()
            .unwrap()
            .get(&actor.key())
            .cloned()
            .unwrap_or_default())
    }
    async fn confirm_identity(
        &self,
        actor: &ExternalActor,
        _: &str,
    ) -> Result<SetupIdentity, BotError> {
        self.identity(actor).await
    }
    async fn owners(
        &self,
        actor: &ExternalActor,
        grant: &str,
    ) -> Result<Vec<SetupOwnerChoice>, BotError> {
        self.check(actor, grant)?;
        Ok(vec![
            SetupOwnerChoice {
                owner: ResourceOwner::Person {
                    subject: SubjectId::new(&actor.actor_id),
                },
                name: "Personal".into(),
            },
            SetupOwnerChoice {
                owner: ResourceOwner::Organization {
                    subject: SubjectId::new("org"),
                },
                name: "Organization".into(),
            },
        ])
    }
    async fn defaults(&self) -> Result<SetupConfiguration, BotError> {
        Ok(SetupConfiguration {
            agent: AgentProfileContent {
                isolated_agent: false,
                model: Some("model".into()),
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: "Default".into(),
            },
            workspace: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "work".into(),
                },
                name: "work".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
            sandbox: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1000,
                memory_mib: 2048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 1,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
            availability: Default::default(),
        })
    }
    async fn validate_configuration(
        &self,
        configuration: &SetupConfiguration,
    ) -> Result<(), BotError> {
        configuration
            .agent
            .validate()
            .map_err(|_| BotError::Invalid("agent".into()))?;
        configuration
            .workspace
            .validate()
            .map_err(|_| BotError::Invalid("workspace".into()))?;
        configuration
            .sandbox
            .validate(ProfileCapacityBounds {
                min_cpu_millis: 500,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            })
            .map_err(|_| BotError::Invalid("sandbox".into()))
    }
    async fn services(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        self.check(actor, grant)?;
        let connected = matches!(owner, ResourceOwner::Person { .. })
            || self.organization_connected.load(Ordering::SeqCst);
        Ok(vec![
            SetupServiceChoice {
                id: "managed-id".into(),
                slug: "managed".into(),
                name: "Managed".into(),
                connected,
                selected: false,
                required: true,
            },
            SetupServiceChoice {
                id: "optional-id".into(),
                slug: "optional".into(),
                name: "Optional".into(),
                connected: true,
                selected: false,
                required: false,
            },
        ])
    }
    async fn connect(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
        service: &str,
    ) -> Result<String, BotError> {
        self.check(actor, grant)?;
        self.connections
            .lock()
            .unwrap()
            .push((owner.clone(), service.into()));
        Ok("https://identity.invalid/connect".into())
    }
    async fn external_bots(
        &self,
        actor: &ExternalActor,
        grant: &str,
        _: &ResourceOwner,
    ) -> Result<Vec<SetupExternalBot>, BotError> {
        self.check(actor, grant)?;
        Ok(vec![SetupExternalBot {
            id: "external-bot".into(),
            name: "Example".into(),
            username: "example".into(),
            connection_ref: None,
        }])
    }
    async fn connection_authority_stamp(&self, setup: &BotSetup) -> Result<String, BotError> {
        self.check(
            &setup.actor,
            setup.grant_id.as_deref().ok_or(BotError::Forbidden)?,
        )?;
        Ok(format!(
            "{}:{}",
            setup.grant_id.as_deref().unwrap(),
            self.channel_incarnation.load(Ordering::SeqCst)
        ))
    }
    async fn channel_connection(
        &self,
        setup: &BotSetup,
    ) -> Result<SetupConnectionStatus, BotError> {
        let request = setup
            .pending_connection
            .as_ref()
            .ok_or(BotError::Conflict)?;
        if request.authority_stamp != self.connection_authority_stamp(setup).await? {
            return Err(BotError::Forbidden);
        }
        self.channel_calls
            .lock()
            .unwrap()
            .push(request.record_id.clone());
        Ok(SetupConnectionStatus {
            connection_ref: request.record_id.clone(),
            status: if self.channel_ready.load(Ordering::SeqCst) {
                "ready"
            } else {
                "pending"
            }
            .into(),
            setup_url: Some(format!(
                "https://gateway.invalid/connect/{}",
                request.record_id
            )),
        })
    }
    async fn cancel_channel_connection(&self, setup: &BotSetup) -> Result<(), BotError> {
        let request = setup
            .pending_connection
            .as_ref()
            .ok_or(BotError::Conflict)?;
        if request.authority_stamp != self.connection_authority_stamp(setup).await? {
            return Err(BotError::Forbidden);
        }
        self.channel_cancellations
            .lock()
            .unwrap()
            .push(request.record_id.clone());
        Ok(())
    }
    async fn provision(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        self.check(
            &setup.actor,
            setup.grant_id.as_deref().ok_or(BotError::Forbidden)?,
        )?;
        if setup.confirmation_digest.as_deref() != Some(setup.plan_digest()?.as_str()) {
            return Err(BotError::Conflict);
        }
        self.provision_calls.fetch_add(1, Ordering::SeqCst);
        let mut resources = self.resources.lock().unwrap();
        Ok(resources
            .entry(setup.id.clone())
            .or_insert_with(|| SetupResult {
                bot_id: format!("bot_{}", setup.id),
                state: "creating".into(),
                operation: Some("create".into()),
                failure: None,
                sandbox_id: None,
                workspace_id: None,
                agent_id: None,
                bot_url: None,
            })
            .clone())
    }
    async fn result(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        self.check(
            &setup.actor,
            setup.grant_id.as_deref().ok_or(BotError::Forbidden)?,
        )?;
        let mut result = self
            .resources
            .lock()
            .unwrap()
            .get(&setup.id)
            .cloned()
            .ok_or(BotError::NotFound)?;
        if self.active.load(Ordering::SeqCst) {
            result.state = "active".into();
            result.operation = None;
        }
        Ok(result)
    }
}
struct Fixture {
    store: Arc<Memory>,
    dependencies: Arc<Dependencies>,
    service: Arc<BotSetupService>,
    actor: ExternalActor,
    sequence: usize,
}
impl Fixture {
    fn new() -> Self {
        let actor = ExternalActor {
            binding_id: "binding".into(),
            channel_id: "channel".into(),
            conversation_id: "conversation".into(),
            actor_id: "alice".into(),
            platform: "fixture".into(),
        };
        let store = Arc::new(Memory::default());
        let dependencies = Arc::new(Dependencies {
            events: Mutex::new(HashMap::new()),
            identities: Mutex::new(HashMap::from([(
                actor.key(),
                SetupIdentity {
                    grant_id: Some("grant-alice".into()),
                    subject: Some("alice".into()),
                    display_name: Some("Alice".into()),
                    login_url: None,
                },
            )])),
            resources: Mutex::new(HashMap::new()),
            provision_calls: AtomicUsize::new(0),
            active: AtomicBool::new(false),
            organization_connected: AtomicBool::new(false),
            connections: Mutex::new(vec![]),
            channel_ready: AtomicBool::new(false),
            channel_calls: Mutex::new(vec![]),
            channel_cancellations: Mutex::new(vec![]),
            channel_incarnation: AtomicUsize::new(1),
        });
        let service = BotSetupService::new(store.clone(), dependencies.clone());
        Self {
            store,
            dependencies,
            service,
            actor,
            sequence: 0,
        }
    }
    fn event(&mut self, input: SetupInput) -> String {
        self.sequence += 1;
        let id = format!("event-{}", self.sequence);
        self.dependencies.events.lock().unwrap().insert(
            id.clone(),
            SetupEvent {
                reference: "cmaeg1.ev.fixture".into(),
                actor: self.actor.clone(),
                event_id: id.clone(),
                occurred_at: (time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(60 + self.sequence as i64))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
                input,
            },
        );
        id
    }
    async fn advance(&self, reference: &str) -> Result<SetupView, BotError> {
        self.service
            .advance(
                "binding",
                "binding-fixture",
                reference,
                SetupDraft::default(),
                SetupCustomization::default(),
            )
            .await
    }
    async fn begin(&mut self) -> SetupView {
        let id = self.event(SetupInput::Text("Create Example".into()));
        self.service
            .advance(
                "binding",
                "binding-fixture",
                &id,
                SetupDraft {
                    name: Some("Example".into()),
                    instructions: Some("Be precise".into()),
                },
                SetupCustomization::default(),
            )
            .await
            .unwrap()
    }
    async fn action(&mut self, view: &SetupView, action: &str) -> SetupView {
        let id = self.event(SetupInput::Interaction {
            block_id: view.interaction_id.clone(),
            action_id: action.into(),
        });
        self.advance(&id).await.unwrap()
    }
    async fn text(&mut self, text: &str) -> SetupView {
        let id = self.event(SetupInput::Text(text.into()));
        self.advance(&id).await.unwrap()
    }
    async fn services(&mut self, owner: &str) -> SetupView {
        let view = self.begin().await;
        assert_eq!(view.step, SetupStep::Owner);
        let mut view = self.action(&view, owner).await;
        for step in [SetupStep::Agent, SetupStep::Workspace, SetupStep::Sandbox] {
            assert_eq!(view.step, step);
            view = self.action(&view, "defaults").await;
        }
        assert_eq!(view.step, SetupStep::Services);
        view
    }
    async fn review(&mut self) -> SetupView {
        let view = self.services("personal").await;
        let view = self.action(&view, "done").await;
        assert_eq!(view.step, SetupStep::ExternalBot);
        let view = self.action(&view, "bot:external-bot").await;
        assert_eq!(view.step, SetupStep::Admission);
        let view = self.action(&view, "private").await;
        assert_eq!(view.step, SetupStep::Review);
        view
    }
    fn confirm(&mut self, view: &SetupView) -> String {
        let action = view
            .choices
            .iter()
            .find(|choice| choice.id.starts_with("create:"))
            .unwrap()
            .id
            .clone();
        self.event(SetupInput::Interaction {
            block_id: view.interaction_id.clone(),
            action_id: action,
        })
    }
}

mod customization;
mod flows;

mod agent_actions;
mod channel_connections;
