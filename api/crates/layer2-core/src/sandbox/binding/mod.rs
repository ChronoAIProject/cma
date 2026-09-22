//! Durable external environment bindings and conversation ownership.
mod credential;
mod model;
mod profile;
mod profile_input;
pub mod store;
pub use credential::{BindingCredentialCustody, ProvidedCredential};
pub use model::{
    BindingConversation, BindingConversationState, BindingState, BindingWork,
    PreviousBindingEnvironment, ProviderBinding, BINDING_CONFORMANCE_LEVEL,
};
pub use profile::{map_profile, BindingEnvironmentPlan, BindingFieldRefusal, BindingProfilePlan};
pub use store::BindingStore;

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod profile_tests;
