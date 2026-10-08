//! Output identity comes from the command's immutable channel snapshot.
use crate::tools::ConsumerSummary;
use cmaeg_core::{channel::ChannelDefinition, ids::ConsumerName, seal::EventRefPayload};

pub(super) fn summary(
    definition: &ChannelDefinition,
    event: &EventRefPayload,
    routed: bool,
) -> Option<ConsumerSummary> {
    if !routed {
        return None;
    }
    let id = super::group::consumer_of(event);
    let (name, label) = definition
        .consumer(Some(&id))
        .map(|view| (view.name, view.label))
        // A synthetic removed-consumer failure still belongs to this id.
        // Omitting it would let a multiplexed waiter attribute it to default.
        .unwrap_or((id.as_str(), id.as_str()));
    Some(ConsumerSummary {
        name: ConsumerName::parse(name).expect("validated consumer name or consumer id"),
        label: label.to_owned(),
        id,
    })
}
