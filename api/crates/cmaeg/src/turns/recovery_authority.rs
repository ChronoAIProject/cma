use std::{fmt, sync::Arc, time::Instant};

use cmaeg_core::{
    activity::SourceKind,
    channel::{Secret, Sink},
    ids::ConsumerId,
};
use futures_util::future::{ready, BoxFuture};

use crate::{
    channels::{
        profile_assignment,
        resolver::{ChannelResolver, ResolvedChannel},
    },
    delivery::gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate},
    ingress::launch::LaunchRequest,
};

pub trait RecoveryChannelAuthority: Send + Sync {
    fn current<'a>(
        &'a self,
        turn: &'a LaunchRequest,
    ) -> BoxFuture<'a, Option<Arc<ResolvedChannel>>>;
}

impl RecoveryChannelAuthority for ChannelResolver {
    fn current<'a>(
        &'a self,
        turn: &'a LaunchRequest,
    ) -> BoxFuture<'a, Option<Arc<ResolvedChannel>>> {
        Box::pin(async move {
            let current = self.resolve_current(turn.channel.id.as_str()).await.ok()?;
            let consumer = turn
                .consumer
                .clone()
                .unwrap_or_else(ConsumerId::default_consumer);
            profile_assignment::execution_channel(self, current, &consumer, &turn.conversation_id)
                .await
                .ok()
        })
    }
}

pub struct RecoveryAuthority {
    authority: Arc<dyn RecoveryChannelAuthority>,
    turn: Arc<LaunchRequest>,
    pub(super) deadline: Instant,
}

impl fmt::Debug for RecoveryAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecoveryAuthority")
            .finish_non_exhaustive()
    }
}

impl RecoveryAuthority {
    pub(super) fn new(
        authority: Arc<dyn RecoveryChannelAuthority>,
        turn: Arc<LaunchRequest>,
        deadline: Instant,
    ) -> Option<Self> {
        matches!(turn.activity.source, SourceKind::Custom).then_some(Self {
            authority,
            turn,
            deadline,
        })
    }

    pub(crate) async fn current(&self) -> Option<Arc<ResolvedChannel>> {
        let current = self.authority.current(&self.turn).await?;
        let original = &self.turn.channel;
        let source = &self.turn.activity.source;
        let original_source = original.definition.source_of(source)?;
        let consumer: Option<&ConsumerId> = self.turn.consumer.as_ref();
        let before = original.definition.consumer(consumer)?;
        let after = current.definition.consumer(consumer)?;
        if current.id != original.id
            || current.definition.record_id != original.definition.record_id
            || current.definition.owner != original.definition.owner
            || current.definition.mutation.is_some()
            || current.definition.source_of(source) != Some(original_source)
            || after.pending
            || before.id != after.id
            || before.provider.slug != after.provider.slug
            || before.provider.binding_id != after.provider.binding_id
            || before.provider.target != after.provider.target
            || before.credential != after.credential
            || after.credential.is_none()
        {
            return None;
        }
        Some(current)
    }

    pub(crate) async fn credential(&self) -> Option<Secret> {
        self.current()
            .await?
            .definition
            .consumer(self.turn.consumer.as_ref())?
            .credential
            .cloned()
    }

    pub(crate) async fn permits_sync(&self) -> bool {
        self.turn
            .channel
            .definition
            .reply
            .sinks
            .contains(&Sink::Sync {})
            && self
                .current()
                .await
                .is_some_and(|current| current.definition.reply.sinks.contains(&Sink::Sync {}))
    }
}

impl SendGate for RecoveryAuthority {
    fn enter_route<'a>(
        &'a self,
        _route: &'a str,
    ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
        Box::pin(ready(Ok(RouteTicket::empty())))
    }

    fn commit(&self) -> bool {
        true
    }

    fn attempt(&self) -> BoxFuture<'_, Result<AttemptPermit, GateRefusal>> {
        Box::pin(ready(Ok(AttemptPermit::empty())))
    }

    fn progress(&self) {}

    fn authorize_webhook<'a>(&'a self, url: &'a str, secret: &'a Secret) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let Some(current) = self.current().await else {
                return false;
            };
            let original = &self.turn.channel.definition;
            original.reply.sinks.iter().any(|sink| {
                let Sink::ResultWebhook {
                    url: original_url,
                    hmac_key_id,
                } = sink
                else {
                    return false;
                };
                original_url == url
                    && current.definition.reply.sinks.contains(sink)
                    && original
                        .secrets
                        .hmac_secrets
                        .iter()
                        .any(|(id, key)| id == hmac_key_id && key == secret)
                    && current
                        .definition
                        .secrets
                        .hmac_secrets
                        .iter()
                        .any(|(id, key)| id == hmac_key_id && key == secret)
            })
        })
    }
}
