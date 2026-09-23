use super::Scenario;
use layer2_core::{bots::BotError, event_workers::*, Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;
struct Unavailable;
#[async_trait::async_trait]
impl EventWorkerProfileStore for Unavailable {
    async fn get(&self, _: &str, _: Option<u32>) -> Result<Option<EventWorkerProfile>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn list(
        &self,
        _: &SubjectId,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<EventWorkerProfile>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn save(&self, _: Option<u32>, _: &EventWorkerProfile) -> Result<(), BotError> {
        Err(BotError::Unavailable)
    }
}
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("event_worker_contract_")?;
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::ManageOwnProfiles]),
    );
    let store: Arc<dyn EventWorkerProfileStore> = if mode == "unavailable" {
        Arc::new(Unavailable)
    } else {
        Arc::new(InMemoryEventWorkerProfileStore::default())
    };
    let service = EventWorkerProfileService::new(
        store,
        Arc::new(super::bot_cases::Profiles),
        "cma_codex".into(),
    );
    let input:EventWorkerProfileInput=serde_json::from_value(serde_json::json!({"name":"Example worker","description":"Complete worker configuration","agent_profile":{"id":"agp_bot","revision":1},"sandbox_profile":{"id":"sbp_bot","revision":1},"workspace_profile":{"id":"wkp_bot","revision":1}})).unwrap();
    if mode != "create" && mode != "invalid" && mode != "unavailable" {
        service
            .create(&owner, "contract-worker", input)
            .await
            .unwrap();
    }
    Some(Scenario {
        router: crate::rest::event_workers::router(service),
        subject: match mode {
            "anonymous" => None,
            "foreign" => Some("foreign"),
            _ => Some("owner"),
        },
        authorization: if mode == "forbidden" {
            Authorization::new([])
        } else {
            owner.authorization
        },
        symbols: Default::default(),
    })
}
