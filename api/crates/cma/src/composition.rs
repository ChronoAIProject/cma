use std::sync::Arc;
mod access;
mod account_deletion;
pub(crate) mod activity;
mod adapter;
mod agent_manager;
pub(crate) mod backend_free;
mod binding_diagnostics;
mod bindings;
mod bots;
mod capacity_recovery;
mod credentials;
mod desktop;
mod documents;
pub(crate) mod event_routing;
mod legal_consent;
mod media;
mod observation;
mod profiles;
mod routes;
mod runtime;
mod runtime_inventory;
mod stores;
mod usage;
use crate::{
    auth::{service_portal, AuthState},
    config::Config,
    profile_policy::profile_route_policy,
    projections::{AdapterAgentHistoryExporter, AdapterSandboxResources, AgentProjectionServices},
    sandbox::{
        runtime_capabilities, LiveAgentRegistry, LiveSandboxPolicy, PlatformTitleEndpoint,
        SandboxLifecycleComposition, SandboxLifecycleDependencies,
    },
    store::{DefaultProfileSpec, MongoStores},
    trigger::{TriggerComposition, TriggerCompositionDependencies},
};
use layer1_public::rest::{self, HierarchyState, ProfilesState};
use layer2_core::{
    sandbox::{AgentContextService, HierarchyReadService, SandboxService, WorkspaceService},
    ActivityService, OperationStore, PlatformSettingsService, UserDirectory,
};
mod state;
pub(crate) use state::ControlPlaneComposition;
mod assembly;
