//! Durable login callback delivery belongs to CMA, including its agent and reply.
use super::*;
use crate::auth::{actor_links::ActorWakeupNotification, persistence::SecretValue};
use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use layer1_public::{AuthenticatedSubject, DelegatedCredential};
use layer2_core::{
    resource_access::{CallerContext, CredentialContext, CredentialSource, ResourceActor},
    sandbox::agent_io::{
        AgentCommand, AgentCommandService, CommandSubmission, CommandSubmissionSource,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

pub(crate) struct ActorContinuationWorker {
    dependencies: Arc<SetupAuthority>,
    commands: Arc<AgentCommandService>,
    resources: Router,
}

impl ActorContinuationWorker {
    pub(crate) fn new(
        dependencies: Arc<SetupDependencies>,
        commands: Arc<AgentCommandService>,
        resources: Router,
    ) -> Arc<Self> {
        Self::with_authority(dependencies.authority.clone(), commands, resources)
    }

    pub(super) fn with_authority(
        dependencies: Arc<SetupAuthority>,
        commands: Arc<AgentCommandService>,
        resources: Router,
    ) -> Arc<Self> {
        Arc::new(Self {
            dependencies,
            commands,
            resources,
        })
    }

    pub(crate) fn spawn(
        self: Arc<Self>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let holder = format!("actor-continuation-{}", uuid::Uuid::new_v4().simple());
            let mut tasks = tokio::task::JoinSet::new();
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                    _ = tick.tick() => {
                        let capacity = 4_usize.saturating_sub(tasks.len());
                        if capacity == 0 { continue; }
                        match self.dependencies.auth.pending_actor_wakeups(capacity).await {
                            Ok(rows) => for row in rows {
                                match self.dependencies.auth.claim_actor_wakeup(&row.grant_id, &row.notification_id, &holder, 120).await {
                                    Ok(Some(notification)) => {
                                        let worker = self.clone();
                                        tasks.spawn(async move { worker.run(notification).await; });
                                    }
                                    Ok(None) => {},
                                    Err(_) => tracing::warn!("login continuation claim will retry"),
                                }
                            },
                            Err(_) => tracing::warn!("login continuation discovery will retry"),
                        }
                    }
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            // Aborted attempts remain discoverable after their durable lease.
        })
    }

    pub(super) async fn run(&self, notification: ActorWakeupNotification) {
        let result =
            tokio::time::timeout(Duration::from_secs(90), self.deliver(&notification)).await;
        match result {
            Ok(Ok(receipt)) => {
                if self
                    .dependencies
                    .auth
                    .ack_actor_wakeup(&notification, &receipt)
                    .await
                    .is_err()
                {
                    tracing::warn!("login continuation receipt will be recovered");
                }
            }
            Ok(Err(BotError::Forbidden)) => {
                let _ = self
                    .dependencies
                    .auth
                    .cancel_actor_wakeup(&notification)
                    .await;
            }
            _ => {
                let _ = self
                    .dependencies
                    .auth
                    .release_actor_wakeup(&notification, 5)
                    .await;
                tracing::warn!("login continuation will retry its durable command and delivery");
            }
        }
    }

    pub(super) async fn authority(
        &self,
        notification: &ActorWakeupNotification,
    ) -> Result<(CallerContext, SecretValue), BotError> {
        let dep = &self.dependencies;
        let reference = format!(
            "cma1.cont.{}.{}",
            notification.grant.grant_id, notification.notification_id
        );
        let (actor, origin) = dep.auth.authorized_actor_continuation(&reference).await?;
        if actor != notification.actor || origin != notification.origin {
            return Err(BotError::Forbidden);
        }
        let grant = dep
            .auth
            .current_actor_grant(&notification.grant.grant_id, &notification.actor)
            .await?;
        if grant.grant_id != notification.grant.grant_id
            || grant.scope != crate::auth::actor_links::ActorGrantScope::OrdinaryCma
            || grant.subject != notification.grant.subject
            || grant.credential_incarnation != notification.grant.credential_incarnation
        {
            return Err(BotError::Forbidden);
        }
        let binding = dep
            .store
            .binding(&notification.actor.binding_id)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        if binding.generation != notification.origin.binding_generation
            || binding.binding_id != dep.binding
        {
            return Err(BotError::Forbidden);
        }
        let material = dep
            .custody
            .list(&binding.sandbox_id)
            .await
            .map_err(|_| BotError::Unavailable)?
            .into_iter()
            .find(|material| {
                material.owner == binding.owner
                    && format!("{:x}", Sha256::digest(material.value.expose().as_bytes()))
                        == binding.key_sha256
            })
            .ok_or(BotError::Forbidden)?;
        let actor = ExternalActor {
            binding_id: notification.actor.binding_id.clone(),
            channel_id: notification.actor.channel_id.clone(),
            conversation_id: notification.actor.conversation_id.clone(),
            actor_id: notification.actor.actor_id.clone(),
            platform: notification.actor.platform.clone(),
        };
        let (current, conversation, source) = dep
            .admitted_event(
                &actor,
                &notification.origin.source_event.event_id,
                &notification.origin.source_event.reference,
                material.value.expose(),
            )
            .await?;
        if current.generation != notification.origin.binding_generation
            || conversation.agent_id != notification.origin.agent_id
            || conversation.worktree_id != notification.origin.worktree_id
            || conversation.conversation_id != notification.origin.provider_conversation_id
            || source != notification.origin.source_event
        {
            return Err(BotError::Forbidden);
        }
        let auth = dep
            .auth
            .binding_request(material.value.expose().into())
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        let principal = auth.binding.ok_or(BotError::Forbidden)?;
        let context = CallerContext {
            credential_source: CredentialSource::Binding,
            caller: principal.caller(),
            credential: CredentialContext {
                issuer: "binding:local".into(),
                subject: principal.owner().clone(),
                credential_id: principal.key_sha256().into(),
            },
            actor: ResourceActor::Api {
                subject: principal.owner().clone(),
                delegation_id: None,
            },
            memberships: vec![],
        };
        Ok((context, material.value))
    }

    pub(super) async fn deliver(
        &self,
        notification: &ActorWakeupNotification,
    ) -> Result<String, BotError> {
        let (context, credential) = self.authority(notification).await?;
        // This is an internal notification, not a fabricated signed human event.
        // Its text is deterministic so crash recovery reuses the same admission.
        let command = AgentCommand::message(format!(
            "CMA notification: the user completed NyxID sign-in and explicitly authorized CMA to act for this conversation. Resume the user's pending request using your CMA skill. Read current state before mutations, and ask the user for any remaining choices. This notification does not authorize final bot creation or replace user confirmation. CMA will deliver your final answer to the original Telegram chat automatically; do not call replyToEvent for this notification. Use this CMA continuation capability in the event_ref field of CMA delegated/setup APIs (it cannot be used with gateway tools):\nevent_ref: cma1.cont.{}.{}",
            notification.grant.grant_id, notification.notification_id));
        let submission = CommandSubmission {
            agent_id: notification.origin.agent_id.clone(),
            command,
            idempotency_key: format!("actor-login:{}", notification.notification_id),
            expected_agent_revision: None,
            requested_model: None,
        };
        let admitted = self
            .commands
            .admit(
                &context,
                credential.expose(),
                CommandSubmissionSource::Api,
                submission,
            )
            .await
            .map_err(|_| BotError::Unavailable)?;
        let response_id = admitted.receipt.response_id.ok_or(BotError::Unavailable)?;
        loop {
            self.dependencies
                .auth
                .renew_actor_wakeup(notification, 120)
                .await?;
            let (context, credential) = self.authority(notification).await?;
            let mut request = Request::builder()
                .uri(format!("/api/v2/responses/{response_id}"))
                .body(Body::empty())
                .map_err(|_| BotError::Unavailable)?;
            request
                .extensions_mut()
                .insert(AuthenticatedSubject(context.caller.subject.clone()));
            request
                .extensions_mut()
                .insert(context.caller.authorization.clone());
            request.extensions_mut().insert(context);
            request
                .extensions_mut()
                .insert(DelegatedCredential::new(credential.expose().into()));
            let response = self
                .resources
                .clone()
                .oneshot(request)
                .await
                .map_err(|_| BotError::Unavailable)?;
            if !response.status().is_success() {
                return Err(BotError::Unavailable);
            }
            let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .map_err(|_| BotError::Unavailable)?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| BotError::Unavailable)?;
            match value["status"].as_str() {
                Some("completed" | "failed" | "incomplete") => {
                    let text = response_text(&value);
                    self.authority(notification).await?;
                    let receipt = self
                        .dependencies
                        .json(
                            self.dependencies
                                .http
                                .post(format!(
                                    "{}/v1/reply-targets/{}/messages",
                                    self.dependencies.gateway, notification.origin.reply_target_ref
                                ))
                                .bearer_auth(credential.expose())
                                .header(
                                    "idempotency-key",
                                    format!("actor-login:{}", notification.notification_id),
                                )
                                .json(&json!({"text":text})),
                        )
                        .await?;
                    if receipt["delivery"]["outcome"] != "confirmed" {
                        return Err(BotError::Unavailable);
                    }
                    return layer2_core::profiles::content_digest(&receipt)
                        .map_err(|_| BotError::Unavailable);
                }
                Some("queued" | "in_progress") => tokio::time::sleep(Duration::from_secs(2)).await,
                _ => return Err(BotError::Unavailable),
            }
        }
    }
}

fn response_text(response: &Value) -> String {
    let text = response["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|part| part["type"] == "output_text")
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        "You are signed in. I couldn't finish the follow-up request; please send another message to continue.".into()
    } else {
        let mut end = text.len().min(32 * 1024);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_delivery_uses_only_final_public_message_output() {
        let response = json!({"output":[{"type":"reasoning","content":[{"type":"output_text","text":"private"}]}, {"type":"message","content":[{"type":"output_text","text":"Choose personal or organization."}]}]});
        assert_eq!(response_text(&response), "Choose personal or organization.");
    }
}
