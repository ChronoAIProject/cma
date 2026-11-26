//! Agent Manager's exact schema authority. Legacy names retain existing data IDs.
use super::retained_consents;
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::collections::{BTreeMap, BTreeSet};
mod observation;
#[cfg(test)]
use observation::validate_observation_grants;
pub(crate) use observation::verify_observation;
pub(crate) mod replicas;
pub(crate) const COLLECTIONS: &[&str] = &[
    "account_deletions",
    "account_deletion_events",
    retained_consents::COLLECTION,
    super::migrations::measured_custody::COLLECTION,
    super::migrations::measured_batches::COLLECTION,
    super::migrations::measured_index::PROJECTIONS,
    super::migrations::measured_index::MANIFESTS,
    super::migrations::measured_index::BATCHES,
    super::agent_log_segments::active_proof::COLLECTION,
    super::budget::COLLECTION,
    super::agent_observation::COLLECTION,
    super::sandbox_metric_bindings::COLLECTION,
    super::sandbox_metric_rollups::COLLECTION,
    super::sandbox_usage_rollups::COLLECTION,
    super::allocation_buckets::COLLECTION,
    super::usage_reports::JOBS,
    super::usage_reports::ROWS,
    super::usage_reports::CHARTS,
    super::migrations::usage_projections::COLLECTION,
    super::usage_reports::CONTROL,
    super::request_limits::COLLECTION,
    "visitor_launch_days",
    crate::document_artifacts::COLLECTION,
    crate::document_artifacts::QUOTA_COLLECTION,
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
    super::agent_log_segments::HEADS,
    super::agent_log_segments::SEGMENTS,
    super::agent_log_segments::PUBLICATIONS,
    super::agent_log_segments::PROJECTIONS,
    super::agent_log_segments::EVIDENCE,
    super::agent_log_segments::AUDITS,
    super::agent_log_segments::writer_gate::COLLECTION,
    super::agent_log_segments::writer_admission::COLLECTION,
    super::agent_capacity_recovery::records::COLLECTION,
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
    "resource_execution_effects_v1",
    "resource_execution_grants_v1",
    "resource_execution_records_v1",
    "resource_policies",
    "resource_revocation_operations_v1",
    "resource_share_commands",
    "resource_shares",
    "sandbox_admissions",
    super::creation_outcomes::COLLECTION,
    super::sandbox_mongo::SANDBOX_BILLING,
    super::sandbox_colors::COLLECTION,
    "sandbox_controls",
    super::sandbox_credentials::issuance::COLLECTION,
    "sandbox_credential_revocations",
    "sandbox_credentials",
    "sandbox_events",
    super::sandbox_usage::STATES,
    super::sandbox_usage::EVENTS,
    super::sandbox_usage_export::CAPTURES,
    super::sandbox_usage_export::EVIDENCE,
    super::sandbox_usage_export::OUTBOX,
    super::sandbox_usage_export::STREAMS,
    super::sandbox_usage_export::INVENTORY,
    "sandbox_profile_revisions",
    "sandbox_profiles",
    "sandbox_retained_availability",
    "sandbox_sessions",
    "sandboxes",
    super::home_backups::SCHEDULES_COLLECTION,
    super::home_backups::BACKUPS_COLLECTION,
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
    super::released_runtimes::COLLECTION,
    super::released_runtimes::TRANSITIONS_COLLECTION,
];
pub(crate) const JOURNAL_COLLECTIONS: &[&str] =
    super::account_deletions::worker_schema::COLLECTIONS;
// These stores activate separately. Existing credentials still start while disabled.
pub(crate) const OPTIONAL_COLLECTIONS: &[&str] = &[
    super::account_deletions::worker_schema::WORK,
    super::account_deletions::worker_schema::EFFECTS,
    super::migrations::usage_projections::COLLECTION,
    retained_consents::COLLECTION,
    super::migrations::measured_custody::COLLECTION,
    super::migrations::measured_batches::COLLECTION,
    super::migrations::measured_index::PROJECTIONS,
    super::migrations::measured_index::MANIFESTS,
    super::migrations::measured_index::BATCHES,
    super::agent_log_segments::active_proof::COLLECTION,
    super::sandbox_usage_export::CAPTURES,
    super::sandbox_usage_export::EVIDENCE,
    super::sandbox_usage_export::OUTBOX,
    super::sandbox_usage_export::STREAMS,
    super::sandbox_usage_export::INVENTORY,
    super::home_backups::SCHEDULES_COLLECTION,
    super::home_backups::BACKUPS_COLLECTION,
    super::allocation_buckets::COLLECTION,
    super::agent_observation::COLLECTION,
    super::released_runtimes::COLLECTION,
    super::released_runtimes::TRANSITIONS_COLLECTION,
    super::creation_outcomes::COLLECTION,
    // Resource execution reads its activation record on ordinary share and
    // revocation paths; credentials issued before it still start.
    "resource_execution_effects_v1",
    "resource_execution_grants_v1",
    "resource_execution_records_v1",
    "resource_revocation_operations_v1",
];
const USAGE_PROJECTIONS: &str = super::migrations::usage_projections::COLLECTION;
const USAGE_PROJECTION_ACTIONS: &[&str] = &["find", "insert", "remove", "listIndexes"];
mod measured;
#[cfg(test)]
use measured::MEASURED_CUSTODY;
use measured::{MEASURED_APPEND_ONLY, MEASURED_MANIFESTS};
pub(super) const MEASURED_MANIFEST_ACTIONS: &[&str] = &["find", "insert", "update", "listIndexes"];
pub(super) const MEASURED_CUSTODY_ACTIONS: &[&str] = &["find", "insert", "listIndexes"];
const MIGRATION_COLLECTION: &str = "agent_manager_migrations";
const MIGRATION_ACTIONS: &[&str] = &["find", "insert", "update", "createCollection"];
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
// The Activity operation roles add metadata reads and, for build mode, ledger
// writes. Ordinary startup keeps the original runtime role below.
const ACTIVITY_INSPECTION_ACTIONS: &[&str] = &["collStats"];
pub(crate) fn role(database: &str) -> Document {
    role_with_index_operation(database, &[], false, true)
}

#[cfg(test)]
pub(crate) fn activity_index_inspection_role(database: &str) -> Document {
    role_with_index_operation(database, &["activity"], false, true)
}

#[cfg(test)]
pub(crate) fn activity_index_job_role(database: &str) -> Document {
    role_with_index_operation(database, &["activity"], true, true)
}

mod runtime_role;
use runtime_role::{role_with_index_operation, validate_index_role};

pub(crate) fn validate(status: &Document, database: &str) -> Result<(), StoreError> {
    validate_index_role(status, database, &[], false)
}

#[cfg(test)]
pub(crate) fn validate_activity_index_inspection(
    status: &Document,
    database: &str,
) -> Result<(), StoreError> {
    validate_index_role(status, database, &["activity"], false)
}

#[cfg(test)]
pub(crate) fn validate_activity_index_job(
    status: &Document,
    database: &str,
) -> Result<(), StoreError> {
    validate_index_role(status, database, &["activity"], true)
}

fn required_role(database: &str) -> Document {
    role_with_index_operation(database, &[], false, false)
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
                &required_role(database.name()),
            )
            .map_err(|error| StoreError::Unavailable(error.into()))
        }
    }
}

pub(crate) fn verify_index_operation_authority(
    status: &Document,
    database: &str,
    authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    collections: &[&str],
    build: bool,
) -> Result<(), StoreError> {
    if collections.is_empty() || collections.iter().any(|name| !COLLECTIONS.contains(name)) {
        return Err(StoreError::Unavailable(
            "index operation collection is outside the AM schema".into(),
        ));
    }
    let role = role_with_index_operation(database, collections, build, false);
    match authority {
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated => {
            validate_index_role(status, database, collections, build)
        }
        cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema => {
            cma_service_auth::mongodb_authority::verify_shared_schema(status, database, &role)
                .map_err(|error| StoreError::Unavailable(error.into()))?;
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "agent_lifecycle_contract.rs"]
pub(super) mod agent_lifecycle_contract;
#[cfg(test)]
#[path = "isolation/allocation_tests.rs"]
mod allocation_tests;
#[cfg(test)]
#[path = "lifecycle_children_contract.rs"]
mod lifecycle_children_contract;
#[cfg(test)]
#[path = "isolation/measured_tests.rs"]
mod measured_tests;
#[cfg(test)]
#[path = "profile_internal_contract.rs"]
mod profile_internal_contract;
#[cfg(test)]
mod segments;
#[cfg(test)]
#[path = "shared_schema_contract.rs"]
mod shared_schema_contract;
#[cfg(test)]
mod tests;
