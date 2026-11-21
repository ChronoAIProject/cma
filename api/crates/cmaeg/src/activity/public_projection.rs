use super::{ActivityPage, ChannelActivity};
use cmaeg_core::activity::ActorKind;

pub(crate) fn public_activity(activity: &ChannelActivity) -> ChannelActivity {
    let mut public = activity.clone();
    public.actor.id = "redacted".into();
    public.actor.display_name = Some(
        match public.actor.kind {
            ActorKind::Human => "Participant",
            ActorKind::Bot => "Bot",
            ActorKind::System => "System",
            ActorKind::Unknown => "Actor not reported",
        }
        .into(),
    );
    for consumer in &mut public.consumers {
        if !consumer.execution_attributions.is_empty() {
            consumer.execution_reference = consumer
                .provider
                .as_deref()
                .zip(consumer.response_id.as_deref())
                .and_then(|(provider, response)| {
                    consumer
                        .execution_attributions
                        .iter()
                        .find(|receipt| receipt.matches(provider, response))
                })
                .and_then(super::ExecutionAttribution::public_reference);
            consumer.execution_attributions.clear();
        }
    }
    public
}

pub(crate) fn public_page(page: &ActivityPage) -> ActivityPage {
    ActivityPage {
        items: page.items.iter().map(public_activity).collect(),
        cursor: page.cursor.clone(),
        retention_days: page.retention_days,
        limit: page.limit,
        has_more: page.has_more,
    }
}
