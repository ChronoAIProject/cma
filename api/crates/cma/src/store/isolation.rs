//! Agent Manager's exact schema authority. Legacy names retain existing data IDs.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) const COLLECTIONS: &[&str] = &[
    super::budget::COLLECTION,
    super::sandbox_metric_bindings::COLLECTION,
    super::sandbox_metric_rollups::COLLECTION,
    super::sandbox_usage_rollups::COLLECTION,
    super::usage_reports::JOBS,
    super::usage_reports::ROWS,
    super::usage_reports::CHARTS,
    super::usage_reports::CONTROL,
    super::request_limits::COLLECTION,
    "visitor_launch_days",
    crate::document_artifacts::COLLECTION,
    super::native_warm_pools::COLLECTION,
    super::native_prepared::COLLECTION,
    "runtime_backend_routes",
    "activity",
    "agent_artifacts",
    "agent_channel_usage",
    "agent_suggestions",
    "agent_command_orders",
    "agent_commands",
    "agent_creation_provenance",
    "agent_access_tools",
    "agent_access_requests",
    "agent_log",
    super::message_feedback::COLLECTION,
    "agent_lifecycle_operations",
    "agent_profile_revisions",
    "agent_profiles",
    "agent_reuse_bindings",
    "agents",
    "auth_api_bearers",
    "auth_actor_links",
    super::auth::PROXY_GRANTS,
    "auth_browser_logins",
    "auth_login_attempts",
    "auth_subjects",
    "binding_conversations",
    super::media_uploads::COLLECTION,
    "bot_setups",
    "bot_setup_contexts",
    "bots",
    "bridge_allocation_heads",
    "bridge_allocations",
    "bridge_callback_dispositions",
    "bridge_callback_inbox",
    "bridge_callback_platform_messages",
    "bridge_callback_rates",
    "bridge_callback_receipts",
    "bridge_callback_replies",
    "bridge_channel_capabilities",
    "bridge_channel_control_aliases",
    "bridge_channel_control_intents",
    "bridge_channel_control_references",
    "bridge_credentials",
    "bridge_definition_receipts",
    "bridge_definitions",
    "bridge_invoke_audits",
    "bridge_invoke_grants",
    "bridge_invoke_receipts",
    "bridge_outbox_audits",
    "bridge_outbox_heads",
    "bridge_outbox_invocations",
    "bridge_outbox_items",
    "bridge_outbox_recoveries",
    "bridge_outbox_scans",
    "bridge_outbox_send_rates",
    "bridge_partition_command_receipts",
    "bridge_refused_control_aliases",
    "bridge_refused_control_intents",
    "bridge_source_control_audits",
    "bridge_source_control_policies",
    "bridge_source_control_receipts",
    "bridge_text_attachments",
    "channel_child_cleanup_job_admissions_v1",
    "channel_child_cleanup_jobs_v1",
    "channel_command_revival_audits",
    "channel_credential_renewal_audits",
    "channel_history_references",
    "channel_lifecycle_credential_audits",
    "channel_mint_orphan_audits",
    "channel_mint_orphans",
    "channel_provider_dispatch_journals",
    "channel_provider_finality_receipts",
    "channel_provider_teardown_receipts",
    "channel_retirement_audits",
    "channel_retirement_finalizations",
    "channel_reuse_cleanup_admission_audits",
    "channel_reuse_cleanup_admissions",
    "channel_reuse_cleanup_audits",
    "channel_reuse_cleanup_removal_audits",
    "channel_reuse_cleanup_requests",
    "channel_reuse_cleanup_tombstones",
    "channel_usage_events",
    "checkpoint_archive_gates",
    "checkpoint_archive_tombstones",
    "checkpoint_objects",
    "checkpoints",
    "codex_subscription_connections",
    "event_worker_profiles",
    "event_worker_profile_revisions",
    "command_authorization_audits",
    "operation_claims",
    "owned_workspace_cleanup_audits_v1",
    "owned_workspace_cleanup_consents_v1",
    "owned_workspace_cleanup_jobs_v1",
    "owned_workspace_cleanup_tombstones_v1",
    "platform_settings",
    "provider_bindings",
    "provider_binding_preparations",
    "provider_tool_opens",
    "provider_tool_relays",
    "resource_policies",
    "resource_share_commands",
    "resource_shares",
    "sandbox_admissions",
    super::sandbox_mongo::SANDBOX_BILLING,
    super::sandbox_colors::COLLECTION,
    "sandbox_controls",
    "sandbox_credential_revocations",
    "sandbox_credentials",
    "sandbox_events",
    super::sandbox_usage::STATES,
    super::sandbox_usage::EVENTS,
    "sandbox_profile_revisions",
    "sandbox_profiles",
    "sandbox_retained_availability",
    "sandbox_sessions",
    "sandboxes",
    "service_connection_handoffs",
    "trigger_definition_revisions",
    "trigger_definitions",
    "trigger_first_message_bindings",
    "trigger_launch_events",
    "trigger_launch_input_protocols",
    "trigger_launches",
    "users",
    "warm_sandboxes",
    "workspace_profile_revisions",
    "workspace_profiles",
    "workspaces",
    "worktrees",
];
// Existing AM stores initialize their own indexes and schemas at startup.
// These privileges confer no access to EG/IM (or legacy gateway) collections.
const ACTIONS: &[&str] = &[
    "find",
    "insert",
    "update",
    "remove",
    "createCollection",
    "createIndex",
    "listIndexes",
    "collMod",
];
pub(crate) fn role(database: &str) -> Document {
    let mut grants: Vec<_> = COLLECTIONS
        .iter()
        .map(|name| {
            let mut actions = ACTIONS.to_vec();
            if *name == "binding_conversations" {
                actions.push("dropIndex");
            }
            doc! {"resource":{"db":database,"collection":*name},"actions":actions}
        })
        .collect();
    grants.push(doc! {"resource":{"db":database,"collection":""},"actions":["listCollections"]});
    doc! {"createRole":"cma_am_runtime","privileges":grants,"roles":[]}
}
pub(crate) fn validate(status: &Document, database: &str) -> Result<(), StoreError> {
    let denied = || {
        StoreError::Unavailable(
            "Agent Manager database credential is not isolated to its schema".into(),
        )
    };
    let info = status.get_document("authInfo").map_err(|_| denied())?;
    if info
        .get_array("authenticatedUsers")
        .map_err(|_| denied())?
        .len()
        != 1
    {
        return Err(denied());
    }
    let mut grants = BTreeMap::<String, BTreeSet<String>>::new();
    for grant in info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| denied())?
    {
        let grant = grant.as_document().ok_or_else(&denied)?;
        let resource = grant.get_document("resource").map_err(|_| denied())?;
        let collection = resource.get_str("collection").map_err(|_| denied())?;
        if resource.len() != 2
            || resource.get_str("db").ok() != Some(database)
            || (!collection.is_empty()
                && !COLLECTIONS.contains(&collection)
                && collection != super::provenance_progress::COLLECTION)
        {
            return Err(denied());
        }
        for action in grant.get_array("actions").map_err(|_| denied())? {
            let action = action.as_str().ok_or_else(&denied)?;
            if !(if collection.is_empty() {
                action == "listCollections"
            } else if collection == super::provenance_progress::COLLECTION {
                super::provenance_progress::ACTIONS.contains(&action)
            } else {
                ACTIONS.contains(&action)
                    || (collection == "binding_conversations" && action == "dropIndex")
            }) {
                return Err(denied());
            }
            grants
                .entry(collection.into())
                .or_default()
                .insert(action.into());
        }
    }
    if grants
        .get(super::provenance_progress::COLLECTION)
        .is_some_and(|actions| {
            !super::provenance_progress::ACTIONS
                .iter()
                .all(|action| actions.contains(*action))
        })
    {
        return Err(denied());
    }
    if !COLLECTIONS.iter().all(|name| {
        ACTIONS
            .iter()
            .all(|action| grants.get(*name).is_some_and(|s| s.contains(*action)))
    }) || !grants
        .get("binding_conversations")
        .is_some_and(|actions| actions.contains("dropIndex"))
    {
        return Err(denied());
    }
    Ok(())
}
#[cfg(test)]
pub(crate) async fn verify(database: &Database) -> Result<(), StoreError> {
    verify_with_authority(
        database,
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
    )
    .await
}

pub(crate) async fn verify_with_authority(
    database: &Database,
    authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
) -> Result<(), StoreError> {
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| {
            StoreError::Unavailable("cannot verify Agent Manager schema authority".into())
        })?;
    match authority {
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated => {
            validate(&status, database.name())
        }
        cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema => {
            cma_service_auth::mongodb_authority::verify_shared_schema(
                &status,
                database.name(),
                &role(database.name()),
            )
            .map_err(|error| StoreError::Unavailable(error.into()))
        }
    }
}
#[cfg(test)]
#[path = "agent_lifecycle_contract.rs"]
pub(super) mod agent_lifecycle_contract;
#[cfg(test)]
#[path = "lifecycle_children_contract.rs"]
mod lifecycle_children_contract;
#[cfg(test)]
#[path = "profile_internal_contract.rs"]
mod profile_internal_contract;
#[cfg(test)]
#[path = "shared_schema_contract.rs"]
mod shared_schema_contract;
#[cfg(test)]
mod tests;
