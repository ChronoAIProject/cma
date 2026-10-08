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
    ChannelSummary::of(
        &definition(&record(1), "alice", "orders"),
        60_000,
        true,
        &std::collections::BTreeSet::new(),
    )
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
        forms: "<form>attach</form>",
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

/// The fixture channel with a policy that gives every event to `default`
/// and one rule naming only a source the channel does not hold.
fn routed() -> ChannelSummary {
    let mut definition = definition(&record(1), "alice", "orders");
    definition.routing = Some(
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "default_consumer": {"name": "lead"},
            "default_targets": ["default"],
            "rules": [{"id": "slack-only", "when": {"sources": ["relay_slack"]},
                "targets": ["default"]}]
        }))
        .unwrap(),
    );
    ChannelSummary::of(
        &definition,
        2_048,
        true,
        &std::collections::BTreeSet::from(["cma_codex".to_owned()]),
    )
}

#[test]
fn routed_channels_explain_routing_while_it_is_off() {
    let routed = view(routed());
    let html = routing::render(&routed, false);
    assert!(html.contains("All events continue to the default consumer"));
    assert!(html.contains("<dt>Model</dt><dd>routed</dd>"));
    assert!(html.contains("<code>slack-only</code> <span class=\"badge\">inactive</span>"));
    assert!(html.contains("<dt>Default targets</dt><dd>@lead <code>default</code></dd>"));
    assert!(!routing::render(&routed, true).contains("disabled by the operator"));
}

#[test]
fn a_routed_channel_whose_definition_is_unreadable_shows_no_policy() {
    let mut summary = ChannelSummary::without_definition("alice", SummaryStatus::Unreadable);
    summary.routed = true;
    let html = routing::render(&view(summary), true);
    assert!(html.contains("its routing is not shown"));
    assert!(!html.contains("<h2>Rules</h2>"));
}

#[test]
fn the_overview_names_the_custom_key_and_when_it_is_inert() {
    let overview = |definition: &cmaeg_core::channel::ChannelDefinition| {
        let mut summary = readable();
        summary.detail = Some(ChannelDetail::of(definition));
        overview::render(&OverviewView {
            view: &view(summary),
            public_url: "https://gateway.example",
            sources: SourceAvailability::Available,
            multi_consumer: false,
            unknown_id: "unknown",
        })
    };
    let mut definition = definition(&record(1), "alice", "orders");
    assert!(overview(&definition)
        .contains("<dt>Custom event key</dt><dd>same as the partition key</dd>"));
    definition.partition.key = cmaeg_core::channel::PartitionKey::Conversation;
    definition.partition.custom_key = Some(cmaeg_core::channel::PartitionKey::EventSubject);
    let html = overview(&definition);
    assert!(html.contains("<dt>Partition key</dt><dd>conversation</dd>"));
    assert!(html.contains("<dt>Custom event key</dt><dd>event_subject</dd>"));
    definition.sources.clear();
    assert!(overview(&definition).contains(
        "<dt>Custom event key</dt><dd>event_subject (inert: no custom source is attached)</dd>"
    ));
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
        availability: SourceAvailability::Available,
        forms: "<form>attach</form>",
    });
    assert!(html.contains("A connection manages this channel's sources."));
    assert!(html.contains("This channel has no sources."));
    // A connection-managed channel's sources change only through it.
    assert!(!html.contains("<form>attach</form>"));
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
        routing: None,
        trace: None,
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
    assert!(html.contains("No routing decision was recorded for this event"));
}
