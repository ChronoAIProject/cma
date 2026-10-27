//! Native destination discovery requires sealed conversation and current source custody.
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, ConversationKind, SourceKind},
    seal::{EventRefPayload, ReplyAddress},
};
pub(super) fn native_whatsapp_templates(
    enabled: bool,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
    verified: bool,
) -> bool {
    if !enabled
        || !verified
        || activity.conversation.thread_id.is_some()
        || crate::whatsapp::credentials(&channel.definition, &event.reply)
            .is_none_or(|credentials| credentials.validate().is_err())
    {
        return false;
    }
    match (&event.reply, &activity.source) {
        (
            ReplyAddress::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                ..
            },
            SourceKind::Whatsapp {
                app_id: app,
                waba_id: account,
                phone_number_id: phone,
            },
        ) => {
            app_id == app
                && waba_id == account
                && phone_number_id == phone
                && recipient_id == &activity.conversation.id
                && activity.conversation.kind == ConversationKind::Private
        }
        (
            ReplyAddress::WhatsappGroup(reply),
            SourceKind::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
            },
        ) => {
            reply.validate().is_ok()
                && reply.app_id == *app_id
                && reply.waba_id == *waba_id
                && reply.phone_number_id == *phone_number_id
                && reply.group_id == activity.conversation.id
                && reply.participant_id == activity.actor.id
                && activity.conversation.kind == ConversationKind::Group
        }
        _ => false,
    }
}

pub(super) fn group_capabilities(capabilities: &mut super::Capabilities, event: &EventRefPayload) {
    if matches!(event.reply, ReplyAddress::WhatsappGroup(_)) {
        capabilities.buttons = "none".into();
        if capabilities.whatsapp_templates == Some(true) {
            capabilities.whatsapp_template_components = Some(vec!["header".into(), "body".into()]);
        }
    }
}
