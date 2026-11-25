//! Durable external environment bindings and conversation ownership.
mod access;
mod commands;
mod conversation_retirement;
mod credential;
mod existing;
mod inspection;
pub use access::{
    BindingAccessService, BindingConversationAuthority, BindingPrincipal, BindingSubmissionPolicy,
};
pub use inspection::{
    BindingConversationCatalog, BindingConversationInspection, BindingProfileDrift,
    DesiredProfileDrift,
};
mod fence;
mod model;
mod preparation_service;
mod profile;
mod profile_input;
mod profile_transfer;
pub use profile_transfer::{ProfileTransferReceipt, ProfileTransferRequest};
mod profiles;
pub use preparation_service::{BindingPreparationEvent, BindingPreparationInspection};
mod readiness;
mod reprovision;
pub use reprovision::{
    BindingGenerationGuard, BindingPreparationIntent, BindingPreparationReceipt,
};
mod recovery;
mod reset;
pub use reset::{
    reset_binding_eligibility, reset_eligibility, ConversationHead, ConversationResetFormat,
    ConversationResetReceipt, ConversationResetRequest, ConversationResetState,
};
mod retry;
mod rotation;
mod service;
pub mod store;
pub use credential::{BindingCredentialCustody, ProvidedCredential};
pub use fence::{BindingAdmissionFence, BindingConversationFence};
pub use model::{
    BindingConversation, BindingConversationState, BindingState, BindingWork, ExistingBindingAgent,
    PreviousBindingEnvironment, ProviderBinding, BINDING_CONFORMANCE_LEVEL,
    EXISTING_AGENT_CONVERSATION,
};
pub use profile::{
    map_profile, BindingEnvironmentPlan, BindingFieldRefusal, BindingProfilePlan, SessionAffinity,
};
pub use store::BindingStore;

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod reset_format_tests;

pub use service::{
    BindingError, BindingPolicy, BindingResources, BindingService, BindingServiceStore,
};

#[cfg(test)]
mod service_tests;

pub(super) fn valid_binding_id(id: &str) -> bool {
    model::valid_id(id, 1)
}

// Transport parsing belongs to the interface and adapter. Retained authority
// still requires one bounded, nonempty destination without control characters.
pub(super) fn valid_tool_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}
