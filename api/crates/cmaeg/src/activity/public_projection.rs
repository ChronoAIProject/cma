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
        consumer.produced_by = consumer.answer_production_bound();
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
