use super::{native_lark, native_lark_authority, whatsapp::ReceivedMessageContext, ToolsState};
use crate::{auth::AgentCaller, channels::resolver::ResolvedChannel, tools::Capabilities};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    seal::{EventRefPayload, ReplyAddress},
};

impl ToolsState {
    pub(super) async fn native_lark_received_context(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        activity: &Activity,
        verified: bool,
    ) -> Option<ReceivedMessageContext> {
        if !verified || !self.native_lark_effects.as_ref()?.received_issuance_enabled {
            return None;
        }
        let channel = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            crate::turns::routed::current_received_message(
                self.resolver.record_client(),
                self.keys.clone(),
                &self.resolver,
                caller,
                event,
                activity,
            ),
        )
        .await
        .ok()?
        .ok()?;
        let message_ref = native_lark_authority::received(&channel, event, activity, &self.keys)?;
        let message = message_ref.open(&self.keys).ok()?.payload;
        let effects = native_lark::capabilities(&message, &channel, caller, true).ok()?;
        Some(ReceivedMessageContext {
            message_ref,
            effects,
        })
    }
}
pub(crate) fn project(
    state: &ToolsState,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
    verified: bool,
    capabilities: &mut Capabilities,
) {
    let (ReplyAddress::NativeLark(reply), SourceKind::NativeLark(identity)) =
        (&event.reply, &activity.source)
    else {
        return;
    };
    if verified
        && event.lineage.is_none()
        && event.verifies_activity(activity)
        && event.channel_id == channel.id
        && event.record_id == channel.pointer.record_id
        && identity == &reply.identity
        && crate::native_lark::credentials(&channel.definition, &event.reply).is_some()
        && state
            .native_lark_effects
            .as_ref()
            .is_some_and(|c| c.confirmed_issuance_enabled)
    {
        capabilities.edit = true;
        capabilities.message_effects = Some(super::MessageEffectsCapabilities::new(true, None));
    }
    if let Some(native) = &state.native_lark_effects {
        crate::interactions::native_lark::project(
            capabilities,
            channel,
            event,
            activity,
            verified,
            native.controls_issuance_enabled,
            native.forms_issuance_enabled,
        );
    }
}
