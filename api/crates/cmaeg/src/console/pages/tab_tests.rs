//! Edge cases of the detail tabs that integration fixtures do not reach:
//! unreadable definitions, routed channels, connection-managed sources.
use std::sync::Arc;

use super::{
    consumers, overview,
    overview::OverviewView,
    routing,
    sources::{self, SourcesView},
};
use crate::console::{
    detail::ChannelDetail,
    directory::ChannelView,
    source_changes::SourceAvailability,
    summary::{ChannelSummary, SummaryStatus},
    test_support::{definition, record},
};

fn view(summary: ChannelSummary) -> ChannelView {
    ChannelView {
        record: record(1),
        version: 3,
        summary: Arc::new(summary),
        channel_id: None,
    }
}

fn readable() -> ChannelSummary {
    ChannelSummary::of(&definition(&record(1), "alice", "orders"), 60_000, true)
}

#[test]
fn an_unreadable_definition_shows_its_status_only() {
    let view = view(ChannelSummary::without_definition(
        "alice",
        SummaryStatus::Unreadable,
    ));
    let html = overview::render(&OverviewView {
        view: &view,
        public_url: "https://gateway.example",
        sources: SourceAvailability::Available,
        multi_consumer: false,
        unknown_id: "unknown",
    });
    assert!(html.contains("definition cannot be read"));
    assert!(html.contains("<dt>Channel id</dt><dd>unknown</dd>"));
    assert!(!html.contains("Endpoints"));
    let sources = sources::render(&SourcesView {
        view: &view,
        last_events: &[],
        availability: SourceAvailability::Available,
    });
    assert!(sources.contains("its sources are not shown"));
    assert!(consumers::render(&view).contains("No consumer is known"));
}

#[test]
fn a_nearly_full_record_warns_on_the_overview_and_the_consumers() {
    let view = view(readable());
    let html = overview::render(&OverviewView {
        view: &view,
        public_url: "https://gateway.example",
        sources: SourceAvailability::Disabled,
        multi_consumer: false,
        unknown_id: "unknown",
    });
    assert!(html.contains("Warning: record nearly full"));
    assert!(html.contains("Source changes are disabled by the operator."));
    // The meter never runs past its maximum.
    assert!(html.contains("max=\"49152\" value=\"49152\""));
    assert!(consumers::render(&view).contains("record nearly full"));
}

#[test]
fn routed_channels_explain_routing_while_it_is_off() {
    let mut summary = readable();
    summary.routed = true;
    summary.default_receives_all = Some(true);
    let routed = view(summary);
    let html = routing::render(&routed, false);
    assert!(html.contains("All events continue to the default consumer"));
    assert!(html.contains("cannot display"));
    assert!(!routing::render(&routed, true).contains("disabled by the operator"));
}

#[test]
fn connection_managed_and_sourceless_channels_say_so() {
    let mut definition = definition(&record(1), "alice", "orders");
    definition.connection_ref = Some("connection-reference-value".into());
    definition.sources.clear();
    let mut summary = readable();
    summary.detail = Some(ChannelDetail::of(&definition));
    let view = view(summary);
    let html = sources::render(&SourcesView {
        view: &view,
        last_events: &[],
        availability: SourceAvailability::NotYetAvailable,
    });
    assert!(html.contains("A connection manages this channel's sources."));
    assert!(html.contains("This channel has no sources."));
    assert!(!html.contains("connection-reference-value"));
}

fn entry() -> crate::console::activity::ActivityEntry {
    crate::console::activity::ActivityEntry {
        seq: 1,
        at: time::OffsetDateTime::UNIX_EPOCH,
        event_key: Some("00000000000000aa".into()),
        source: "custom".into(),
        kind: "event",
        actor_masked: String::new(),
        chat_masked: String::new(),
        attachments: [0; 4],
        consumers: Vec::new(),
        refusal: None,
        causation: Some(crate::console::observer::CausationSummary {
            parent_key: "00000000000000bb".into(),
            hop: 2,
            published_by: cmaeg_core::ids::ConsumerId::default_consumer(),
        }),
    }
}

#[test]
fn an_event_whose_parent_left_the_ring_says_so() {
    let view = view(readable());
    let entry = entry();
    let html = super::event::render(&super::event::EventView {
        view: &view,
        entry: &entry,
        parent_held: false,
    });
    assert!(html.contains("<code>00000000000000bb</code> (no longer in memory)"));
    assert!(!html.contains("events/00000000000000bb"));
    assert!(html.contains("No consumer has reported progress"));
    assert!(html.contains("<dt>Actor</dt><dd>—</dd>"));
}

#[test]
fn refused_and_routed_events_explain_their_routing() {
    let mut refused = entry();
    refused.refusal = Some("sender_denied".into());
    let legacy = view(readable());
    let html = super::event::render(&super::event::EventView {
        view: &legacy,
        entry: &refused,
        parent_held: true,
    });
    assert!(html.contains("Refused before routing: <code>sender_denied</code>"));
    let mut summary = readable();
    summary.routed = true;
    let routed = view(summary);
    let html = super::event::render(&super::event::EventView {
        view: &routed,
        entry: &entry(),
        parent_held: true,
    });
    assert!(html.contains("keeps no routing trace"));
}
