//! The pure routing resolver: rule matching, precedence, the routing
//! constraints and the per-event fail-closed kill switch.
#[path = "routing_resolver/constraints.rs"]
mod constraints;
#[path = "routing_resolver/event_field.rs"]
mod event_field;
#[path = "routing_resolver/fixture.rs"]
mod fixture;
#[path = "routing_resolver/kill_switch.rs"]
mod kill_switch;
#[path = "routing_policy/fixture.rs"]
mod policy;
#[path = "routing_resolver/precedence.rs"]
mod precedence;
#[path = "routing_resolver/rules.rs"]
mod rules;

#[path = "routing_resolver/classification.rs"]
mod classification;
