//! Durable external environment bindings and conversation ownership.
mod profile;
mod profile_input;
pub use profile::{map_profile, BindingEnvironmentPlan, BindingFieldRefusal, BindingProfilePlan};

#[cfg(test)]
mod profile_tests;
