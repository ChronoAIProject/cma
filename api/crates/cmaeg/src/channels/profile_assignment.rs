//! Permanent, channel-version-fenced assignment of a partition to a provider revision.
use super::{definition::hash_secret, resolver::ResolvedChannel};
use crate::{
    connections::store::index_id,
    replicas::{
        store::{Store, Write},
        Error,
    },
};
use cmaeg_core::{
    channel::{ChannelDefinition, ProfileRevisionView},
    ids::{ConsumerId, ConversationId},
};
use serde::{Deserialize, Serialize};

const KIND: &str = "cmaeg_partition_profile_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Assignment {
    kind: String,
    id: String,
    owner: String,
    channel_id: String,
    record_id: String,
    consumer: String,
    conversation_id: String,
    revision: u64,
    binding_id: String,
    profile_digest: String,
    credential_fingerprint: String,
}

impl Assignment {
    pub(crate) fn view<'a>(
        &self,
        channel: &'a ChannelDefinition,
    ) -> Result<ProfileRevisionView<'a>, Error> {
        let view = channel.profile_revision(self.revision).ok_or(Error::Lost)?;
        if self.binding_id != view.provider.binding_id.as_str()
            || self.profile_digest != view.profile_digest
            || self.credential_fingerprint != hash_secret(view.credential.as_str())
        {
            return Err(Error::Lost);
        }
        Ok(view)
    }
}

/// The transport conversation is already derived from the canonical partition
/// key, channel and consumer. It also identifies legacy AM mappings whose
/// original event records have expired. A revision or credential change cannot
/// create a second assignment for that conversation.
pub(crate) async fn admit(
    store: &Store,
    current: &ResolvedChannel,
    consumer: &ConsumerId,
    conversation: &ConversationId,
) -> Result<Option<Assignment>, Error> {
    let channel = &current.definition;
    let Some(history) = channel.secrets.profile_revisions.as_ref() else {
        return Ok(None);
    };
    if !consumer.is_default() {
        return Ok(None);
    }
    let id = index_id(
        KIND,
        &[
            &channel.owner.subject,
            current.id.as_str(),
            channel.record_id.as_str(),
            consumer.as_str(),
            channel.provider.slug.as_str(),
            conversation.as_str(),
        ],
    );
    let active = channel
        .profile_revision(history.current_revision)
        .ok_or(Error::Corrupt)?;
    let candidate = Assignment {
        kind: KIND.into(),
        id: id.clone(),
        owner: channel.owner.subject.clone(),
        channel_id: current.id.to_string(),
        record_id: channel.record_id.to_string(),
        consumer: consumer.to_string(),
        conversation_id: conversation.to_string(),
        revision: active.revision,
        binding_id: active.provider.binding_id.to_string(),
        profile_digest: active.profile_digest.to_owned(),
        credential_fingerprint: hash_secret(active.credential.as_str()),
    };
    let validate = |assignment: Assignment| {
        if assignment.kind != candidate.kind
            || assignment.id != candidate.id
            || assignment.owner != candidate.owner
            || assignment.channel_id != candidate.channel_id
            || assignment.record_id != candidate.record_id
            || assignment.consumer != candidate.consumer
            || assignment.conversation_id != candidate.conversation_id
        {
            return Err(Error::Corrupt);
        }
        assignment.view(channel)?;
        Ok(Some(assignment))
    };
    match store.read::<Assignment>(&id, false).await? {
        Some((retained, 1)) => return validate(retained),
        Some(_) => return Err(Error::Corrupt),
        None => {}
    }
    // The Mongo channel guard writes the channel row and inserts the permanent
    // assignment in one transaction. Profile publication updates that row.
    let result = store
        .write(
            &id,
            &candidate.owner,
            &candidate,
            Write {
                channel: Some((channel.record_id.to_string(), current.version)),
                ..Default::default()
            },
        )
        .await;
    match result {
        Ok(1) => Ok(Some(candidate)),
        Ok(_) => Err(Error::Corrupt),
        Err(error) => match store.read::<Assignment>(&id, false).await? {
            Some((retained, 1)) => validate(retained),
            Some(_) => Err(Error::Corrupt),
            None => Err(error),
        },
    }
}

/// Project the pinned execution profile after validating its retained credential.
pub(crate) async fn execution_channel(
    resolver: &super::resolver::ChannelResolver,
    current: std::sync::Arc<ResolvedChannel>,
    consumer: &ConsumerId,
    conversation: &ConversationId,
) -> Result<std::sync::Arc<ResolvedChannel>, super::resolver::ResolveError> {
    use super::resolver::ResolveError;
    if current.definition.secrets.profile_revisions.is_none() || !consumer.is_default() {
        return Ok(current);
    }
    let store = resolver.profile_assignment_store()?;
    let assignment = admit(&store, &current, consumer, conversation)
        .await
        .map_err(|_| ResolveError::Unavailable)?
        .ok_or(ResolveError::InvalidDefinition)?;
    let view = assignment
        .view(&current.definition)
        .map_err(|_| ResolveError::InvalidDefinition)?;
    let mut execution = (*current).clone();
    execution.definition.provider = view.provider.clone();
    execution.definition.profile_digest = view.profile_digest.to_owned();
    execution.definition.profile = view.profile;
    execution.definition.secrets.provider_credential = Some(view.credential.clone());
    execution.definition.secrets.profile_revisions = None;
    Ok(std::sync::Arc::new(execution))
}

pub(crate) async fn pin_legacy(
    resolver: Option<&super::resolver::ChannelResolver>,
    request: &mut crate::ingress::launch::LaunchRequest,
) -> Result<(), cmaeg_core::problem::Problem> {
    use cmaeg_core::problem::{Problem, ProblemCode};
    if request
        .channel
        .definition
        .secrets
        .profile_revisions
        .is_none()
    {
        return Ok(());
    }
    let unavailable = || {
        Problem::new(
            ProblemCode::ProviderUnavailable,
            503,
            "partition profile assignment unavailable",
        )
    };
    let resolver = resolver.ok_or_else(unavailable)?;
    let current = resolver
        .resolve_current(request.channel.id.as_str())
        .await
        .map_err(|_| unavailable())?;
    if current.version != request.channel.version {
        return Err(unavailable());
    }
    let consumer = request
        .consumer
        .clone()
        .unwrap_or_else(ConsumerId::default_consumer);
    request.channel = execution_channel(resolver, current, &consumer, &request.conversation_id)
        .await
        .map_err(|_| unavailable())?;
    Ok(())
}
