//! Agent Manager's exact schema authority. Legacy names retain existing data IDs.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) const COLLECTIONS: &[&str] = &[
    super::native_warm_pools::COLLECTION,
    "runtime_backend_routes",
    "activity",
    "agent_artifacts",
    "agent_channel_usage",
    "agent_command_orders",
    "agent_commands",
    "agent_creation_provenance",
    "agent_access_tools",
    "agent_access_requests",
    "agent_log",
    "agent_lifecycle_operations",
    "agent_profile_revisions",
    "agent_profiles",
    "agent_reuse_bindings",
    "agents",
    "auth_api_bearers",
    "auth_actor_links",
    "auth_browser_logins",
    "auth_login_attempts",
    "auth_subjects",
    "binding_conversations",
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
    "provider_tool_opens",
    "resource_policies",
    "resource_share_commands",
    "resource_shares",
    "sandbox_admissions",
    "sandbox_controls",
    "sandbox_credential_revocations",
    "sandbox_credentials",
    "sandbox_events",
    "sandbox_profile_revisions",
    "sandbox_profiles",
    "sandbox_retained_availability",
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
            || (!collection.is_empty() && !COLLECTIONS.contains(&collection))
        {
            return Err(denied());
        }
        for action in grant.get_array("actions").map_err(|_| denied())? {
            let action = action.as_str().ok_or_else(&denied)?;
            if !(if collection.is_empty() {
                action == "listCollections"
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
mod agent_lifecycle_contract;
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
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires authenticated disposable service Mongo fixture"]
    async fn real_agent_manager_credential_cannot_read_gateway_or_images() {
        use mongodb::{
            options::{ClientOptions, Credential},
            Client,
        };
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_am_contract_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&name);
        db.run_command(role(&name)).await.unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        db.run_command(doc!{"createUser":"agent-runtime","pwd":&password,"roles":[{"role":"cma_am_runtime","db":&name}]}).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username("agent-runtime".to_owned())
                .password(password)
                .source(name.clone())
                .build(),
        );
        let runtime = Client::with_options(options).unwrap();
        let isolated = runtime.database(&name);
        verify(&isolated).await.unwrap();
        assert!(verify(&db).await.is_err());
        let key = crate::store::AuthEncryptionKey::from_base64(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [7_u8; 32],
        ))
        .unwrap();
        let before = db.list_collection_names().await.unwrap();
        let broad = crate::store::MongoStores::connect(
            &uri,
            &name,
            "refused-bootstrap",
            &key,
            std::time::Duration::from_secs(86_400),
            ("runtime-isolation-contract", None),
        )
        .await;
        assert!(
            matches!(broad, Err(StoreError::Unavailable(ref detail)) if detail.contains("not isolated"))
        );
        assert_eq!(db.list_collection_names().await.unwrap(), before);
        for _ in 0..2 {
            let stores = crate::store::MongoStores::open(
                runtime.clone(),
                &name,
                "isolated-bootstrap",
                &key,
                std::time::Duration::from_secs(86_400),
                std::sync::Arc::new(super::super::checkpoint_storage::RetiredCheckpointStorage),
                ("runtime-isolation-contract", None),
            )
            .await
            .expect("the complete production store assembly must initialize with runtime grants");
            stores.ping().await.unwrap();
            assert!(stores.sandbox_model.warm_runtime_version.is_none());
            assert!(stores.sandbox_model.warm_home_policy.is_none());
        }
        use layer3_adapter::hosted::RuntimeRoutingStore;
        let routing =
            super::super::runtime_routing::MongoRuntimeRouting::open(&isolated, "fixture", true)
                .await
                .unwrap();
        assert!(routing.select("new-sandbox", 1, true).await.unwrap());
        let restarted =
            super::super::runtime_routing::MongoRuntimeRouting::open(&isolated, "fixture", false)
                .await
                .unwrap();
        assert!(restarted.select("new-sandbox", 1, false).await.unwrap());
        assert!(!restarted.select("new-sandbox", 2, false).await.unwrap());
        assert!(restarted.select("overflow", u64::MAX, true).await.is_err());

        profile_internal_contract::verify(&runtime, &name).await;
        agent_lifecycle_contract::verify(&isolated).await;
        lifecycle_children_contract::verify(&runtime, &name).await;

        for foreign in [
            "gateway_channel_records",
            "gateway_connection_records",
            "gateway_interaction_records",
            "eg_channel_records",
            "eg_auth_sessions",
            "im_images",
            "im_builds",
            "im_auth_sessions",
        ] {
            db.collection::<Document>(foreign)
                .insert_one(doc! {"_id":"private"})
                .await
                .unwrap();
            let error = isolated
                .collection::<Document>(foreign)
                .find_one(doc! {})
                .await
                .unwrap_err();
            assert!(
                matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==13)
            );
        }
        db.drop().await.unwrap();
    }
    #[test]
    fn rejects_foreign_and_database_wide_privileges() {
        let grants = role("cma").get_array("privileges").unwrap().clone();
        let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager","db":"cma"}],"authenticatedUserPrivileges":grants.clone()}};
        validate(&status, "cma").unwrap();
        for collection in [
            "im_images",
            "eg_channel_records",
            "gateway_channel_records",
            "",
        ] {
            let mut changed = grants.clone();
            changed.push(
                doc! {"resource":{"db":"cma","collection":collection},"actions":["find"]}.into(),
            );
            assert!(validate(&doc!{"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":changed}},"cma").is_err());
        }
    }

    #[test]
    fn warm_pool_target_grants_are_required_without_database_wide_access() {
        let mut grants = role("cma").get_array("privileges").unwrap().clone();
        let target = super::super::native_warm_pools::COLLECTION;
        let grant = grants
            .iter()
            .find(|grant| {
                grant
                    .as_document()
                    .unwrap()
                    .get_document("resource")
                    .unwrap()
                    .get_str("collection")
                    .unwrap()
                    == target
            })
            .expect("the generated runtime role includes durable native pool targets");
        assert!(grant
            .as_document()
            .unwrap()
            .get_array("actions")
            .unwrap()
            .iter()
            .any(|action| action.as_str() == Some("createIndex")));
        grants.retain(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                != target
        });
        let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":grants}};
        assert!(validate(&status, "cma").is_err());
    }

    #[test]
    fn index_removal_is_confined_to_the_binding_migration() {
        let grants = role("cma").get_array("privileges").unwrap().clone();
        for collection in ["sandboxes", "agents", ""] {
            let mut changed = grants.clone();
            changed.push(
                doc! {"resource":{"db":"cma","collection":collection},"actions":["dropIndex"]}
                    .into(),
            );
            let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":changed}};
            assert!(validate(&status, "cma").is_err());
        }
        let mut missing = grants;
        for grant in &mut missing {
            let grant = grant.as_document_mut().unwrap();
            grant
                .get_array_mut("actions")
                .unwrap()
                .retain(|action| action.as_str() != Some("dropIndex"));
        }
        let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":missing}};
        assert!(validate(&status, "cma").is_err());
    }
}
