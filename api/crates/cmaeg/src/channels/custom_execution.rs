//! Immutable owner opt-in for self-service Custom execution and response recovery.
use super::management::{ManagementError, ManagementState};
use crate::providers::ProviderClient;
use cmaeg_core::{
    channel::{ChannelDefinition, RoutingPolicy, Source},
    ValidationError,
};

pub(super) fn default_routing() -> RoutingPolicy {
    serde_json::from_value(serde_json::json!({
        "version": 1, "default_consumer": {"name": "default", "non_human": true},
        "consumers": [], "default_targets": ["default"]
    }))
    .expect("default Custom routing is valid")
}

/// The marker is created together with routing. Neither a full PUT nor a
/// policy-only edit can turn a legacy channel into a newly selected channel.
pub(super) fn write_gate(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
    enabled: bool,
) -> Result<(), ValidationError> {
    if let Some(current) = current {
        if current.durable_custom_execution != next.durable_custom_execution {
            return Err(ValidationError::new(
                "/durable_custom_execution",
                "immutable; create a new channel to change execution mode",
            ));
        }
    } else if next.durable_custom_execution {
        if !enabled {
            return Err(ValidationError::new(
                "/durable_custom_execution",
                "custom_durable_execution_disabled",
            ));
        }
        if !next
            .sources
            .iter()
            .any(|s| matches!(s, Source::Custom { .. }))
        {
            return Err(ValidationError::new(
                "/sources",
                "durable Custom execution requires a Custom source",
            ));
        }
    }
    Ok(())
}

/// Every consumer may become a routing target after a policy edit; refusing an
/// incompatible provider before binding prevents an unusable retained channel.
pub(super) fn validate_providers(
    providers: &ProviderClient,
    definition: &ChannelDefinition,
) -> Result<(), ManagementError> {
    if definition.durable_custom_execution
        && definition.consumers().any(|consumer| {
            !providers
                .supports_response_replay(&consumer.provider.slug)
                .unwrap_or(false)
        })
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/provider/slug",
            "durable Custom execution requires providers configured for response recovery",
        )));
    }
    Ok(())
}

/// Existing marker edits may retain their exact media contract while writers are
/// paused. New attachment contracts must have the concrete continuation path.
pub(super) fn validate_media(
    state: &ManagementState,
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
) -> Result<(), ManagementError> {
    let custom_sources = |d: &ChannelDefinition| {
        d.sources
            .iter()
            .filter(|s| matches!(s, Source::Custom { .. }))
            .cloned()
            .collect::<Vec<_>>()
    };
    let targets = |d: &ChannelDefinition| {
        d.consumers()
            .filter(|c| !c.provider.target.is_profile())
            .map(|c| c.provider.clone())
            .collect::<Vec<_>>()
    };
    if !next.durable_custom_execution
        || !next.sources.iter().any(|s| {
            matches!(
                s,
                Source::Custom {
                    attachments: Some(_),
                    ..
                }
            )
        })
        || current.is_some_and(|old| {
            custom_sources(old) == custom_sources(next) && targets(old) == targets(next)
        })
    {
        return Ok(());
    }
    if targets(next).iter().any(|target| {
        !state
            .custom_existing_agent_media_providers
            .contains(target.slug.as_str())
    }) {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/sources/custom/attachments",
            "custom_existing_agent_media_unavailable",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{routing_fixture, write_gate::WriteFlags};

    #[test]
    fn marker_is_omitted_for_legacy_and_cannot_be_added_or_removed_by_updates() {
        let mut value = routing_fixture::routed();
        let legacy = routing_fixture::definition(value.clone());
        assert!(!legacy.durable_custom_execution);
        assert!(legacy
            .to_blob_json()
            .get("durable_custom_execution")
            .is_none());
        assert!(legacy
            .to_public_json()
            .get("durable_custom_execution")
            .is_none());
        value["durable_custom_execution"] = true.into();
        let selected = routing_fixture::definition(value);
        selected.validate().unwrap();
        assert!(write_gate(None, &selected, false).is_err());
        write_gate(None, &selected, true).unwrap();
        write_gate(Some(&selected), &selected, false).unwrap();
        assert!(write_gate(Some(&legacy), &selected, true).is_err());
        assert!(write_gate(Some(&selected), &legacy, true).is_err());
        let mut unrouted = selected;
        unrouted.routing = None;
        assert_eq!(unrouted.validate().unwrap_err().pointer, "/routing");
        // Unchanged marker flows through the ordinary gate with writers disabled.
        crate::channels::write_gate::check(Some(&legacy), &legacy, WriteFlags::default()).unwrap();
    }

    #[test]
    fn retained_and_pending_consumers_must_keep_compatible_provider_recovery() {
        let registry = crate::providers::ProviderRegistry::parse(&serde_json::json!([{
            "slug":"cma_codex","base_url":"http://127.0.0.1:9099","kind":"responses_http",
            "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/bindings/{binding_id}",
            "conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/responses",
            "recovery":{"response":"/responses/{response_id}","events":"/responses/{response_id}/events"}},
            "max_inflight":1
        }]).to_string()).unwrap();
        let providers = ProviderClient::new(
            registry,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            1,
        )
        .unwrap();
        let mut value = routing_fixture::routed();
        value["durable_custom_execution"] = true.into();
        for pending in [false, true] {
            value["routing"]["consumers"][1]["pending"] = pending.into();
            let definition = routing_fixture::definition(value.clone());
            assert!(validate_providers(&providers, &definition).is_err());
        }
        value["routing"]["consumers"].as_array_mut().unwrap().pop();
        let definition = routing_fixture::definition(value);
        validate_providers(&providers, &definition).unwrap();
        assert!(definition
            .consumers()
            .all(|c| c.provider.slug.as_str() == "cma_codex"));
    }
}
