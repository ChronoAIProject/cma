//! The multi-consumer routing policy: its stored shapes, the reader's rules
//! and the pure helpers built on them.
#[path = "routing_policy/default_reply.rs"]
mod default_reply;
#[path = "routing_policy/fixture.rs"]
mod fixture;
#[path = "routing_policy/receives_all.rs"]
mod receives_all;
#[path = "routing_policy/rules.rs"]
mod rules;
#[path = "routing_policy/shapes.rs"]
mod shapes;
#[path = "routing_policy/validation.rs"]
mod validation;
