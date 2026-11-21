//! Native resource custody is checked again before derivatives and every API read.
use super::FetchRequest;
use crate::{
    channels::resolver::{ChannelResolver, ResolveError, ResolvedChannel},
    media::FetchError,
};
use cmaeg_core::{
    activity::{ActivityKind, ActorKind, AttachmentSource, SourceKind},
    channel::Source,
    seal::ReplyAddress,
};
use std::sync::Arc;
use tokio::time::timeout_at;

pub(crate) fn configured(
    channel: &ResolvedChannel,
    source: &SourceKind,
    locator: &AttachmentSource,
) -> bool {
    let (
        SourceKind::NativeLark(identity),
        AttachmentSource::NativeLark {
            source_revision, ..
        },
    ) = (source, locator)
    else {
        return false;
    };
    let Some(credentials) = channel.definition.secrets.native_lark.as_ref() else {
        return false;
    };
    channel.definition.sources.iter().any(|s| matches!(s, Source::NativeLark(s)
        if &s.identity == identity && crate::native_lark::revision(s, credentials) == *source_revision))
}

pub(crate) fn authorize(request: &FetchRequest<'_>) -> Result<(), FetchError> {
    let (
        SourceKind::NativeLark(identity),
        ReplyAddress::NativeLark(reply),
        Some(
            locator @ AttachmentSource::NativeLark {
                message_id,
                source_revision,
                ..
            },
        ),
        ActivityKind::Message { attachments, .. },
    ) = (
        &request.activity.source,
        &request.event.reply,
        request.locator,
        &request.activity.kind,
    )
    else {
        return Err(FetchError::NotRetrievable);
    };
    if &reply.identity != identity
        || reply.message_id != *message_id
        || reply.revision != *source_revision
        || reply.chat_id != request.activity.conversation.id
        || reply.thread_id != request.activity.conversation.thread_id
        || request.activity.actor.kind != ActorKind::Human
        || request.event.channel_id != request.channel.id
        || request.event.record_id != request.channel.pointer.record_id
        || request.event.event_id != request.activity.event_id
        || !request.event.verifies_activity(request.activity)
        || !attachments
            .iter()
            .any(|a| a.source.as_ref() == Some(locator) || a.thumbnail.as_ref() == Some(locator))
        || !configured(request.channel, &request.activity.source, locator)
        || crate::native_lark::credentials(&request.channel.definition, &request.event.reply)
            .is_none()
    {
        return Err(FetchError::Unauthorized);
    }
    Ok(())
}

pub(crate) async fn refresh(
    resolver: &ChannelResolver,
    request: &FetchRequest<'_>,
) -> Result<Arc<ResolvedChannel>, FetchError> {
    authorize(request)?;
    let current = timeout_at(
        request.deadline,
        resolver.resolve_current(request.channel.id.as_str()),
    )
    .await
    .map_err(|_| FetchError::TimedOut)?
    .map_err(|e| match e {
        ResolveError::Unavailable => FetchError::Upstream,
        ResolveError::RateLimited => FetchError::RateLimited,
        _ => FetchError::Unauthorized,
    })?;
    let before = request
        .channel
        .consumer(request.event.consumer.as_ref())
        .ok_or(FetchError::Unauthorized)?;
    let after = current
        .consumer(request.event.consumer.as_ref())
        .ok_or(FetchError::Unauthorized)?;
    if current.definition.owner != request.channel.definition.owner
        || after.pending
        || after.agent_key_hash != before.agent_key_hash
        || after.provider != before.provider
    {
        return Err(FetchError::Unauthorized);
    }
    authorize(&FetchRequest {
        channel: &current,
        ..*request
    })?;
    Ok(current)
}
