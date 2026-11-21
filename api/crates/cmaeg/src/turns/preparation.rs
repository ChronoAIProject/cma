//! Shared, prepare-only input construction for live and durable turn execution.
use super::{
    metadata, render_input,
    render_media::{self, Audience, RenderMedia},
};
use crate::{
    ingress::launch::LaunchRequest,
    media::{
        prepare::{prepare, PrepareConfig, Prepared},
        service::MediaService,
    },
    providers::EventContextSnapshot,
};
use serde_json::Value;
use std::{collections::BTreeMap, time::Instant};

pub(super) struct RenderedTurn {
    pub input: Vec<Value>,
    pub metadata: BTreeMap<String, String>,
    pub event_context: EventContextSnapshot,
}

pub(super) async fn prepare_or_empty(
    service: Option<&MediaService>,
    config: &PrepareConfig,
    turn: &LaunchRequest,
    audience: Audience,
    nonce: Option<String>,
    deadline: Instant,
) -> Prepared {
    match service {
        Some(service) if has_attachments(&turn.activity) => {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            prepare(service, turn, audience, config, nonce, deadline.into(), now).await
        }
        _ => Prepared {
            media: RenderMedia::empty(nonce),
            eager: false,
        },
    }
}

pub(super) fn render(
    turn: &LaunchRequest,
    media: &RenderMedia,
    audience: Audience,
    admitted_partition: Option<&str>,
) -> RenderedTurn {
    let metadata = match admitted_partition {
        Some(partition) => metadata::render_for_partition(turn, partition),
        None => metadata::render(turn),
    };
    RenderedTurn {
        input: render_input::render(&turn.activity, &turn.channel.definition, media, audience),
        metadata,
        event_context: EventContextSnapshot {
            schema_version: 1,
            channel_id: turn.channel.id.clone(),
            conversation_id: turn.conversation_id.clone(),
            activity: turn.activity.clone(),
            event_ref: turn.event_ref.clone(),
        },
    }
}

pub(super) fn has_attachments(activity: &cmaeg_core::activity::Activity) -> bool {
    !render_media::attachments(activity).is_empty()
}
