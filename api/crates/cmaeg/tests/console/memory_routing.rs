//! How routing's reports become activity entries: the decision bound into
//! the activity, the resolver's bounded trace, routing refusals with the
//! kill switch's shadow decision, and one entry per admission however the
//! ingress and the launcher both report it.
use cmaeg::{
    console::{
        activity::{ActivityEntry, ActivityFilter, ActivityRings, TraceStepView, Update},
        mask,
        observer::{ConsoleObserver, Observation},
    },
    turns::observer::{ActivityObserver, LaunchClass},
};
use cmaeg_core::{
    activity::{Activity, RouteResolution},
    channel::ChannelDefinition,
    ids::{ConsumerId, RecordId},
    routing::{explain, resolve, RouteError, Trace},
};
use serde_json::json;

use super::{
    policy::{NYX, REVIEWER},
    resolver_fixture::{causation, channel, decision, event, handoff, input, lark, message, rule},
};

fn record() -> RecordId {
    RecordId::new("00000000-0000-4000-8000-000000000001").unwrap()
}

/// Rings holding what `report` told a console observer.
async fn observed(report: impl FnOnce(&ConsoleObserver)) -> ActivityRings {
    let rings = ActivityRings::new(100, 1 << 20, 32);
    let (observer, queue) = ConsoleObserver::new();
    report(&observer);
    drop(observer);
    queue
        .drain(|observation| match observation {
            Observation::Routed { record, entry_seed } => rings.admitted(record, *entry_seed),
            Observation::Launch {
                record,
                event_key,
                consumer,
                outcome,
                accepted_ms,
            } => rings.update(
                &record,
                &event_key,
                Update::Launch {
                    consumer,
                    outcome,
                    accepted_ms,
                },
            ),
            other => panic!("unexpected {other:?}"),
        })
        .await;
    rings
}

fn entries(rings: &ActivityRings) -> Vec<ActivityEntry> {
    rings.entries(&record(), &ActivityFilter::default(), usize::MAX)
}

/// The routed fixture with a rule giving `com.example.*` events to
/// reviewer and nyx; nyx's provider ignores non-human actors.
fn ci_channel() -> ChannelDefinition {
    channel(|value| {
        value["routing"]["rules"] = json!([rule(
            "ci",
            json!({"event_types": ["com.example.*"]}),
            json!([REVIEWER, NYX])
        )]);
    })
}

/// `activity` routed on `def` with routing on, its decision bound in.
fn routed(def: &ChannelDefinition, mut activity: Activity) -> (Activity, Trace) {
    let routed = decision(def, &input(def, &activity));
    activity.routing = Some(routed.routing);
    (activity, routed.trace)
}

#[tokio::test]
async fn a_routed_event_keeps_its_decision_trace_and_skipped_consumers() {
    let def = ci_channel();
    let (activity, trace) = routed(&def, event("com.example.build", json!({})));
    let rings = observed(|observer| {
        observer.routed(&record(), &activity, &trace);
        // The launcher reports the same admission without the trace.
        observer.admitted(&record(), &activity);
        observer.launched(
            &record(),
            &activity.event_id,
            &ConsumerId::parse(REVIEWER).unwrap(),
            LaunchClass::Accepted,
            std::time::Duration::from_millis(40),
        );
    })
    .await;
    let held = entries(&rings);
    assert_eq!(held.len(), 1, "one entry per admission");
    let entry = &held[0];
    assert_eq!(entry.source, "custom");
    let routing = entry.routing.as_ref().unwrap();
    assert_eq!(routing.resolution, RouteResolution::Rule);
    assert_eq!(routing.rule_id.as_deref(), Some("ci"));
    let targets: Vec<&str> = routing.targets.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(targets, [REVIEWER]);
    assert_eq!(routing.targets[0].name, "reviewer");
    let steps = &entry.trace.as_ref().unwrap().steps;
    assert!(steps.contains(&TraceStepView::Rule {
        rule_id: "ci".into(),
        matched: true,
        inactive: false,
        failed_on: None,
    }));
    assert!(steps.contains(&TraceStepView::Skipped {
        consumer: ConsumerId::parse(NYX).unwrap(),
        reason: "ignores_actor_kind",
    }));
    assert_eq!(entry.consumers.len(), 1);
    assert_eq!(rings.totals().bytes, entry.weight());
    let dump = serde_json::to_string(&held).unwrap();
    for raw in ["https://ci.example/build", "ce-1", "user-1"] {
        assert!(!dump.contains(raw), "an entry keeps {raw}");
    }
}

#[tokio::test]
async fn reports_of_one_admission_merge_in_either_order_and_redeliveries_do_not() {
    let def = ci_channel();
    let (activity, trace) = routed(&def, event("com.example.build", json!({})));
    let reviewer = ConsumerId::parse(REVIEWER).unwrap();
    let rings = observed(|observer| {
        observer.admitted(&record(), &activity);
        observer.routed(&record(), &activity, &trace);
        observer.launched(
            &record(),
            &activity.event_id,
            &reviewer,
            LaunchClass::Accepted,
            std::time::Duration::ZERO,
        );
        // A redelivery of the event after it launched is a new admission.
        observer.routed(&record(), &activity, &trace);
        observer.admitted(&record(), &activity);
    })
    .await;
    let held = entries(&rings);
    assert_eq!(held.len(), 2);
    assert!(held
        .iter()
        .all(|entry| entry.trace.is_some() && entry.routing.is_some()));
    assert!(held[0].consumers.is_empty() && held[1].consumers.len() == 1);
    let sum: usize = held.iter().map(ActivityEntry::weight).sum();
    assert_eq!(rings.totals().bytes, sum);

    // Legacy launches carry no trace, so every report is its own entry.
    let plain = message(lark(), "hello");
    let rings = observed(|observer| {
        observer.admitted(&record(), &plain);
        observer.admitted(&record(), &plain);
    })
    .await;
    assert_eq!(entries(&rings).len(), 2);
}

#[tokio::test]
async fn a_kill_switch_refusal_keeps_its_shadow_decision() {
    let def = ci_channel();
    let activity = event("com.example.build", json!({}));
    let route = input(&def, &activity);
    let error = resolve(&def, &route, false).unwrap_err();
    assert_eq!(error, RouteError::KillSwitchClosed);
    let trace = explain(&def, &route, false);
    let rings = observed(|observer| {
        observer.route_refused(&record(), &activity, &error.to_string(), &trace);
    })
    .await;
    let held = entries(&rings);
    let entry = &held[0];
    assert_eq!(entry.refusal.as_deref(), Some("kill_switch_closed"));
    assert!(entry.routing.is_none() && entry.failed());
    let steps = &entry.trace.as_ref().unwrap().steps;
    assert_eq!(
        steps.first(),
        Some(&TraceStepView::Precedence {
            stage: "kill_switch"
        })
    );
    assert_eq!(
        steps.last(),
        Some(&TraceStepView::Shadow {
            resolution: RouteResolution::Rule,
            outcome: "closed",
        })
    );
    let refused = ActivityFilter {
        outcome: Some(LaunchClass::Refused),
        ..ActivityFilter::default()
    };
    assert_eq!(rings.entries(&record(), &refused, 10).len(), 1);
    // A free-text reason is never kept.
    let rings = observed(|observer| {
        observer.route_refused(&record(), &activity, "Target <b>x</b> unknown", &trace);
    })
    .await;
    assert_eq!(entries(&rings)[0].refusal.as_deref(), Some("other"));
}

#[tokio::test]
async fn a_legacy_launch_under_the_kill_switch_keeps_its_shadow_decision() {
    let def = channel(|_| {});
    let activity = message(lark(), "hello");
    let route = input(&def, &activity);
    assert!(matches!(
        resolve(&def, &route, false),
        Ok(cmaeg_core::routing::Resolution::Legacy)
    ));
    let trace = explain(&def, &route, false);
    let rings = observed(|observer| {
        observer.routed(&record(), &activity, &trace);
        observer.admitted(&record(), &activity);
    })
    .await;
    let held = entries(&rings);
    assert_eq!(held.len(), 1);
    assert!(held[0].routing.is_none());
    assert_eq!(
        held[0].trace.as_ref().unwrap().steps.last(),
        Some(&TraceStepView::Shadow {
            resolution: RouteResolution::Default,
            outcome: "legacy",
        })
    );
}

#[tokio::test]
async fn a_hand_off_records_the_agent_slot_and_its_causation() {
    let def = channel(|_| {});
    let activity = handoff("please summarize");
    let tokens = vec!["reviewer".to_owned()];
    let mut route = input(&def, &activity);
    route.published = Some((&tokens, causation()));
    let routed = decision(&def, &route);
    let mut activity = activity;
    activity.routing = Some(routed.routing);
    let rings = observed(|observer| observer.routed(&record(), &activity, &routed.trace)).await;
    let entry = &entries(&rings)[0];
    assert_eq!(entry.source, "agent");
    assert_eq!(
        entry.routing.as_ref().unwrap().resolution,
        RouteResolution::AgentEvent
    );
    let causation = entry.causation.as_ref().unwrap();
    assert_eq!(causation.parent_key, mask::event_key("evt-0"));
    assert_eq!(causation.hop, 1);
    assert_eq!(causation.published_by.as_str(), REVIEWER);
}
