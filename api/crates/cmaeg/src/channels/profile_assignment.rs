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
        Ok(Some((assignment, None)))
    };
    match store.read::<Assignment>(&id, false).await? {
        Some((retained, 1)) => return validate(retained),
        Some(_) => return Err(Error::Corrupt),
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
    execution.definition.provider = view.provider.clone();
    execution.definition.profile_digest = view.profile_digest.to_owned();
    execution.definition.profile = view.profile;
    execution.definition.secrets.provider_credential = Some(view.credential.clone());
    let fingerprint = hash_secret(view.credential.as_str());
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
        if target.launch.provider == current.definition.provider {
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
        if assignment.revision != 1 {
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

pub(crate) async fn prepare_admission(
    resolver: Option<&super::resolver::ChannelResolver>,
    request: &mut crate::ingress::launch::LaunchRequest,
) -> Result<Option<Admission>, cmaeg_core::problem::Problem> {
    use cmaeg_core::problem::{Problem, ProblemCode};
    if request
        .channel
        .definition
        .secrets
        .profile_revisions
        .is_none()
    {
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
        return Ok(None);
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
    if let Some(write) = write {
        if store.batch(&[write]).await.is_err() {
            let Some((retained, None)) =
                prepare(store, current, &consumer, conversation, false).await?
            else {
                return Err(Error::Lost);
            };
            if retained != assignment {
                return Err(Error::Lost);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replicas::mongo_fixture::Fixture;
    use cmaeg_core::{channel::ProfileRevisions, seal::seal_blob};

    #[tokio::test]
    #[ignore = "requires authenticated disposable Mongo"]
    async fn first_assignment_commits_only_with_the_accepted_event_and_channel_version() {
        let fixture = Fixture::new().await;
        let left = fixture.records("https://fixture.example").await;
        let right = fixture.records("https://fixture.example").await;
        let store = Store::new(left.clone(), fixture.keys()).unwrap();
        let peer = Store::new(right.clone(), fixture.keys()).unwrap();
        let request = crate::ingress::routing_plan::fixture::request(
            |_| {},
            crate::ingress::routing_plan::fixture::message("assignment"),
        );
        let mut channel = (*request.channel).clone();
        channel.definition.secrets.profile_revisions = Some(ProfileRevisions {
            current_revision: 1,
            baseline_complete: true,
            last_publication: None,
            retained: Vec::new(),
        });
        channel.definition.validate().unwrap();
        let blob = seal_blob(
            &fixture.keys(),
            &serde_json::to_vec(&channel.definition.to_blob_json()).unwrap(),
        )
        .unwrap();
        channel.version = left
            .put(
                &channel.definition.record_id,
                &channel.definition.owner.subject,
                &blob,
                None,
            )
            .await
            .unwrap();
        let consumer = ConsumerId::default_consumer();
        let conversation = &request.conversation_id;
        let (candidate, write) = prepare(&store, &channel, &consumer, conversation, true)
            .await
            .unwrap()
            .unwrap();
        let id = candidate.id.clone();
        assert!(peer.read::<Assignment>(&id, false).await.unwrap().is_none());
        let stale = Admission {
            channel_record_id: channel.definition.record_id.to_string(),
            channel_version: channel.version + 1,
            insert: write,
        };
        let expiry = time::OffsetDateTime::now_utc().unix_timestamp() + 600;
        assert!(left
            .put_profile_admission(
                "fixture-refused",
                &channel.definition.owner.subject,
                b"fixture",
                expiry,
                &stale,
                &[],
                None
            )
            .await
            .is_err());
        assert!(peer.read::<Assignment>(&id, false).await.unwrap().is_none());
        let (_, first) = prepare(&store, &channel, &consumer, conversation, true)
            .await
            .unwrap()
            .unwrap();
        let (_, second) = prepare(&peer, &channel, &consumer, conversation, true)
            .await
            .unwrap()
            .unwrap();
        let admission = |insert| Admission {
            channel_record_id: channel.definition.record_id.to_string(),
            channel_version: channel.version,
            insert,
        };
        let a = admission(first);
        let b = admission(second);
        let owner = &channel.definition.owner.subject;
        let (one, two) = tokio::join!(
            left.put_profile_admission("fixture-first", owner, b"fixture", expiry, &a, &[], None),
            right.put_profile_admission("fixture-second", owner, b"fixture", expiry, &b, &[], None)
        );
        assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
        let (retained, version) = peer.read::<Assignment>(&id, false).await.unwrap().unwrap();
        assert_eq!(retained, candidate);
        assert_eq!(version, 1);
        let (again, insert) = prepare(&peer, &channel, &consumer, conversation, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again, retained);
        assert!(insert.is_none());
        fixture.cleanup().await;
    }
}
