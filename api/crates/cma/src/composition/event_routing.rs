//! CMA selects profile instances from verified source identity; CMAEG owns only IO.
use layer1_public::{
    responses::v2::{BindingEventRouter, BindingRouteError},
    AuthenticatedResourceCaller,
};
use layer2_core::{
    sandbox::{
        agent_io::ExternalEventContext,
        binding::{BindingConversationState, BindingService, BindingStore, SessionAffinity},
    },
    AgentId, Authorization, Caller, Capability,
};
use serde::Deserialize;
mod sender;
use std::{sync::Arc, time::Duration};

pub(crate) struct CmaEventRouter {
    store: Arc<crate::store::MongoSandboxStore>,
    bindings: Arc<BindingService>,
    gateway: String,
    http: reqwest::Client,
}

impl CmaEventRouter {
    pub(crate) fn new(
        store: Arc<crate::store::MongoSandboxStore>,
        bindings: Arc<BindingService>,
        gateway: &str,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            store,
            bindings,
            gateway: gateway.trim_end_matches('/').into(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .map_err(|_| "event verifier configuration failed")?,
        }))
    }
}

#[derive(Deserialize)]
struct VerifiedContext {
    channel_id: String,
    provider_binding_id: String,
    conversation_id: String,
    event_id: String,
    activity_verified: bool,
}

#[async_trait::async_trait]
impl BindingEventRouter for CmaEventRouter {
    async fn verified_sender_line(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &ExternalEventContext,
        generation: u64,
    ) -> Result<String, BindingRouteError> {
        use BindingRouteError as E;
        let binding = self
            .store
            .find_by_key_hash(&caller.context.credential.credential_id)
            .await
            .map_err(|_| E::SenderUnavailable)?
            .ok_or(E::SenderInvalid)?;
        binding
            .require_generation(Some(generation))
            .map_err(|_| E::SenderInvalid)?;
        let credential = caller.credential.as_ref().ok_or(E::SenderInvalid)?;
        // No transaction or lease crosses this bounded network operation. The
        // caller separately fences its binding revision during turn reservation.
        let activity = tokio::time::timeout(
            Duration::from_secs(2),
            verify_activity(
                &self.http,
                &self.gateway,
                &binding.binding_id,
                credential.value(),
                event,
            ),
        )
        .await
        .map_err(|_| E::SenderUnavailable)?
        .map_err(|error| match error {
            E::Forbidden => E::SenderInvalid,
            E::Invalid => E::SenderUnsupported,
            _ => E::SenderUnavailable,
        })?;
        sender::line(&activity)
    }
    async fn route(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
    ) -> Result<Option<AgentId>, BindingRouteError> {
        self.route_inner(caller, event, None).await
    }
    async fn route_at_generation(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
        generation: u64,
    ) -> Result<Option<AgentId>, BindingRouteError> {
        self.route_inner(caller, event, Some(generation)).await
    }
}
impl CmaEventRouter {
    async fn route_inner(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
        generation: Option<u64>,
    ) -> Result<Option<AgentId>, BindingRouteError> {
        use BindingRouteError as E;
        let binding = self
            .store
            .find_by_key_hash(&caller.context.credential.credential_id)
            .await
            .map_err(|_| E::Unavailable)?
            .ok_or(E::Forbidden)?;
        binding
            .require_generation(generation)
            .map_err(|_| E::Forbidden)?;
        let manager = Caller::new(
            binding.manager.clone(),
            Authorization::new([
                Capability::ManageGatewayRecords,
                Capability::ManageProviderBindings,
            ]),
        );
        if let Some(target) = &binding.existing_agent {
            let (conversation, _) = self
                .bindings
                .ensure_conversation_at_generation(
                    &manager,
                    &binding.binding_id,
                    layer2_core::sandbox::binding::EXISTING_AGENT_CONVERSATION,
                    Some(binding.generation),
                )
                .await
                .map_err(|_| E::Unavailable)?;
            if conversation.agent_id != target.agent_id
                || conversation.state != BindingConversationState::Ready
            {
                return Err(E::NotReady);
            }
            event.provider_conversation_id = Some(conversation.conversation_id);
            return Ok(Some(conversation.agent_id));
        }
        if binding.profile.session_affinity == SessionAffinity::Conversation {
            return Ok(None);
        }
        let credential = caller.credential.as_ref().ok_or(E::Forbidden)?;
        let activity = verify_activity(
            &self.http,
            &self.gateway,
            &binding.binding_id,
            credential.value(),
            event,
        )
        .await?;
        let session = source_user_session(&binding.binding_id, binding.generation, &activity)?;
        let (conversation, _) = self
            .bindings
            .ensure_conversation_at_generation(
                &manager,
                &binding.binding_id,
                &session,
                Some(binding.generation),
            )
            .await
            .map_err(|_| E::Unavailable)?;
        match conversation.state {
            BindingConversationState::Ready => {}
            BindingConversationState::Provisioning => return Err(E::NotReady),
            _ => return Err(E::Unavailable),
        }
        event.provider_conversation_id = Some(conversation.conversation_id);
        Ok(Some(conversation.agent_id))
    }
}

// This is a CMA routing projection, not the gateway's activity schema.
// The gateway validates the full original Activity through its authenticated API.
#[derive(Deserialize)]
struct ActivityIdentity {
    event_id: String,
    actor: SourceActor,
    source: ActivitySource,
    conversation: ActivityConversation,
    #[serde(default)]
    kind: serde_json::Value,
}
#[derive(Deserialize)]
struct SourceActor {
    id: String,
    kind: String,
}
#[derive(Deserialize)]
struct ActivitySource {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    platform: Option<String>,
}
#[derive(Deserialize)]
struct ActivityConversation {
    kind: String,
}

async fn verify_activity(
    http: &reqwest::Client,
    gateway: &str,
    binding: &str,
    credential: &str,
    event: &ExternalEventContext,
) -> Result<ActivityIdentity, BindingRouteError> {
    use BindingRouteError as E;
    event.validate().map_err(|_| E::Invalid)?;
    if !event.reference.starts_with("cmaeg1.ev.")
        || !event
            .reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(E::Forbidden);
    }
    // Authenticate the complete original wire value. Routing only consumes
    // the identity projection after the gateway verifies its sealed digest.
    let activity = event.payload.get("activity").ok_or(E::Invalid)?;
    let response = http
        .post(format!("{}/v1/events/{}/context", gateway, event.reference))
        .bearer_auth(credential)
        .json(activity)
        .send()
        .await
        .map_err(|_| E::Unavailable)?;
    if matches!(response.status().as_u16(), 401 | 403 | 400 | 410) {
        return Err(E::Forbidden);
    }
    if !response.status().is_success() {
        return Err(E::Unavailable);
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| E::Unavailable)? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(E::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    let proof: VerifiedContext = serde_json::from_slice(&bytes).map_err(|_| E::Unavailable)?;
    if !proof.activity_verified
        || proof.channel_id != event.source_id
        || proof.conversation_id != event.conversation_id
        || proof.provider_binding_id != binding
        || proof.event_id != event.event_id
    {
        return Err(E::Forbidden);
    }
    let identity: ActivityIdentity =
        serde_json::from_value(activity.clone()).map_err(|_| E::Invalid)?;
    if identity.event_id != event.event_id {
        return Err(E::Forbidden);
    }
    Ok(identity)
}

fn source_user_session(
    binding: &str,
    generation: u64,
    activity: &ActivityIdentity,
) -> Result<String, BindingRouteError> {
    let Some(platform) = &activity.source.platform else {
        return Err(BindingRouteError::Invalid);
    };
    let completion = activity.actor.kind == "system"
        && activity.kind["type"].as_str() == Some("event")
        && activity.kind["ce"]["type"].as_str() == Some("gateway.connection.ready");
    if (activity.actor.kind != "human" && !completion)
        || activity.actor.id.is_empty()
        || platform.is_empty()
        || platform.len() > 32
    {
        return Err(BindingRouteError::Invalid);
    }
    layer2_core::profiles::content_digest(&(
        binding,
        generation,
        platform.as_str(),
        &activity.actor.id,
    ))
    .map(|digest| format!("cma_user_{digest}"))
    .map_err(|_| BindingRouteError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::HeaderMap, routing::post, Json, Router};
    use serde_json::json;
    fn activity() -> serde_json::Value {
        json!({"schema_version":1,"event_id":"event-a",
            "kind":{"type":"message","text":"hello","mentions_bot":false},
            "actor":{"id":"sender-one","kind":"human"},
            "conversation":{"id":"chat-a","kind":"private"},
            "occurred_at":"2026-09-29T00:00:00Z", "source":{"type":"nyxid_relay","platform":"telegram","route_id":"route-a"}})
    }
    fn identity(activity: &serde_json::Value) -> ActivityIdentity {
        serde_json::from_value(activity.clone()).unwrap()
    }
    #[test]
    fn source_user_affinity_is_stable_across_events_and_chats_but_separates_authority() {
        let mut first = activity();
        let expected = source_user_session("binding-a", 1, &identity(&first)).unwrap();
        first["conversation"]["id"] = "chat-b".into();
        first["event_id"] = "event-b".into();
        assert_eq!(
            source_user_session("binding-a", 1, &identity(&first)).unwrap(),
            expected
        );
        assert_ne!(
            source_user_session("binding-a", 2, &identity(&first)).unwrap(),
            expected
        );
        assert_ne!(
            source_user_session("binding-b", 1, &identity(&first)).unwrap(),
            expected
        );
        first["actor"]["id"] = "sender-two".into();
        assert_ne!(
            source_user_session("binding-a", 1, &identity(&first)).unwrap(),
            expected
        );
    }
    #[test]
    fn system_and_unattested_custom_senders_do_not_acquire_human_affinity() {
        let mut first = activity();
        first["actor"]["kind"] = "system".into();
        assert!(source_user_session("binding", 1, &identity(&first)).is_err());
        first["actor"]["kind"] = "human".into();
        first["source"] = json!({"type":"custom"});
        assert!(source_user_session("binding", 1, &identity(&first)).is_err());
    }

    #[test]
    fn attested_connection_completion_and_native_source_keep_original_actor_affinity() {
        let mut value = activity();
        let expected = source_user_session("binding", 1, &identity(&value)).unwrap();
        value["source"] = json!({"type":"native-fixture", "platform":"telegram", "bot_id":"123"});
        assert_eq!(
            source_user_session("binding", 1, &identity(&value)).unwrap(),
            expected
        );
        value["actor"]["kind"] = "system".into();
        value["kind"] = json!({"type":"event","ce":{"type":"gateway.connection.ready","data":{"connection_ref":"opaque","status":"ready"}}});
        assert_eq!(
            source_user_session("binding", 1, &identity(&value)).unwrap(),
            expected
        );
        value["kind"]["ce"]["type"] = "unrelated.system.event".into();
        assert!(source_user_session("binding", 1, &identity(&value)).is_err());
    }

    #[tokio::test]
    async fn verifier_sends_complete_activity_and_requires_exact_gateway_proof() {
        let mut original = activity();
        original["kind"]["text"] = "x".repeat(32 * 1024).into();
        original["kind"]["attachments"] = json!([{
            "reference":"fixture-image", "kind":"image", "filename":"photo.png",
            "mime_type":"image/png", "size_bytes":1024
        }]);
        original["actor"]["display_name"] = "Visitor".into();
        original["conversation"]["thread_id"] = "thread-a".into();
        let event = ExternalEventContext {
            provider_conversation_id: None,
            source_id: "channel-a".into(),
            conversation_id: "chat-a".into(),
            event_id: "event-a".into(),
            reference: "cmaeg1.ev.fixture".into(),
            payload: json!({"activity":original}),
        };
        for mismatch in [
            None,
            Some("activity_verified"),
            Some("channel_id"),
            Some("conversation_id"),
            Some("provider_binding_id"),
            Some("event_id"),
        ] {
            let expected = event.payload["activity"].clone();
            let app = Router::new().route(
                "/v1/events/cmaeg1.ev.fixture/context",
                post(move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let expected = expected.clone();
                    async move {
                        assert_eq!(headers["authorization"], "Bearer fixture-credential");
                        assert_eq!(body, expected, "authenticate the complete original Activity");
                        let mut proof = json!({"channel_id":"channel-a","conversation_id":"chat-a",
                            "provider_binding_id":"binding-a","event_id":"event-a","activity_verified":true});
                        if let Some(field) = mismatch {
                            proof[field] = if field == "activity_verified" {
                                false.into()
                            } else {
                                "wrong-source".into()
                            };
                        }
                        Json(proof)
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let gateway = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let result = verify_activity(
                &reqwest::Client::new(),
                &gateway,
                "binding-a",
                "fixture-credential",
                &event,
            )
            .await;
            server.abort();
            if mismatch.is_some() {
                assert!(matches!(result, Err(BindingRouteError::Forbidden)));
            } else {
                let verified = result.unwrap();
                assert_eq!(verified.event_id, "event-a");
                assert_eq!(verified.actor.id, "sender-one");
                assert_eq!(
                    source_user_session("binding-a", 1, &verified).unwrap(),
                    source_user_session("binding-a", 1, &identity(&original)).unwrap()
                );
            }
        }
    }
}
