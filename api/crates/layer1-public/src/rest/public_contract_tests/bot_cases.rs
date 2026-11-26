use super::Scenario;
use layer2_core::{bots::*, profiles::*, Authorization, Capability, SubjectId};
use serde_json::json;
use std::sync::Arc;

pub(super) struct Profiles;
#[async_trait::async_trait]
impl BotProfileResolver for Profiles {
    async fn resolve(&self, _: &SubjectId, request: &CreateBot) -> Result<BotProfiles, BotError> {
        let mut pinned = profiles();
        pinned.agent.profile_id = request.profile.id.clone();
        pinned.agent.revision = request.profile.revision;
        if let Some(reference) = &request.sandbox_profile {
            pinned.sandbox.profile_id = reference.id.clone();
            pinned.sandbox.revision = reference.revision;
        }
        if let Some(reference) = &request.workspace_profile {
            pinned.workspace.profile_id = reference.id.clone();
            pinned.workspace.revision = reference.revision;
        }
        Ok(pinned)
    }
}
fn profiles() -> BotProfiles {
    let at = "2026-01-01T00:00:00Z";
    let agent = json!({"model":"test","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":""});
    let sandbox = json!({"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]});
    let workspace = json!({"location":{"type":"directory","name":"project"},"name":"project","setup_commands":[],"agent_profiles":[]});
    serde_json::from_value(json!({"resolved_model":"test", "agent":{"profile_id":"agp_bot","revision":1,"digest":content_digest(&agent).unwrap(),"content":agent,"published_at":at},
        "sandbox":{"profile_id":"sbp_bot","revision":1,"digest":content_digest(&sandbox).unwrap(),"content":sandbox,"published_at":at},
        "workspace":{"profile_id":"wkp_bot","revision":1,"digest":content_digest(&workspace).unwrap(),"content":workspace,"published_at":at}})).unwrap()
}

struct External(bool);
#[async_trait::async_trait]
impl BotExternal for External {
    async fn available(&self, _: &SubjectId) -> Result<Vec<ExternalBot>, BotError> {
        if self.0 {
            return Err(BotError::Unavailable);
        }
        Ok(vec![ExternalBot {
            id: "external-bot".into(),
            name: "Example bot".into(),
            platform: "telegram".into(),
        }])
    }
    async fn test(&self, _: &Bot) -> Result<BotTestResult, BotError> {
        if self.0 {
            return Err(BotError::Unavailable);
        }
        Ok(BotTestResult {
            accepted_after_ms: 12,
            first_text: "Ready".into(),
            level: "l2".into(),
            tested_at: "2026-01-02T00:00:00Z".into(),
        })
    }
    async fn verify(&self, _: &Bot) -> Result<(), BotError> {
        unreachable!()
    }
    async fn mint(&self, _: &Bot, _: BotKeyRole) -> Result<BotKey, BotError> {
        unreachable!()
    }
    async fn keys(&self, _: &Bot) -> Result<Vec<BotKeyRef>, BotError> {
        unreachable!()
    }
    async fn revoke(&self, _: &Bot, _: &str) -> Result<(), BotError> {
        unreachable!()
    }
    async fn create_channel(
        &self,
        _: &Bot,
        _: &BotSecrets,
    ) -> Result<BotChannelObservation, BotError> {
        unreachable!()
    }
    async fn channel(&self, _: &Bot) -> Result<Option<BotChannelObservation>, BotError> {
        unreachable!()
    }
    async fn configure_channel(
        &self,
        _: &Bot,
        _: &BotChannelObservation,
        _: &BotCredentialSet,
        _: Option<&BotSecrets>,
        _: bool,
    ) -> Result<BotChannelObservation, BotError> {
        unreachable!()
    }
    async fn delete_channel(&self, _: &Bot, _: &BotChannelObservation) -> Result<(), BotError> {
        unreachable!()
    }
    async fn set_callback(&self, _: &Bot, _: &str, _: &str) -> Result<(), BotError> {
        unreachable!()
    }
    async fn create_route(&self, _: &Bot, _: &str) -> Result<String, BotError> {
        unreachable!()
    }
    async fn set_route_key(&self, _: &Bot, _: &str, _: &str) -> Result<(), BotError> {
        unreachable!()
    }
    async fn active_route_key(&self, _: &Bot) -> Result<Option<String>, BotError> {
        unreachable!()
    }
    async fn delete_routes(&self, _: &Bot, _: &[String]) -> Result<(), BotError> {
        unreachable!()
    }
}
struct Unavailable;
#[async_trait::async_trait]
impl BotStore for Unavailable {
    async fn insert(&self, _: &Bot) -> Result<(), BotError> {
        Err(BotError::Unavailable)
    }
    async fn get(&self, _: &str) -> Result<Option<Bot>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn creation(&self, _: &SubjectId, _: &str) -> Result<Option<Bot>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn list(&self, _: &SubjectId, _: Option<&str>, _: usize) -> Result<Vec<Bot>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn replace(&self, _: u64, _: &Bot) -> Result<(), BotError> {
        Err(BotError::Unavailable)
    }
    async fn pending(&self, _: Option<&str>, _: usize) -> Result<Vec<Bot>, BotError> {
        Err(BotError::Unavailable)
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("bot_contract_")?;
    let store = Arc::new(InMemoryBotStore::default());
    let request = CreateBot {
        event_worker_profile: None,
        name: "Example bot".into(),
        external_bot_id: "external-bot".into(),
        connection_ref: None,
        image: None,
        repository: None,
        inline_agent: None,
        profile: AgentProfileRef {
            id: layer2_core::AgentProfileId("agp_bot".into()),
            revision: 1,
        },
        sandbox_profile: None,
        workspace_profile: None,
        admission: BotAdmission::All,
        reply: BotReply {
            auto_final: true,
            ack_after_ms: 0,
        },
    };
    let at = "2026-01-01T00:00:00Z";
    let mut bot = Bot {
        ownership: None,
        bot_id: "bot_contract".into(),
        owner: SubjectId::new("owner"),
        request,
        profiles: profiles(),
        provider: "cma_codex".into(),
        record_id: "8ace3890-01ba-4b93-9cc5-fba8fa496bfa".into(),
        resources: BotResources::default(),
        state: BotState::Active,
        work: None,
        generation: 1,
        revision: 1,
        creation_key_hash: "a".repeat(64),
        request_fingerprint: "b".repeat(64),
        created_at: at.into(),
        updated_at: at.into(),
        last_test: None,
    };
    bot.resources.channel = Some(BotChannel {
        channel_id: "opaque-channel".into(),
        record_id: bot.record_id.clone(),
        version: 1,
        binding_id: "binding-contract".into(),
        level: "l2".into(),
        callback_url: "https://gateway.example/v1/callbacks/opaque-channel".into(),
        event_url: "https://gateway.example/v1/channels/opaque-channel/events".into(),
    });
    bot.resources.route_id = Some("route-contract".into());
    bot.resources.credentials = BotCredentialSet {
        agent_key_id: Some("agent-contract".into()),
        route_key_id: Some("route-key-contract".into()),
        agent_key_hash: Some("c".repeat(64)),
        route_key_hash: Some("d".repeat(64)),
        expires_at: Some("2026-02-01T00:00:00Z".into()),
    };
    if mode.starts_with("disabled") {
        bot.state = BotState::Disabled;
    }
    if mode == "failed" {
        bot.state = BotState::Failed {
            reason: "fixture_failure".into(),
        };
    }
    if mode == "busy" {
        bot.work = Some(BotWork {
            effect_protocol: 1,
            effect: None,
            id: "work-contract".into(),
            operation: BotOperation::Rotate,
            step: "admitted".into(),
            generation: 1,
            holder: None,
            expires_at: None,
        });
    }
    store.insert(&bot).await.unwrap();
    let store: Arc<dyn BotStore> = if mode == "unavailable" {
        Arc::new(Unavailable)
    } else {
        store
    };
    let service = BotService::new(store, Arc::new(Profiles), "cma_codex".into());
    if mode == "creation_conflict" {
        let caller = layer2_core::Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::ManageOwnBots]),
        );
        let mut prior = bot.request;
        prior.name = "Previous request".into();
        service
            .create(&caller, "contract-create", prior)
            .await
            .unwrap();
    }
    Some(Scenario {
        router: crate::rest::bot_router(BotControl::new(
            service,
            Arc::new(External(mode == "unavailable")),
        )),
        subject: if mode == "anonymous" {
            None
        } else if mode == "foreign" {
            Some("other")
        } else {
            Some("owner")
        },
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::ManageOwnBots])
        },
        symbols: Default::default(),
    })
}
