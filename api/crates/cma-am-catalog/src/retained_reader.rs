//! Hydrate inert drafts only for canonical, fenced erasure tombstones.
use layer2_core::{
    profiles::{
        store::ProfileStoreError, AgentProfileContent, AgentProvider, ModelAccess, PermissionLevel,
        SandboxProfileContent, ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent,
    },
    seconds_from_rfc3339, DELETED_ACCOUNT_SUBJECT,
};
use mongodb::bson::Document;

use crate::DOCUMENT_SCHEMA_VERSION;
use cma_am_mongo_primitives::{record_bson, require_version};

fn corrupt() -> ProfileStoreError {
    ProfileStoreError::Corrupt("invalid retained Profile erasure tombstone".into())
}

pub(super) fn is_retained(document: &Document) -> Result<bool, ProfileStoreError> {
    if !document.contains_key("account_erasure_retained") {
        return Ok(false);
    }
    require_version(document, DOCUMENT_SCHEMA_VERSION).map_err(ProfileStoreError::Corrupt)?;
    if document.get_bool("account_erasure_retained").ok() != Some(true)
        || document.get_bool("account_erasure_fenced").ok() != Some(true)
        || document.get_datetime("account_erasure_fenced_at").is_err()
        || !matches!(
            document.get_str("kind"),
            Ok("agent" | "workspace" | "sandbox")
        )
    {
        return Err(corrupt());
    }
    let record = document.get_document("record").map_err(|_| corrupt())?;
    if record.contains_key("draft") || record.contains_key("forked_from") {
        return Err(corrupt());
    }
    for (field, expected) in [
        ("owner", DELETED_ACCOUNT_SUBJECT),
        ("name", ""),
        ("description", ""),
        ("visibility", "private"),
    ] {
        if document.get_str(field).ok() != Some(expected)
            || record.get_str(field).ok() != Some(expected)
        {
            return Err(corrupt());
        }
    }
    let retired_at = document.get_str("retired_at").map_err(|_| corrupt())?;
    if seconds_from_rfc3339(retired_at).is_none()
        || record.get_str("retired_at").ok() != Some(retired_at)
        || document.get_str("_id").ok().filter(|id| !id.is_empty())
            != record.get_str("id").ok().filter(|id| !id.is_empty())
    {
        return Err(corrupt());
    }
    Ok(true)
}

pub(super) fn hydrate(document: &Document) -> Result<Option<Document>, ProfileStoreError> {
    if !is_retained(document)? {
        return Ok(None);
    }
    let draft = match document.get_str("kind").map_err(|_| corrupt())? {
        "agent" => record_bson(&AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Minimal,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        }),
        "workspace" => record_bson(&WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "deleted-profile".into(),
            },
            name: "deleted-profile".into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        }),
        "sandbox" => record_bson(&SandboxProfileContent {
            cpu_millis: 0,
            memory_mib: 0,
            runtime_image: None,
            nyxid_services: vec![],
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 0,
            max_agents_per_workspace: 0,
            workspace_profiles: vec![],
            software_install_commands: vec![],
            environment_variables: vec![],
        }),
        _ => return Err(corrupt()),
    }
    .map_err(ProfileStoreError::Corrupt)?;
    // This copy exists only for typed readers. No draft is restored in storage.
    let mut hydrated = document.clone();
    hydrated
        .get_document_mut("record")
        .map_err(|_| corrupt())?
        .insert("draft", draft);
    Ok(Some(hydrated))
}
