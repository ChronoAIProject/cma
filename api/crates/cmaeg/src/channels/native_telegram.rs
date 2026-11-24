//! Owner opt-in retains the existing default binding and requires durable recovery.
use super::management::{ManagementError, ManagementState};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};

pub(super) fn check_write(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
    flags: super::write_gate::WriteFlags,
) -> Result<(), ValidationError> {
    if !flags.telegram_member_joins {
        check_member_join_write(current, next)?;
    }
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::Telegram {
                topic_creation: true,
                ..
            }
        ) && !flags.telegram_topic_creation
            && !current.is_some_and(|old| retains_creation_opt_in(old, source))
    }) {
        return Err(ValidationError::new(
            format!("/sources/{index}/topic_creation"),
            "native_telegram_topic_creation_disabled",
        ));
    }
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::Telegram {
                topic_lifecycle: true,
                ..
            }
        ) && !flags.telegram_topic_lifecycle
            && !current.is_some_and(|old| retains_topic_opt_in(old, source))
    }) {
        return Err(ValidationError::new(
            format!("/sources/{index}/topic_lifecycle"),
            "native_telegram_topic_lifecycle_disabled",
        ));
    }
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::Telegram {
                message_events: true,
                ..
            }
        ) && !flags.telegram_message_events
            && !current.is_some_and(|old| retains_opt_in(old, source))
    }) {
        return Err(ValidationError::new(
            format!("/sources/{index}/message_events"),
            "native_telegram_message_events_writes_disabled",
        ));
    }
    Ok(())
}

fn check_member_join_write(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
) -> Result<(), ValidationError> {
    for (index, source) in next.sources.iter().enumerate() {
        let Source::Telegram {
            bot_id,
            admission,
            member_join_chat_ids,
            ..
        } = source
        else {
            continue;
        };
        if member_join_chat_ids.is_empty() {
            continue;
        }
        let retained = current.is_some_and(|old| {
            old.sources.iter().any(|prior| {
                matches!(prior, Source::Telegram {
                    bot_id: prior_bot,
                    admission: prior_admission,
                    member_join_chat_ids: prior_chats,
                    ..
                } if prior_bot == bot_id && prior_admission == admission
                    && member_join_chat_ids.iter().all(|chat| prior_chats.contains(chat)))
            }) && old.secrets.telegram_bot_token == next.secrets.telegram_bot_token
                && old.secrets.telegram_webhook_secret == next.secrets.telegram_webhook_secret
        });
        if !retained {
            return Err(ValidationError::new(
                format!("/sources/{index}/member_join_chat_ids"),
                "native_telegram_member_joins_disabled",
            ));
        }
    }
    Ok(())
}

/// Retaining a saved source or removing only channel-post access is an opt-out.
pub(super) fn retains_opt_in(current: &ChannelDefinition, source: &Source) -> bool {
    let source = crate::telegram::messaging_source(source);
    if current
        .sources
        .iter()
        .any(|old| crate::telegram::messaging_source(old) == source)
    {
        return true;
    }
    let mut before = source.clone();
    let Source::Telegram { channel_posts, .. } = &mut before else {
        return false;
    };
    if *channel_posts {
        return false;
    }
    *channel_posts = true;
    current
        .sources
        .iter()
        .any(|old| crate::telegram::messaging_source(old) == before)
}

/// Removing message-event access does not require reauthorizing retained topics.
pub(super) fn retains_topic_opt_in(current: &ChannelDefinition, source: &Source) -> bool {
    let Source::Telegram {
        message_events,
        channel_posts,
        topic_creation,
        member_join_chat_ids,
        ..
    } = source
    else {
        return false;
    };
    current.sources.iter().any(|old| {
        let mut retained = old.clone();
        let Source::Telegram {
            message_events: old_events,
            channel_posts: old_posts,
            topic_creation: old_creation,
            member_join_chat_ids: old_member_join_chat_ids,
            ..
        } = &mut retained
        else {
            return false;
        };
        if (*message_events && !*old_events) || (*channel_posts && !*old_posts) {
            return false;
        }
        *old_creation = *topic_creation;
        *old_events = *message_events;
        *old_posts = *channel_posts;
        old_member_join_chat_ids.clone_from(member_join_chat_ids);
        retained == *source
    })
}

/// Creation is independently consented; other Telegram grants do not widen it.
pub(super) fn retains_creation_opt_in(current: &ChannelDefinition, source: &Source) -> bool {
    let normalize = |source: &Source| {
        let mut source = source.clone();
        if let Source::Telegram {
            topic_lifecycle,
            message_events,
            channel_posts,
            member_join_chat_ids,
            ..
        } = &mut source
        {
            *topic_lifecycle = false;
            *message_events = false;
            *channel_posts = false;
            member_join_chat_ids.clear();
        }
        source
    };
    current
        .sources
        .iter()
        .any(|old| normalize(old) == normalize(source))
}

pub(super) fn initialize(
    state: &ManagementState,
    current: Option<&ChannelDefinition>,
    definition: &mut ChannelDefinition,
) -> Result<(), ManagementError> {
    let opted = |value: &ChannelDefinition| {
        super::relay_telegram::opted_in(value)
            || value.sources.iter().any(|source| {
                matches!(
                    source,
                    Source::Telegram {
                        message_events: true,
                        ..
                    } | Source::Telegram {
                        topic_lifecycle: true,
                        ..
                    } | Source::Telegram {
                        topic_creation: true,
                        ..
                    }
                )
            })
    };
    if !opted(definition) || current.is_some_and(|old| opted(old) && old.routing.is_some()) {
        return Ok(());
    }
    let topics = definition.sources.iter().any(|s| {
        matches!(
            s,
            Source::Telegram {
                topic_lifecycle: true,
                ..
            }
        )
    });
    let events = definition.sources.iter().any(|s| {
        matches!(
            s,
            Source::Telegram {
                message_events: true,
                ..
            }
        )
    });
    let creation = definition.sources.iter().any(|s| {
        matches!(
            s,
            Source::Telegram {
                topic_creation: true,
                ..
            }
        )
    });
    if super::relay_telegram::opted_in(definition) && !state.relay_telegram_member_join_authoring()
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/sources",
            "Telegram relay member joins require enabled authoring and durable admission",
        )));
    }
    if (creation && !state.native_telegram_topic_creation_enabled)
        || (events && !state.native_telegram_message_events_writes_enabled)
        || (topics && !state.native_telegram_topic_lifecycle_enabled)
        || !state.native_telegram_durable_admission_enabled
        || !state.multi_consumer_writes_enabled
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/sources",
            "Telegram message events or topic management require enabled authoring and durable admission",
        )));
    }
    if definition
        .consumers()
        .filter(|consumer| !consumer.pending)
        .any(|consumer| {
            !state
                .providers
                .supports_response_replay(&consumer.provider.slug)
                .unwrap_or(false)
        })
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/provider/slug",
            "Telegram retained operations require providers configured for durable response recovery",
        )));
    }
    if definition.routing.is_none() {
        definition.routing = Some(crate::telegram::messaging_compat::default_routing());
    }
    super::definition::validate_definition(current, definition, state.write_flags())
        .map_err(ManagementError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telegram_message_events_write_gate_retains_exact_opt_in_and_allows_opt_out() {
        let mut value = crate::channels::routing_fixture::routed();
        value["sources"][0]["message_events"] = true.into();
        let original = crate::channels::routing_fixture::definition(value);
        let flags = crate::channels::write_gate::WriteFlags::default();
        assert!(crate::channels::write_gate::check(None, &original, flags).is_err());
        let mut next = original.clone();
        next.name = "renamed".into();
        crate::channels::write_gate::check(Some(&original), &next, flags).unwrap();
        let Source::Telegram { message_events, .. } = &mut next.sources[0] else {
            panic!()
        };
        *message_events = false;
        crate::channels::write_gate::check(Some(&original), &next, flags).unwrap();
        assert!(serde_json::to_value(&next.sources[0])
            .unwrap()
            .get("message_events")
            .is_none());
    }
    #[test]
    fn telegram_channel_posts_are_explicit_require_message_events_and_share_authoring_gate() {
        let mut value = crate::channels::routing_fixture::routed();
        value["sources"][0]["channel_posts"] = true.into();
        let definition = crate::channels::routing_fixture::definition(value.clone());
        assert!(definition.validate().is_err());
        value["sources"][0]["message_events"] = true.into();
        let definition = crate::channels::routing_fixture::definition(value);
        definition.validate().unwrap();
        let flags = crate::channels::write_gate::WriteFlags::default();
        assert!(crate::channels::write_gate::check(None, &definition, flags).is_err());
        crate::channels::write_gate::check(Some(&definition), &definition, flags).unwrap();
        let mut prior = definition.clone();
        let Source::Telegram { channel_posts, .. } = &mut prior.sources[0] else {
            panic!()
        };
        *channel_posts = false;
        assert!(serde_json::to_value(&prior.sources[0])
            .unwrap()
            .get("channel_posts")
            .is_none());
        assert!(crate::channels::write_gate::check(Some(&prior), &definition, flags).is_err());
        crate::channels::write_gate::check(Some(&definition), &prior, flags).unwrap();
        let Source::Telegram { bot_id, .. } = &mut prior.sources[0] else {
            panic!()
        };
        *bot_id = "88".into();
        assert!(crate::channels::write_gate::check(Some(&definition), &prior, flags).is_err());
    }
}

#[cfg(test)]
#[path = "native_telegram_member_join_tests.rs"]
mod member_join_tests;
