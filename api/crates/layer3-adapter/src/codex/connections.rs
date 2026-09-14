//! Codex model connection routes, including immutable legacy profile values.

use crate::ModelConnectionServices;

pub const PUBLIC_CODEX_SERVICE: &str = "chrono-llm-public";
/// An internal connection requirement, never a NyxID AgentKey service grant.
pub const CODEX_SUBSCRIPTION_CONNECTION: &str = "cma:codex-subscription";

pub const CONNECTION_SERVICES: ModelConnectionServices = ModelConnectionServices {
    managed: PUBLIC_CODEX_SERVICE,
    legacy_managed: "chrono-llm",
    subscription: CODEX_SUBSCRIPTION_CONNECTION,
};
