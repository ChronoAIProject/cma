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
mod control_plane;
mod creation_outcomes;
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
mod runtime_migration;
mod runtime_quarantine;
mod state;
mod stores;
mod suggestion_control;
mod usage;
use crate::{
    auth::AuthState,
    config::Config,
    profile_policy::profile_route_policy,
    projections::{AdapterAgentHistoryExporter, AdapterSandboxResources, AgentProjectionServices},
    sandbox::{
        runtime_capabilities, LiveAgentRegistry, PlatformTitleEndpoint,
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
pub(crate) use state::{ControlPlaneComposition, StartupComposition};
use std::sync::Arc;
