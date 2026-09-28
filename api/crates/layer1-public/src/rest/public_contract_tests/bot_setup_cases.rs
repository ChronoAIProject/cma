use super::Scenario;
use axum::Extension;
use layer2_core::{
    bot_setups::*, bots::BotError, resource_access::ResourceOwner, Authorization, SubjectId,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct SetupStore(Mutex<BotSetup>);
#[async_trait::async_trait]
impl BotSetupStore for SetupStore {
    async fn get(&self, actor: &ExternalActor) -> Result<Option<BotSetup>, BotError> {
        let setup = self.0.lock().unwrap();
        Ok((&setup.actor == actor).then(|| setup.clone()))
    }
    async fn insert(&self, _: &BotSetup) -> Result<(), BotError> {
        Err(BotError::Conflict)
    }
    async fn replace(&self, setup: &BotSetup, expected: u64) -> Result<(), BotError> {
        let mut stored = self.0.lock().unwrap();
        if stored.revision != expected
            || stored.actor != setup.actor
            || setup.revision != expected + 1
        {
            return Err(BotError::Conflict);
        }
        *stored = setup.clone();
        Ok(())
    }
}

fn actor() -> ExternalActor {
    ExternalActor {
        binding_id: "bnd_contract".into(),
        channel_id: "channel-contract".into(),
        conversation_id: "chat-contract".into(),
        actor_id: "actor-contract".into(),
        platform: "telegram".into(),
    }
}
fn configuration() -> SetupConfiguration {
    serde_json::from_value(json!({
        "agent":{"model":"test","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":""},
        "sandbox":{"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]},
        "workspace":{"location":{"type":"directory","name":"project"},"name":"project","setup_commands":[],"agent_profiles":[]},
        "availability":{"warmup_enabled":false,"warmup_count":1,"always_keep_alive":false}
    })).unwrap()
}
struct Dependencies(String);
#[async_trait::async_trait]
impl BotSetupDependencies for Dependencies {
    async fn event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        activity: Option<&Value>,
    ) -> Result<SetupEvent, BotError> {
        assert_eq!(binding, "bnd_contract");
        assert_eq!(credential, "contract-binding-key");
        match self.0.as_str() {
            "forbidden" => return Err(BotError::Forbidden),
            "missing" => return Err(BotError::NotFound),
            "conflict" => return Err(BotError::Conflict),
            "unavailable" => return Err(BotError::Unavailable),
            _ => {}
        }
        let input = if self.0 == "legacy" {
            assert!(activity.is_none());
            SetupInput::Text("Create my bot".into())
        } else {
            let activity = activity.ok_or(BotError::Forbidden)?;
            if activity["schema_version"] != 1
                || activity["event_id"] != reference
                || activity["actor"]["id"] != "actor-contract"
                || activity["conversation"]["id"] != "chat-contract"
                || activity["source"]["platform"] != "telegram"
            {
                return Err(BotError::Forbidden);
            }
            match activity["kind"]["type"].as_str() {
                Some("message") => {
                    SetupInput::Text(activity["kind"]["text"].as_str().unwrap().into())
                }
                Some("interaction") => SetupInput::Interaction {
                    block_id: activity["kind"]["block_id"].as_str().unwrap().into(),
                    action_id: activity["kind"]["action_id"].as_str().unwrap().into(),
                },
                _ => return Err(BotError::Invalid("Unsupported setup activity".into())),
            }
        };
        Ok(SetupEvent {
            occurred_at: "2026-09-28T10:00:00Z".into(),
            actor: actor(),
            event_id: reference.into(),
            input,
        })
    }
    async fn identity(&self, _: &ExternalActor) -> Result<SetupIdentity, BotError> {
        Ok(SetupIdentity {
            grant_id: Some("grant-contract".into()),
            subject: Some("actor-contract".into()),
            display_name: Some("Contract user".into()),
            login_url: None,
        })
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
        _: &ExternalActor,
        grant: &str,
    ) -> Result<Vec<SetupOwnerChoice>, BotError> {
        assert_eq!(grant, "grant-contract");
        Ok(vec![SetupOwnerChoice {
            owner: ResourceOwner::Person {
                subject: SubjectId::new("actor-contract"),
            },
            name: "Personal".into(),
        }])
    }
    async fn defaults(&self) -> Result<SetupConfiguration, BotError> {
        Ok(configuration())
    }
    async fn validate_configuration(&self, _: &SetupConfiguration) -> Result<(), BotError> {
        Ok(())
    }
    async fn services(
        &self,
        _: &ExternalActor,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        unreachable!("setup contract does not reach service selection")
    }
    async fn connect(
        &self,
        _: &ExternalActor,
        _: &str,
        _: &ResourceOwner,
        _: &str,
    ) -> Result<String, BotError> {
        unreachable!("setup contract does not connect services")
    }
    async fn external_bots(
        &self,
        _: &ExternalActor,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<Vec<SetupExternalBot>, BotError> {
        unreachable!("setup contract does not enumerate bots")
    }
    async fn external_bot_registration_url(&self, _: &ResourceOwner) -> Result<String, BotError> {
        unreachable!("setup contract does not register bots")
    }
    async fn provision(&self, _: &BotSetup) -> Result<SetupResult, BotError> {
        panic!("REST setup advance must not provision before explicit consent")
    }
    async fn result(&self, _: &BotSetup) -> Result<SetupResult, BotError> {
        unreachable!("setup contract has no provisioned bot")
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("bot_setup_contract_")?;
    let store = SetupStore(Mutex::new(BotSetup {
        id: "bst_contract".into(),
        actor: actor(),
        revision: 1,
        step: SetupStep::Login,
        grant_id: None,
        owner: None,
        draft: SetupDraft::default(),
        configuration: configuration(),
        selected_services: vec![],
        external_bot_id: None,
        admission: None,
        service_page: 0,
        confirmation_digest: None,
        confirmation_event: None,
        processed_actions: vec![],
        bot_id: None,
        processed_events: vec![],
        updated_at: "now".into(),
    }));
    let service = BotSetupService::new(Arc::new(store), Arc::new(Dependencies(mode.into())));
    Some(Scenario {
        router: crate::rest::bot_setup_router(service)
            .layer(Extension(crate::AuthMethod::BindingKey {
                binding_id: "bnd_contract".into(),
            }))
            .layer(Extension(crate::DelegatedCredential::new(
                "contract-binding-key".into(),
            ))),
        subject: Some("binding-owner"),
        authorization: Authorization::none(),
        symbols: BTreeMap::new(),
    })
}

#[cfg(test)]
mod agent_actions {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
    };
    use tower::ServiceExt;

    async fn post(router: &Router, body: Value) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/bot-setups/actions")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn structured_rest_actions_use_attested_context_and_preserve_replay_revision() {
        let router = scenario("bot_setup_contract_legacy").await.unwrap().router;
        let start = json!({"event_ref":"same-human-event","request":{"request_id":"view","action":{"type":"view"}}});
        let (status, view) = post(&router, start.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(view["step"], "owner");
        let (status, replay) = post(&router, start).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(view["revision"], replay["revision"]);
        let choose = json!({"event_ref":"same-human-event","request":{"request_id":"choose","expected":{"setup_id":view["setup_id"],"revision":view["revision"]},"action":{"type":"choose","choice_id":"personal"}}});
        let (status, selected) = post(&router, choose.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(selected["step"], "name");
        let (status, replay) = post(&router, choose.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(selected["revision"], replay["revision"]);
        let mut stale = choose;
        stale["request"]["request_id"] = "stale".into();
        assert_eq!(post(&router, stale).await.0, StatusCode::CONFLICT);
        let draft = json!({"event_ref":"same-human-event","request":{"request_id":"draft","expected":{"setup_id":selected["setup_id"],"revision":selected["revision"]},"action":{"type":"draft","draft":{"name":"Paper assistant","instructions":"Explain papers"}}}});
        let (status, drafted) = post(&router, draft).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(drafted["step"], "agent");
        assert_eq!(
            drafted["configuration"]["agent"]["system_prompt"],
            "Explain papers"
        );
    }

    #[tokio::test]
    async fn structured_route_requires_revision_and_rejects_missing_event_authority() {
        let router = scenario("bot_setup_contract_legacy").await.unwrap().router;
        let mutation = json!({"event_ref":"event","request":{"request_id":"choose","action":{"type":"choose","choice_id":"personal"}}});
        let (status, problem) = post(&router, mutation).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(problem["code"], "invalid_setup");
        let denied = scenario("bot_setup_contract_forbidden")
            .await
            .unwrap()
            .router;
        let (status, problem) = post(
            &denied,
            json!({"event_ref":"event","request":{"request_id":"view","action":{"type":"view"}}}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(problem["code"], "setup_authority_required");
    }
}
