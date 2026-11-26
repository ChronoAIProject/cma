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

#[path = "profile_reset.rs"]
mod resets;
pub(crate) use resets::{reset_partition, ResetInput};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_operation: Option<String>,
    #[serde(default)]
    incarnation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_revision: Option<u64>,
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
async fn prepare(
    store: &Store,
    current: &ResolvedChannel,
    consumer: &ConsumerId,
    conversation: &ConversationId,
    allow_new: bool,
) -> Result<Option<(Assignment, Option<crate::records::mongo::ReplicaWrite>)>, Error> {
    let channel = &current.definition;
    let Some(history) = channel.secrets.profile_revisions.as_ref() else {
        return Ok(None);
    };
    if !consumer.is_default() {
        return Ok(None);
    }
    let active = channel
        .profile_revision(history.current_revision)
        .ok_or(Error::Corrupt)?;
    let id = index_id(
        KIND,
        &[
            &channel.owner.subject,
            current.id.as_str(),
            channel.record_id.as_str(),
            consumer.as_str(),
            active.provider.slug.as_str(),
            conversation.as_str(),
        ],
    );
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
        pending_operation: None,
        incarnation: 0,
        first_revision: Some(active.revision),
    };
    let validate = |assignment: Assignment, version| {
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
        if allow_new && assignment.pending_operation.is_some() {
            return Err(Error::Lost);
        }
        let write = store.prepare(
            &id,
            &assignment.owner,
            &assignment,
            Write {
                expected: Some(version),
                channel: Some((channel.record_id.to_string(), current.version)),
                ..Default::default()
            },
        )?;
        Ok(Some((assignment, Some(write))))
    };
    match store.read::<Assignment>(&id, false).await? {
        Some((retained, version)) => return validate(retained, version),
        None if !allow_new => return Err(Error::Lost),
        None => {}
    }
    let write = store.prepare(
        &id,
        &candidate.owner,
        &candidate,
        Write {
            channel: Some((channel.record_id.to_string(), current.version)),
            ..Default::default()
        },
    )?;
    Ok(Some((candidate, Some(write))))
}

pub(crate) struct Admission {
    pub channel_record_id: String,
    pub channel_version: u64,
    pub insert: Option<crate::records::mongo::ReplicaWrite>,
}

/// Project the pinned execution profile after validating its retained credential.
fn project(
    current: &ResolvedChannel,
    assignment: &Assignment,
) -> Result<std::sync::Arc<ResolvedChannel>, Error> {
    let view = assignment.view(&current.definition)?;
    project_view(current, view)
}

fn project_view(
    current: &ResolvedChannel,
    view: ProfileRevisionView<'_>,
) -> Result<std::sync::Arc<ResolvedChannel>, Error> {
    let mut execution = current.clone();
    let fingerprint = hash_secret(view.credential.as_str());
    super::definition::project_default_profile(&mut execution.definition, view);
    let key_hash = super::resolver::decode_key_hash(&fingerprint).map_err(|_| Error::Corrupt)?;
    execution.definition.secrets.agent_key_hash = fingerprint;
    execution.agent_key_hash = key_hash;
    if let Some((_, hash)) = execution
        .consumer_keys
        .iter_mut()
        .find(|(consumer, _)| consumer.is_default())
    {
        *hash = key_hash;
    } else {
        return Err(Error::Corrupt);
    }
    execution.definition.secrets.profile_revisions = None;
    Ok(std::sync::Arc::new(execution))
}

/// The event root pins the original provider across subsequent revisions and
/// resets. A pre-revision sealed reference falls back to the original binding.
pub(crate) async fn event_channel(
    resolver: &super::resolver::ChannelResolver,
    current: std::sync::Arc<ResolvedChannel>,
    event_id: &cmaeg_core::ids::EventId,
    issued_to: Option<&ConsumerId>,
    conversation: Option<&ConversationId>,
) -> Result<std::sync::Arc<ResolvedChannel>, super::resolver::ResolveError> {
    use super::resolver::ResolveError;
    if current.definition.secrets.profile_revisions.is_none()
        || issued_to.is_some_and(|id| !id.is_default())
    {
        return Ok(current);
    }
    let store = resolver.profile_assignment_store()?;
    let plans = crate::ingress::routing_store::PlanStore::new(store.records, store.keys);
    let identity = crate::ingress::routing_store::EventIdentity {
        owner: current.definition.owner.subject.clone(),
        channel_id: current.id.clone(),
        record_id: current.definition.record_id.clone(),
        event_id: event_id.clone(),
    };
    let history = current
        .definition
        .secrets
        .profile_revisions
        .as_ref()
        .ok_or(ResolveError::InvalidDefinition)?;
    let revision = if let Some(plan) = plans
        .load(&identity)
        .await
        .map_err(|_| ResolveError::Unavailable)?
    {
        let target = plan
            .targets
            .iter()
            .find(|target| target.launch.consumer.is_default())
            .ok_or(ResolveError::InvalidDefinition)?;
        if conversation.is_some_and(|id| *id != target.launch.event.conversation_id) {
            return Err(ResolveError::InvalidDefinition);
        }
        let consumer = current
            .consumer(Some(&target.launch.consumer))
            .ok_or(ResolveError::InvalidDefinition)?;
        if &target.launch.provider == consumer.provider {
            history.current_revision
        } else {
            history
                .retained
                .iter()
                .find(|revision| revision.provider == target.launch.provider)
                .ok_or(ResolveError::InvalidDefinition)?
                .revision
        }
    } else {
        // Only a retained original partition assignment can authorize a
        // legacy event without a durable Gateway admission root.
        let conversation = conversation.ok_or(ResolveError::InvalidDefinition)?;
        let store = resolver.profile_assignment_store()?;
        let (assignment, _) = prepare(
            &store,
            &current,
            &ConsumerId::default_consumer(),
            conversation,
            false,
        )
        .await
        .map_err(|_| ResolveError::Unavailable)?
        .ok_or(ResolveError::InvalidDefinition)?;
        if assignment.first_revision.unwrap_or(assignment.revision) != 1 {
            return Err(ResolveError::InvalidDefinition);
        }
        1
    };
    let view = current
        .definition
        .profile_revision(revision)
        .ok_or(ResolveError::InvalidDefinition)?;
    project_view(&current, view).map_err(|_| ResolveError::InvalidDefinition)
}

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
    let (assignment, _) = prepare(&store, &current, consumer, conversation, false)
        .await
        .map_err(|_| ResolveError::Unavailable)?
        .ok_or(ResolveError::InvalidDefinition)?;
    project(&current, &assignment).map_err(|_| ResolveError::InvalidDefinition)
}

/// Even an event using the original binding must serialize its event root
/// with the first baseline. Retained revisions continue to require the fence.
pub(crate) fn requires_guard(definition: &ChannelDefinition, writes_enabled: bool) -> bool {
    definition.secrets.profile_revisions.is_some()
        || (writes_enabled && super::profile_revision::can_baseline(definition))
}

pub(crate) fn channel_guard(channel: &ResolvedChannel) -> Admission {
    Admission {
        channel_record_id: channel.definition.record_id.to_string(),
        channel_version: channel.version,
        insert: None,
    }
}

pub(crate) async fn prepare_admission(
    resolver: Option<&super::resolver::ChannelResolver>,
    request: &mut crate::ingress::launch::LaunchRequest,
    writes_enabled: bool,
) -> Result<Option<Admission>, cmaeg_core::problem::Problem> {
    use cmaeg_core::problem::{Problem, ProblemCode};
    if !requires_guard(&request.channel.definition, writes_enabled) {
        return Ok(None);
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
    if current.version != request.channel.version || current.definition.mutation.is_some() {
        return Err(unavailable());
    }
    let consumer = request
        .consumer
        .clone()
        .unwrap_or_else(ConsumerId::default_consumer);
    let store = resolver
        .profile_assignment_store()
        .map_err(|_| unavailable())?;
    let Some((assignment, insert)) =
        prepare(&store, &current, &consumer, &request.conversation_id, true)
            .await
            .map_err(|_| unavailable())?
    else {
        return Ok(Some(channel_guard(&current)));
    };
    request.channel = project(&current, &assignment).map_err(|_| unavailable())?;
    Ok(Some(Admission {
        channel_record_id: current.definition.record_id.to_string(),
        channel_version: current.version,
        insert,
    }))
}

/// Baseline insertion is authorized by an already admitted provider mapping or
/// immutable event. It never creates provider work.
pub(super) async fn baseline(
    store: &Store,
    current: &ResolvedChannel,
    conversation: &ConversationId,
) -> Result<(), Error> {
    let consumer = ConsumerId::default_consumer();
    let Some((assignment, write)) = prepare(store, current, &consumer, conversation, true).await?
    else {
        return Err(Error::Corrupt);
    };
    if let Some(write) = write.filter(|value| value.expected.is_none()) {
        if store.batch(&[write]).await.is_err() {
            let Some((retained, Some(guard))) =
                prepare(store, current, &consumer, conversation, false).await?
            else {
                return Err(Error::Lost);
            };
            if retained != assignment || guard.expected.is_none() {
                return Err(Error::Lost);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "profile_assignment_tests.rs"]
mod tests;
