//! Durable external environment bindings and conversation ownership.
mod credential;
mod profile;
mod profile_input;
pub use credential::{BindingCredentialCustody, ProvidedCredential};
pub use profile::{map_profile, BindingEnvironmentPlan, BindingFieldRefusal, BindingProfilePlan};

#[cfg(test)]
mod profile_tests;
