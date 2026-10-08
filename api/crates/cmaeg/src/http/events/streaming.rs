//! Pin the admitted target order before a multiplexed turn can emit frames.
use crate::{
    delivery::sync::{SyncRegistry, Waiter},
    http::problems::problem,
    tools::ConsumerSummary,
};
use axum::{http::StatusCode, response::Response};
use cmaeg_core::{
    activity::ConsumerTarget, channel::ChannelDefinition, ids::ConsumerName, problem::ProblemCode,
};
use std::sync::Arc;

pub(super) fn register(
    registry: &Arc<SyncRegistry>,
    definition: &ChannelDefinition,
    targets: Option<&[ConsumerTarget]>,
) -> Result<Option<(String, Waiter)>, Box<Response>> {
    let consumer = definition
        .consumer(None)
        .expect("every channel has a default");
    let default = ConsumerSummary {
        id: consumer.id.clone(),
        name: ConsumerName::parse(consumer.name).expect("validated default consumer name"),
        label: consumer.label.to_owned(),
    };
    let count = targets.map_or(1, |targets| targets.len());
    let Ok(count) = u8::try_from(count) else {
        return Err(invalid_targets());
    };
    if !(1..=8).contains(&count) {
        return Err(invalid_targets());
    }
    let registered = registry.register_fanout(count, default);
    if let Some((id, _)) = &registered {
        if targets.is_some_and(|targets| !registry.set_fanout_targets(id, targets)) {
            // Dropping the owned registration removes it before returning.
            return Err(invalid_targets());
        }
    }
    Ok(registered)
}
fn invalid_targets() -> Box<Response> {
    tracing::error!("cannot pin admitted consumers for multiplexed Sync");
    Box::new(problem(
        StatusCode::INTERNAL_SERVER_ERROR,
        ProblemCode::Internal,
        "consumer_admission_invalid",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_target_setup_drops_registration_before_returning() {
        let definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        let registry = Arc::new(SyncRegistry::default());
        let target = ConsumerTarget {
            id: cmaeg_core::ids::ConsumerId::default_consumer(),
            name: ConsumerName::parse("default").unwrap(),
        };
        // Duplicate targets fail after registration; invalid widths fail before it.
        for targets in [vec![], vec![target.clone(); 2], vec![target; 9]] {
            let error = match register(&registry, &definition, Some(&targets)) {
                Ok(_) => panic!("invalid admission must fail"),
                Err(error) => error,
            };
            assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert!(registry.is_empty());
        }
    }
}
