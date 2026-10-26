//! Immutable routing recognition must not depend on mutable channel ownership
//! or routing configuration: revocation can never downgrade to legacy sends.
use super::{model::delivery_event, storage, ExecutionError, Journal};
use crate::ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget};
use cmaeg_core::seal::{EventRefPayload, ReplyAddress};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplyBinding {
    kind: String,
    pub identity: EventIdentity,
    pub target: String,
    pub root: String,
    pub request_hash: String,
    retained_until: i64,
    event_hash: String,
}
impl Journal {
    pub(super) async fn bind_tool_reply(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), ExecutionError> {
        let event = delivery_event(plan, target);
        let binding = ReplyBinding {
            kind: "ordinary_route_reply_binding_v1".into(),
            identity: plan.identity.clone(),
            target: target.id.clone(),
            root: plan.admission_id.clone(),
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
            event_hash: event_hash(&event)?,
        };
        self.write(
            &binding_id(&event),
            &plan.identity.owner,
            plan.retained_until,
            &binding,
            None,
        )
        .await?;
        Ok(())
    }
    pub(super) async fn tool_reply_binding(
        &self,
        event: &EventRefPayload,
    ) -> Result<Option<ReplyBinding>, ExecutionError> {
        let Some((binding, version)) = self.read::<ReplyBinding>(&binding_id(event)).await? else {
            return Ok(None);
        };
        if version != 1
            || binding.kind != "ordinary_route_reply_binding_v1"
            || binding.identity.channel_id != event.channel_id
            || binding.identity.record_id != event.record_id
            || binding.identity.event_id != event.event_id
            || binding.event_hash != event_hash(event)?
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(Some(binding))
    }
}
fn binding_id(event: &EventRefPayload) -> String {
    storage::identifier(&(
        "ordinary-route-reply-binding-v1",
        &event.channel_id,
        &event.record_id,
        &event.event_id,
        &event.consumer,
    ))
}
fn event_hash(event: &EventRefPayload) -> Result<String, ExecutionError> {
    let mut event = event.clone();
    // A delayed capability intentionally drops the single-use relay token.
    if let ReplyAddress::NyxidRelay {
        reply_token,
        reply_token_expires_at,
        ..
    } = &mut event.reply
    {
        *reply_token = cmaeg_core::channel::Secret::new("");
        *reply_token_expires_at = 1;
    }
    Ok(storage::hash(&storage::bytes(&event)?))
}
