//! Only an owner-authored native Telegram opt-in selects automatic durable routing.
use super::*;

pub(crate) fn opted_in(request: &LaunchRequest) -> bool {
    use crate::ingress::telegram::member_joins;
    if member_joins::is_member_join(&request.activity) {
        return request
            .channel
            .definition
            .source_of(&request.activity.source)
            .is_some_and(|source| {
                member_joins::group_opted_in(source, &request.activity.conversation.id)
            });
    }
    matches!(
        request.activity.source,
        cmaeg_core::activity::SourceKind::Telegram { .. }
    ) && matches!(
        request
            .channel
            .definition
            .source_of(&request.activity.source),
        Some(
            cmaeg_core::channel::Source::Telegram {
                message_events: true,
                ..
            } | cmaeg_core::channel::Source::Telegram {
                topic_lifecycle: true,
                ..
            } | cmaeg_core::channel::Source::Telegram {
                topic_creation: true,
                ..
            }
        )
    )
}
impl RoutedIngress {
    pub(crate) fn telegram_selected(&self, request: &LaunchRequest) -> bool {
        opted_in(request) && self.native_eligible(&request.channel.definition)
    }
    pub(crate) fn selected_request(&self, request: &LaunchRequest) -> bool {
        self.selected(&request.channel.definition.record_id)
            || self.telegram_selected(request)
            || (super::custom::opted_in(request)
                && self.native_eligible(&request.channel.definition))
    }
}
