//! Current channel, source, consumer and native-control authority checks.
use super::ExecutionError;
use crate::{
    channels::resolver::{ChannelResolver, ResolveError, ResolvedChannel},
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
};
use futures_util::future::BoxFuture;
use std::sync::Arc;

pub trait Authority: Send + Sync {
    /// Must bypass process caches and authenticate the current sealed definition.
    fn current<'a>(
        &'a self,
        identity: &'a EventIdentity,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>>;
    fn assigned_channel<'a>(
        &'a self,
        current: Arc<ResolvedChannel>,
        _target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>> {
        Box::pin(async move {
            if current.definition.secrets.profile_revisions.is_some() {
                Err(ExecutionError::AuthorityChanged)
            } else {
                Ok(current)
            }
        })
    }
    fn native_control<'a>(
        &'a self,
        _current: &'a ResolvedChannel,
        plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async move {
            if native_lark_control(plan)
                || crate::whatsapp::message_context::shape(&plan.activity)
                || crate::interactions::whatsapp_flow::shape(&plan.activity)
            {
                Err(ExecutionError::AuthorityChanged)
            } else {
                Ok(())
            }
        })
    }
}
impl Authority for ChannelResolver {
    fn assigned_channel<'a>(
        &'a self,
        current: Arc<ResolvedChannel>,
        target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>> {
        Box::pin(async move {
            crate::channels::profile_assignment::execution_channel(
                self,
                current,
                &target.launch.consumer,
                &target.launch.event.conversation_id,
            )
            .await
            .map_err(|_| ExecutionError::AuthorityChanged)
        })
    }
    fn native_control<'a>(
        &'a self,
        current: &'a ResolvedChannel,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async move {
            if native_lark_control(plan) {
                self.native_lark_control_current(current, &plan.activity, &target.launch.event)
                    .await
                    .map_err(|e| match e {
                        crate::interactions::Error::Denied => ExecutionError::AuthorityChanged,
                        crate::interactions::Error::Unavailable => ExecutionError::Unavailable,
                    })?;
            }
            if crate::interactions::whatsapp_flow::shape(&plan.activity) {
                self.whatsapp_flow_current(current, &plan.activity, &target.launch.event)
                    .await
                    .map_err(|e| match e {
                        crate::interactions::Error::Denied => ExecutionError::AuthorityChanged,
                        crate::interactions::Error::Unavailable => ExecutionError::Unavailable,
                    })?;
            }
            if crate::whatsapp::message_context::shape(&plan.activity) {
                self.whatsapp_message_change_current(current, plan)
                    .await
                    .map_err(|error| match error {
                        crate::whatsapp::message_context::Error::Unavailable => {
                            ExecutionError::Unavailable
                        }
                        _ => ExecutionError::AuthorityChanged,
                    })?;
            }
            Ok(())
        })
    }
    fn current<'a>(
        &'a self,
        identity: &'a EventIdentity,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>> {
        Box::pin(async move {
            self.resolve_current(identity.channel_id.as_str())
                .await
                .map_err(|error| match error {
                    ResolveError::Unavailable | ResolveError::RateLimited => {
                        ExecutionError::Unavailable
                    }
                    _ => ExecutionError::AuthorityChanged,
                })
        })
    }
}

pub(super) async fn current_target(
    authority: &dyn Authority,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<Arc<ResolvedChannel>, ExecutionError> {
    let current = authority.current(&plan.identity).await?;
    let current = authority.assigned_channel(current, target).await?;
    validate_current(&current, plan, target)?;
    authority.native_control(&current, plan, target).await?;
    Ok(current)
}

pub(super) fn validate_current(
    current: &ResolvedChannel,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<(), ExecutionError> {
    if matches!(
        target.launch.event.reply,
        cmaeg_core::seal::ReplyAddress::NativeLark(_)
    ) && crate::native_lark::credentials(&current.definition, &target.launch.event.reply)
        .is_none()
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    if matches!(
        target.launch.event.reply,
        cmaeg_core::seal::ReplyAddress::WhatsappGroup(_)
    ) && crate::whatsapp::credentials(&current.definition, &target.launch.event.reply).is_none()
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    let consumer = current
        .definition
        .consumer(Some(&target.launch.consumer))
        .ok_or(ExecutionError::AuthorityChanged)?;
    if current.id != plan.identity.channel_id
        || current.pointer.record_id != plan.identity.record_id
        || current.definition.record_id != plan.identity.record_id
        || current.definition.owner.subject != plan.identity.owner
        || (current.definition.routing.is_none() && !plan.is_compatibility())
        || !current
            .definition
            .source_of(&plan.activity.source)
            .is_some_and(|source| {
                if matches!(source, cmaeg_core::channel::Source::Whatsapp { .. })
                    && !crate::interactions::whatsapp_flow::shape(&plan.activity)
                    && !crate::whatsapp::message_context::shape(&plan.activity)
                {
                    crate::whatsapp::legacy_source(source)
                        == crate::whatsapp::legacy_source(&plan.source)
                } else if matches!(source, cmaeg_core::channel::Source::Telegram { .. }) {
                    use crate::ingress::telegram::member_joins;
                    crate::telegram::messaging_source(source)
                        == crate::telegram::messaging_source(&plan.source)
                        && (!member_joins::is_member_join(&plan.activity)
                            || member_joins::group_opted_in(source, &plan.activity.conversation.id))
                } else if matches!(
                    source,
                    cmaeg_core::channel::Source::NyxidRelay {
                        platform: cmaeg_core::channel::Platform::Telegram,
                        ..
                    }
                ) {
                    use crate::ingress::telegram::member_joins;
                    member_joins::relay_messaging_source(source)
                        == member_joins::relay_messaging_source(&plan.source)
                        && (!member_joins::is_member_join(&plan.activity)
                            || member_joins::group_opted_in(source, &plan.activity.conversation.id))
                } else {
                    crate::native_lark::same_messaging_source(source, &plan.source)
                }
            })
        || current
            .definition
            .creation
            .as_ref()
            .is_some_and(|creation| !creation.ready)
        || consumer.pending
        || consumer.credential.is_none()
        || consumer.provider != &target.launch.provider
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    Ok(())
}

fn native_lark_control(plan: &AdmittedPlan) -> bool {
    matches!(
        plan.activity.source,
        cmaeg_core::activity::SourceKind::NativeLark(_)
    ) && matches!(
        plan.activity.kind,
        cmaeg_core::activity::ActivityKind::Interaction(_)
    )
}

#[cfg(test)]
mod tests {
    use super::{super::test_support::*, *};
    use cmaeg_core::{
        activity::{ActorKind, ConversationKind},
        channel::Source,
    };
    use serde_json::json;

    #[tokio::test]
    async fn pending_telegram_member_join_requires_current_consent_for_the_original_group() {
        let mut activity = fixture::message("");
        activity.kind = serde_json::from_value(json!({
            "type": "event",
            "ce": {
                "specversion": "1.0", "id": "event-1", "source": "telegram:bot:77",
                "type": "telegram.group_members_joined", "data": {"members": []},
            },
        }))
        .unwrap();
        activity.actor.kind = ActorKind::System;
        activity.actor.id = "telegram:system".into();
        activity.conversation.kind = ConversationKind::Group;
        activity.conversation.id = "-100".into();
        let request = fixture::request(
            |value| {
                value["sources"][0]["message_events"] = true.into();
                value["sources"][0]["member_join_chat_ids"] = json!(["-100", "-101"]);
                value["sources"][0]["admission"] = json!({
                    "type": "scoped", "senders": {"type": "open"}, "groups": "all",
                    "chats": {"type": "allowlist", "ids": ["-100", "-101"]},
                });
            },
            activity,
        );
        let setup = Setup::with_request(supported(), request).await;
        let mut current = setup.current.channel.lock().unwrap().clone();
        assert!(validate_current(&current, &setup.plan, setup.target()).is_ok());
        let current = Arc::make_mut(&mut current);
        let Source::Telegram {
            member_join_chat_ids,
            ..
        } = &mut current.definition.sources[0]
        else {
            panic!()
        };
        member_join_chat_ids.retain(|chat| chat != "-101");
        assert!(validate_current(current, &setup.plan, setup.target()).is_ok());
        let Source::Telegram {
            member_join_chat_ids,
            ..
        } = &mut current.definition.sources[0]
        else {
            panic!()
        };
        member_join_chat_ids.clear();
        assert!(matches!(
            validate_current(current, &setup.plan, setup.target()),
            Err(ExecutionError::AuthorityChanged)
        ));
    }
}
