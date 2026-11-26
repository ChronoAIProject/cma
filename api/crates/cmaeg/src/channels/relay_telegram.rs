//! Owner-authored group joins for verified Telegram relay sources.
use crate::ingress::telegram::member_joins;
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};

pub(super) fn check_write(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
    enabled: bool,
) -> Result<(), ValidationError> {
    for (index, next_source) in next.sources.iter().enumerate() {
        let Source::NyxidRelay {
            member_join_chat_ids: groups,
            ..
        } = next_source
        else {
            continue;
        };
        if groups.is_empty() {
            continue;
        }
        let pointer = format!("/sources/{index}/member_join_chat_ids");
        let retained = current.is_some_and(|old| {
            old.sources.iter().any(|previous| {
                member_joins::relay_messaging_source(previous)
                    == member_joins::relay_messaging_source(next_source)
                    && groups
                        .iter()
                        .all(|chat| member_joins::group_opted_in(previous, chat))
            })
        });
        if !enabled && !retained {
            return Err(ValidationError::new(
                pointer,
                "relay_telegram_member_joins_disabled",
            ));
        }
    }
    Ok(())
}

pub(super) fn opted_in(definition: &ChannelDefinition) -> bool {
    definition.sources.iter().any(|source| {
        matches!(
            source, Source::NyxidRelay { member_join_chat_ids, .. }
                if !member_join_chat_ids.is_empty()
        )
    })
}
