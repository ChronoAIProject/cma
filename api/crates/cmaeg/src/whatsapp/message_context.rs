//! Encrypted original-message credential proof, using the existing interaction store.
//! A locator alone grants nothing: callers must also bind the committed original plan.
use crate::{
    channels::{definition::hash_secret, resolver::ResolvedChannel},
    ingress::{
        routing_store::{AdmittedPlan, EventIdentity},
        whatsapp::VerifiedWhatsappMessage,
    },
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    channel::Source,
    ids::RecordId,
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{Deserialize, Serialize};

/// Covers Meta's two-day revoke window, within the existing seven-day store bound
/// even when a valid provider timestamp is up to five minutes ahead.
pub(crate) const TTL: i64 = 3 * 86400;
#[derive(Debug, Clone, Copy)]
pub(crate) enum Error {
    Missing,
    Denied,
    Unavailable,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    kind: String,
    original: EventIdentity,
    sender: String,
    message: String,
    revision: String,
    facts: String,
    expires_at: i64,
}
fn revision(channel: &ResolvedChannel, source: &Source) -> Result<String, Error> {
    let credentials = channel
        .definition
        .secrets
        .whatsapp
        .as_ref()
        .ok_or(Error::Denied)?;
    credentials.validate().map_err(|_| Error::Denied)?;
    Ok(hash_secret(&cmaeg_core::profile::canonical_json(
        serde_json::json!({
            "source":source,"owner":channel.definition.owner.subject,
            "credentials":super::group_revision(source, credentials)
        }),
    )))
}
fn id(
    channel: &ResolvedChannel,
    source: &Source,
    sender: &str,
    message: &str,
) -> Result<String, Error> {
    let Source::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        ..
    } = source
    else {
        return Err(Error::Denied);
    };
    Ok(crate::connections::store::index_id(
        "whatsapp-message-context-v1",
        &[
            channel.definition.record_id.as_str(),
            app_id,
            waba_id,
            phone_number_id,
            sender,
            message,
        ],
    ))
}
pub(crate) fn opted_in(source: &Source) -> bool {
    matches!(
        source,
        Source::Whatsapp {
            message_events: true,
            ..
        }
    )
}
pub(crate) fn shape(activity: &Activity) -> bool {
    matches!(activity.source, SourceKind::Whatsapp { .. })
        && matches!(&activity.kind,
        ActivityKind::Event { ce, .. } if matches!(ce.r#type.as_str(), "cma.message.updated.v1" | "cma.message.deleted.v1"))
}
impl Context {
    pub(crate) fn capture(verified: &VerifiedWhatsappMessage, now: i64) -> Result<Self, Error> {
        let expires_at = verified.message.timestamp.saturating_add(TTL);
        if !opted_in(&verified.source)
            || verified.message.group_id.is_some()
            || expires_at <= now
            || verified.message.timestamp > now + 300
            || !super::webhook::change_originals(&verified.source, &verified.body)
                .is_ok_and(|messages| messages.contains(&verified.message))
        {
            return Err(Error::Denied);
        }
        Ok(Self {
            kind: "whatsapp_message_context_v1".into(),
            original: EventIdentity {
                owner: verified.channel.definition.owner.subject.clone(),
                channel_id: verified.channel.id.clone(),
                record_id: verified.channel.definition.record_id.clone(),
                event_id: super::webhook::event_id(&verified.source, &verified.message.id)
                    .map_err(|_| Error::Denied)?,
            },
            sender: verified.message.sender.clone(),
            message: verified.message.id.clone(),
            revision: revision(&verified.channel, &verified.source)?,
            facts: hash_secret(
                &serde_json::to_string(&verified.message).map_err(|_| Error::Denied)?,
            ),
            expires_at,
        })
    }
    /// Insert before first admission. Callers never mint proof for an existing
    /// plan, including a legacy original retried after credential rotation.
    pub(crate) async fn remember(
        &self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        channel: &ResolvedChannel,
        source: &Source,
    ) -> Result<(), Error> {
        let id = id(channel, source, &self.sender, &self.message)?;
        {
            let bytes = serde_json::to_vec(self).map_err(|_| Error::Unavailable)?;
            let blob = seal_interaction(keys, &id, &bytes).map_err(|_| Error::Unavailable)?;
            match records
                .clone()
                .interactions()
                .put_expiring(
                    &RecordId::new(&id).map_err(|_| Error::Unavailable)?,
                    &self.original.owner,
                    &blob,
                    None,
                    Some(self.expires_at),
                )
                .await
            {
                Ok(1) => return Ok(()),
                Err(RecordError::Conflict { .. }) => (),
                _ => return Err(Error::Unavailable),
            }
        }
        let saved = load(records, keys, &id).await?;
        if saved != *self {
            return Err(Error::Denied);
        }
        Ok(())
    }
}
async fn load(records: &RecordClient, keys: &MasterKeyRing, id: &str) -> Result<Context, Error> {
    let store = records.clone().interactions();
    let row = store
        .get(
            store
                .record_url(id)
                .map_err(|_| Error::Unavailable)?
                .as_str(),
        )
        .await
        .map_err(|error| match error {
            RecordError::NotFound => Error::Missing,
            _ => Error::Unavailable,
        })?;
    if row.version != 1 {
        return Err(Error::Denied);
    }
    let opened = open_interaction(keys, id, &row.blob).map_err(|_| Error::Denied)?;
    serde_json::from_slice(&opened.payload).map_err(|_| Error::Denied)
}
pub(crate) async fn check(
    records: &RecordClient,
    keys: &MasterKeyRing,
    channel: &ResolvedChannel,
    original: &AdmittedPlan,
    sender: &str,
    message: &str,
    now: i64,
) -> Result<(), Error> {
    let source = channel
        .definition
        .source_of(&original.activity.source)
        .ok_or(Error::Denied)?;
    let saved = load(records, keys, &id(channel, source, sender, message)?).await?;
    if !opted_in(source)
        || saved.kind != "whatsapp_message_context_v1"
        || saved.original != original.identity
        || saved.sender != sender
        || saved.message != message
        || saved.expires_at <= now
        || original.retained_until <= now
        || saved.revision != revision(channel, source)?
        || original.source != *source
        || original.identity.owner != channel.definition.owner.subject
        || original.identity.channel_id != channel.id
        || original.identity.record_id != channel.definition.record_id
    {
        return Err(Error::Denied);
    }
    let messages = super::webhook::change_originals(source, &original.source_body)
        .map_err(|_| Error::Denied)?;
    let original = messages
        .iter()
        .find(|value| {
            value.id == message
                && value.sender == sender
                && value.group_id.is_none()
                && matches!(
                    value.content,
                    super::webhook::Content::Text { .. } | super::webhook::Content::Media { .. }
                )
        })
        .ok_or(Error::Denied)?;
    if saved.facts != hash_secret(&serde_json::to_string(original).map_err(|_| Error::Denied)?) {
        return Err(Error::Denied);
    }
    Ok(())
}

pub(crate) async fn current(
    records: &RecordClient,
    keys: &std::sync::Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    plan: &AdmittedPlan,
) -> Result<(), Error> {
    let source = channel
        .definition
        .source_of(&plan.activity.source)
        .ok_or(Error::Denied)?;
    let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
        return Err(Error::Denied);
    };
    let message = ce
        .data
        .as_ref()
        .and_then(|data| data["message_id"].as_str())
        .ok_or(Error::Denied)?;
    let event_id = super::webhook::event_id(source, message).map_err(|_| Error::Denied)?;
    if ce
        .data
        .as_ref()
        .and_then(|data| data["original_event_id"].as_str())
        != Some(event_id.as_str())
    {
        return Err(Error::Denied);
    }
    let identity = EventIdentity {
        event_id,
        ..plan.identity.clone()
    };
    let original = crate::ingress::routing_store::PlanStore::new(records.clone(), keys.clone())
        .load(&identity)
        .await
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Denied)?;
    check(
        records,
        keys,
        channel,
        &original,
        &plan.activity.actor.id,
        message,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
    .await
}
