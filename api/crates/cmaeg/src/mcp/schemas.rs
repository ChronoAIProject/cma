//! Routed-only components must never enter the legacy tool document's $defs.
use super::tools::{EventArguments, UpdateArguments};
use cmaeg_core::{channel::ReplyChoice, message::OutboundMessage, seal::EventRef};
use serde::Deserialize;
use utoipa::{OpenApi, ToSchema};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutedReplyArguments {
    pub event_ref: EventRef,
    pub message: OutboundMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<ReplyChoice>,
}

#[derive(OpenApi)]
#[openapi(components(schemas(EventArguments, RoutedReplyArguments, UpdateArguments)))]
pub(super) struct RoutedSchemas;
