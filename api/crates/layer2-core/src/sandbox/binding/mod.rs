//! Durable external environment bindings and conversation ownership.
mod access;
mod commands;
mod credential;
pub use access::{BindingAccessService, BindingConversationAuthority, BindingPrincipal};
mod fence;
mod model;
mod profile;
mod profile_input;
mod profiles;
mod recovery;
mod rotation;
mod service;
pub mod store;
pub use credential::{BindingCredentialCustody, ProvidedCredential};
pub use fence::{BindingAdmissionFence, BindingConversationFence};
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

pub use profiles::BindingProfileStore;
pub use service::{
    BindingError, BindingPolicy, BindingResources, BindingService, BindingServiceStore,
};

#[cfg(test)]
mod service_tests;

pub(super) fn valid_binding_id(id: &str) -> bool {
    model::valid_id(id, 1)
}
