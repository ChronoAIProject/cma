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
use std::{sync::Arc, time::Duration};

pub(super) struct CmaEventRouter {
    store: Arc<crate::store::MongoSandboxStore>,
    bindings: Arc<BindingService>,
    gateway: String,
    http: reqwest::Client,
}

impl CmaEventRouter {
    pub(super) fn new(
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
    async fn route(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
    ) -> Result<Option<AgentId>, BindingRouteError> {
        use BindingRouteError as E;
        let binding = self
            .store
            .find_by_key_hash(&caller.context.credential.credential_id)
            .await
            .map_err(|_| E::Unavailable)?
            .ok_or(E::Forbidden)?;
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
                .ensure_conversation(
                    &manager,
                    &binding.binding_id,
                    layer2_core::sandbox::binding::EXISTING_AGENT_CONVERSATION,
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
        event.validate().map_err(|_| E::Invalid)?;
        if !event.reference.starts_with("cmaeg1.ev.")
            || !event
                .reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err(E::Forbidden);
        }
        let activity: cmaeg_core::activity::Activity =
            serde_json::from_value(event.payload["activity"].clone()).map_err(|_| E::Invalid)?;
        activity.validate().map_err(|_| E::Invalid)?;
        let credential = caller.credential.as_ref().ok_or(E::Forbidden)?;
        let response = self
            .http
            .post(format!(
                "{}/v1/events/{}/context",
                self.gateway, event.reference
            ))
            .bearer_auth(credential.value())
            .json(&activity)
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
            || proof.provider_binding_id != binding.binding_id
            || proof.event_id != event.event_id
            || activity.event_id.as_str() != event.event_id
        {
            return Err(E::Forbidden);
        }
        let session = source_user_session(&binding.binding_id, binding.generation, &activity)?;
        let (conversation, _) = self
            .bindings
            .ensure_conversation(&manager, &binding.binding_id, &session)
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

fn source_user_session(
    binding: &str,
    generation: u64,
    activity: &cmaeg_core::activity::Activity,
) -> Result<String, BindingRouteError> {
    use cmaeg_core::activity::{ActorKind, SourceKind};
    let SourceKind::NyxidRelay { platform, .. } = &activity.source else {
        return Err(BindingRouteError::Invalid);
    };
    if activity.actor.kind != ActorKind::Human {
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
    use serde_json::json;
    fn activity() -> cmaeg_core::activity::Activity {
        serde_json::from_value(json!({"schema_version":1,"event_id":"event-a",
            "kind":{"type":"message","text":"hello","mentions_bot":false},
            "actor":{"id":"sender-one","kind":"human"},
            "conversation":{"id":"chat-a","kind":"private"},
            "occurred_at":"2026-09-29T00:00:00Z", "source":{"type":"nyxid_relay","platform":"telegram","route_id":"route-a"}})).unwrap()
    }
    #[test]
    fn source_user_affinity_is_stable_across_events_and_chats_but_separates_authority() {
        let mut first = activity();
        let expected = source_user_session("binding-a", 1, &first).unwrap();
        first.conversation.id = "chat-b".into();
        first.event_id = cmaeg_core::ids::EventId::new("event-b").unwrap();
        assert_eq!(
            source_user_session("binding-a", 1, &first).unwrap(),
            expected
        );
        assert_ne!(
            source_user_session("binding-a", 2, &first).unwrap(),
            expected
        );
        assert_ne!(
            source_user_session("binding-b", 1, &first).unwrap(),
            expected
        );
        first.actor.id = "sender-two".into();
        assert_ne!(
            source_user_session("binding-a", 1, &first).unwrap(),
            expected
        );
    }
    #[test]
    fn system_and_unattested_custom_senders_do_not_acquire_human_affinity() {
        let mut first = activity();
        first.actor.kind = cmaeg_core::activity::ActorKind::System;
        assert!(source_user_session("binding", 1, &first).is_err());
        first.actor.kind = cmaeg_core::activity::ActorKind::Human;
        first.source = cmaeg_core::activity::SourceKind::Custom;
        assert!(source_user_session("binding", 1, &first).is_err());
    }
}
