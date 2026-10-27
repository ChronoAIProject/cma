//! Owner opt-in retains the existing default binding and requires durable recovery.
use super::management::{ManagementError, ManagementState};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};

pub(super) fn initialize(
    state: &ManagementState,
    current: Option<&ChannelDefinition>,
    definition: &mut ChannelDefinition,
) -> Result<(), ManagementError> {
    let opted = |value: &ChannelDefinition| {
        value.sources.iter().any(|source| {
            matches!(
                source,
                Source::Telegram {
                    message_events: true,
                    ..
                }
            )
        })
    };
    if !opted(definition) || current.is_some_and(|old| opted(old) && old.routing.is_some()) {
        return Ok(());
    }
    if !state.native_telegram_message_events_writes_enabled
        || !state.native_telegram_durable_admission_enabled
        || !state.multi_consumer_writes_enabled
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/sources",
            "Telegram message events require enabled authoring and durable admission",
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
            "Telegram message events require providers configured for durable response recovery",
        )));
    }
    if definition.routing.is_none() {
        definition.routing = Some(serde_json::from_value(serde_json::json!({
            "version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]
        })).map_err(|_| ManagementError::Internal("cannot initialize native routing".into()))?);
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
}
