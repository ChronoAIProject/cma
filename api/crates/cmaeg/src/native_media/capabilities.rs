//! Eligibility for new uploads is separate from permission on a retained reference.
use super::{authority, store::Store};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    tools::{MessageEffectsCapabilities, ToolsState},
};
use cmaeg_core::{
    activity::Activity,
    ids::MessageRef,
    message::NativeMediaKind,
    problem::Problem,
    seal::{
        native_media_edit::NativeMediaDestination as Destination, EventRefPayload, ReplyAddress,
    },
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub caption: bool,
    pub content: bool,
    pub replacement_kinds: Vec<NativeMediaKind>,
    pub max_attachments: u8,
    pub max_bytes: u64,
    pub idempotency_key_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}
impl Capabilities {
    fn new(discord: bool, kinds: Vec<NativeMediaKind>, expires_at: Option<i64>) -> Self {
        Self {
            caption: true,
            content: discord,
            replacement_kinds: kinds,
            max_attachments: 1,
            max_bytes: crate::media::outbound::MAX_OUTBOUND_BYTES,
            idempotency_key_required: true,
            expires_at,
        }
    }
    pub(crate) fn context(
        state: &ToolsState,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: &Activity,
    ) -> Option<Self> {
        let client = state.native_media_edits.as_ref()?;
        let discord = match event.reply {
            ReplyAddress::Discord { .. } if client.discord_issuance => true,
            ReplyAddress::Telegram { .. } if client.telegram_issuance => false,
            _ => return None,
        };
        authority::custody(channel, event, activity)?;
        Some(Self::new(
            discord,
            vec![
                NativeMediaKind::Image,
                NativeMediaKind::File,
                NativeMediaKind::Audio,
                NativeMediaKind::Video,
            ],
            None,
        ))
    }
}
impl ToolsState {
    pub(crate) async fn native_media_capabilities(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
    ) -> Result<MessageEffectsCapabilities, Problem> {
        let (channel, message, _) = self
            .authorize_message(caller, reference, "nativeMediaCapabilities")
            .await?;
        let a = message
            .native_media_edit
            .as_ref()
            .ok_or_else(super::denied)?;
        let (original, current) = Store::new(self.resolver.record_client(), self.keys.clone())
            .load(&a.original_sha256)
            .await?;
        authority::check(&channel, &message, &original, caller)?;
        let enabled = self.native_media_edits.is_some() && a.revision == current.revision;
        let effects = (message.telegram_effect.is_some() && self.telegram_effects.is_some())
            || (message
                .discord_message
                .as_ref()
                .is_some_and(|a| a.effects.is_some())
                && self.discord_effects.is_some());
        let mut result = MessageEffectsCapabilities::new(effects, Some(a.expires_at));
        result.media_edit = enabled.then(|| {
            Capabilities::new(
                matches!(a.destination, Destination::Discord { .. }),
                vec![a.kind],
                Some(a.expires_at),
            )
        });
        Ok(result)
    }
}
