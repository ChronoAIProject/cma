use layer2_core::ids::SubjectId;
use layer2_core::profiles::{
    AgentProfile, AgentProfileRevision, AgentProfileStore, ProfileStoreError, ProfileVisibility,
    SandboxProfile, SandboxProfileRevision, SandboxProfileStore, WorkspaceProfile,
    WorkspaceProfileRevision, WorkspaceProfileStore,
};
use layer2_core::trigger_domain::{
    ResolvedTriggerProfiles, TriggerProfileReader, TriggerReferenceError, TriggerStoreError,
    TriggerTarget,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::{ClientSession, Collection};

use super::super::sandbox_mongo::MongoProfileStore;
use super::super::schema::DOCUMENT_SCHEMA_VERSION;
use super::super::{agent_profiles, sandbox_profiles, workspace_profiles};
use super::{driver_error, MongoTriggerDefinitionStore};

pub(super) enum TransactionProfileError {
    Domain(TriggerStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn authorize(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    target: &TriggerTarget,
    session: &mut ClientSession,
) -> Result<ResolvedTriggerProfiles, TransactionProfileError> {
    let sandbox = current_sandbox(store, &target.sandbox_profile.id.0, session).await?;
    require_public(
        owner,
        &sandbox.owner,
        sandbox.visibility,
        sandbox.retired_at.as_deref(),
        "target.sandbox_profile",
        target.inline.is_some(),
    )?;
    fence(
        &store.profiles.sandbox_profiles,
        &target.sandbox_profile.id.0,
        sandbox.owner.as_str(),
        target.inline.is_some(),
        session,
    )
    .await?;
    let sandbox_revision = sandbox_revision_in_transaction(
        store,
        &target.sandbox_profile.id.0,
        target.sandbox_profile.revision,
        session,
    )
    .await?;
    let mut sandbox_workspaces =
        Vec::with_capacity(sandbox_revision.content.workspace_profiles.len());
    for reference in &sandbox_revision.content.workspace_profiles {
        sandbox_workspaces.push(
            workspace_revision_in_transaction(
                store,
                &reference.id.0,
                reference.revision,
                "target.sandbox_profile.workspace_profiles",
                session,
            )
            .await?,
        );
    }

    let workspace = current_workspace(store, &target.workspace_profile.id.0, session).await?;
    require_public(
        owner,
        &workspace.owner,
        workspace.visibility,
        workspace.retired_at.as_deref(),
        "target.workspace_profile",
        target.inline.is_some(),
    )?;
    fence(
        &store.profiles.workspace_profiles,
        &target.workspace_profile.id.0,
        workspace.owner.as_str(),
        target.inline.is_some(),
        session,
    )
    .await?;
    let workspace_revision = workspace_revision_in_transaction(
        store,
        &target.workspace_profile.id.0,
        target.workspace_profile.revision,
        "target.workspace_profile",
        session,
    )
    .await?;

    let agent = current_agent(store, &target.agent_profile.id.0, session).await?;
    require_public(
        owner,
        &agent.owner,
        agent.visibility,
        agent.retired_at.as_deref(),
        "target.agent_profile",
        target.inline.is_some(),
    )?;
    fence(
        &store.profiles.agent_profiles,
        &target.agent_profile.id.0,
        agent.owner.as_str(),
        target.inline.is_some(),
        session,
    )
    .await?;
    let agent_revision = agent_revision_in_transaction(
        store,
        &target.agent_profile.id.0,
        target.agent_profile.revision,
        session,
    )
    .await?;

    Ok(ResolvedTriggerProfiles {
        sandbox: sandbox_revision,
        sandbox_workspaces,
        workspace: workspace_revision,
        agent: agent_revision,
    })
}

pub(super) async fn read_exact(
    store: &MongoTriggerDefinitionStore,
    target: &TriggerTarget,
) -> Result<ResolvedTriggerProfiles, TriggerStoreError> {
    let sandbox_document = store
        .profiles
        .sandbox_revisions
        .find_one(doc! {
            "profile_id": target.sandbox_profile.id.as_str(),
            "revision": i64::from(target.sandbox_profile.revision),
        })
        .await
        .map_err(|error| driver_error("read Trigger Sandbox Profile revision", error))?
        .ok_or_else(|| unavailable("target.sandbox_profile"))?;
    let sandbox =
        sandbox_profiles::read_revision(&sandbox_document).map_err(stored_profile_error)?;
    let mut sandbox_workspaces = Vec::with_capacity(sandbox.content.workspace_profiles.len());
    for reference in &sandbox.content.workspace_profiles {
        let document = store
            .profiles
            .workspace_revisions
            .find_one(doc! {
                "profile_id": reference.id.as_str(),
                "revision": i64::from(reference.revision),
            })
            .await
            .map_err(|error| driver_error("read nested Trigger Workspace Profile revision", error))?
            .ok_or_else(|| unavailable("target.sandbox_profile.workspace_profiles"))?;
        sandbox_workspaces
            .push(workspace_profiles::read_revision(&document).map_err(stored_profile_error)?);
    }
    let workspace = store
        .profiles
        .workspace_revisions
        .find_one(doc! {
            "profile_id": target.workspace_profile.id.as_str(),
            "revision": i64::from(target.workspace_profile.revision),
        })
        .await
        .map_err(|error| driver_error("read Trigger Workspace Profile revision", error))?
        .ok_or_else(|| unavailable("target.workspace_profile"))?;
    let agent = store
        .profiles
        .agent_revisions
        .find_one(doc! {
            "profile_id": target.agent_profile.id.as_str(),
            "revision": i64::from(target.agent_profile.revision),
        })
        .await
        .map_err(|error| driver_error("read Trigger Agent Profile revision", error))?
        .ok_or_else(|| unavailable("target.agent_profile"))?;
    Ok(ResolvedTriggerProfiles {
        sandbox,
        sandbox_workspaces,
        workspace: workspace_profiles::read_revision(&workspace).map_err(stored_profile_error)?,
        agent: agent_profiles::read_revision(&agent).map_err(stored_profile_error)?,
    })
}

#[async_trait::async_trait]
impl TriggerProfileReader for MongoProfileStore {
    async fn read_trigger_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError> {
        let sandbox_field = "target.sandbox_profile";
        let sandbox_workspaces_field = "target.sandbox_profile.workspace_profiles";
        let workspace_field = "target.workspace_profile";
        let agent_field = "target.agent_profile";

        let sandbox = self
            .sandbox_profile_revision(&target.sandbox_profile.id, target.sandbox_profile.revision)
            .await
            .map_err(|_| reference_unavailable(sandbox_field))?
            .ok_or_else(|| reference_unavailable(sandbox_field))?;
        let mut sandbox_workspaces = Vec::with_capacity(sandbox.content.workspace_profiles.len());
        for reference in &sandbox.content.workspace_profiles {
            let revision = self
                .workspace_profile_revision(&reference.id, reference.revision)
                .await
                .map_err(|_| reference_unavailable(sandbox_workspaces_field))?
                .ok_or_else(|| reference_unavailable(sandbox_workspaces_field))?;
            sandbox_workspaces.push(revision);
        }
        let workspace = self
            .workspace_profile_revision(
                &target.workspace_profile.id,
                target.workspace_profile.revision,
            )
            .await
            .map_err(|_| reference_unavailable(workspace_field))?
            .ok_or_else(|| reference_unavailable(workspace_field))?;
        let agent = self
            .agent_profile_revision(&target.agent_profile.id, target.agent_profile.revision)
            .await
            .map_err(|_| reference_unavailable(agent_field))?
            .ok_or_else(|| reference_unavailable(agent_field))?;

        Ok(ResolvedTriggerProfiles {
            sandbox,
            sandbox_workspaces,
            workspace,
            agent,
        })
    }
}

async fn current_sandbox(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    session: &mut ClientSession,
) -> Result<SandboxProfile, TransactionProfileError> {
    let document = store
        .profiles
        .sandbox_profiles
        .find_one(doc! { "_id": id })
        .session(session)
        .await
        .map_err(TransactionProfileError::Driver)?
        .ok_or_else(|| transaction_unavailable("target.sandbox_profile"))?;
    sandbox_profiles::read_profile(&document)
        .map_err(stored_profile_error)
        .map_err(TransactionProfileError::Domain)
}

async fn current_workspace(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    session: &mut ClientSession,
) -> Result<WorkspaceProfile, TransactionProfileError> {
    let document = store
        .profiles
        .workspace_profiles
        .find_one(doc! { "_id": id })
        .session(session)
        .await
        .map_err(TransactionProfileError::Driver)?
        .ok_or_else(|| transaction_unavailable("target.workspace_profile"))?;
    workspace_profiles::read_profile(&document)
        .map_err(stored_profile_error)
        .map_err(TransactionProfileError::Domain)
}

async fn current_agent(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    session: &mut ClientSession,
) -> Result<AgentProfile, TransactionProfileError> {
    let document = store
        .profiles
        .agent_profiles
        .find_one(doc! { "_id": id })
        .session(session)
        .await
        .map_err(TransactionProfileError::Driver)?
        .ok_or_else(|| transaction_unavailable("target.agent_profile"))?;
    agent_profiles::read_profile(&document)
        .map_err(stored_profile_error)
        .map_err(TransactionProfileError::Domain)
}

async fn sandbox_revision_in_transaction(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    revision: u32,
    session: &mut ClientSession,
) -> Result<SandboxProfileRevision, TransactionProfileError> {
    let document = exact_revision(
        &store.profiles.sandbox_revisions,
        id,
        revision,
        "target.sandbox_profile",
        session,
    )
    .await?;
    sandbox_profiles::read_revision(&document)
        .map_err(stored_profile_error)
        .map_err(TransactionProfileError::Domain)
}

async fn workspace_revision_in_transaction(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    revision: u32,
    field: &'static str,
    session: &mut ClientSession,
) -> Result<WorkspaceProfileRevision, TransactionProfileError> {
    let document = exact_revision(
        &store.profiles.workspace_revisions,
        id,
        revision,
        field,
        session,
    )
    .await?;
    workspace_profiles::read_revision(&document)
        .map_err(stored_profile_error)
        .map_err(TransactionProfileError::Domain)
}

async fn agent_revision_in_transaction(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    revision: u32,
    session: &mut ClientSession,
) -> Result<AgentProfileRevision, TransactionProfileError> {
    let document = exact_revision(
        &store.profiles.agent_revisions,
        id,
        revision,
        "target.agent_profile",
        session,
    )
    .await?;
    agent_profiles::read_revision(&document)
        .map_err(stored_profile_error)
        .map_err(TransactionProfileError::Domain)
}

async fn exact_revision(
    collection: &Collection<Document>,
    id: &str,
    revision: u32,
    field: &'static str,
    session: &mut ClientSession,
) -> Result<Document, TransactionProfileError> {
    collection
        .find_one(doc! {
            "profile_id": id,
            "revision": i64::from(revision),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        })
        .session(session)
        .await
        .map_err(TransactionProfileError::Driver)?
        .ok_or_else(|| transaction_unavailable(field))
}

async fn fence(
    collection: &Collection<Document>,
    id: &str,
    profile_owner: &str,
    allow_owned_inline: bool,
    session: &mut ClientSession,
) -> Result<(), TransactionProfileError> {
    let mut filter = doc! {
        "_id": id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": profile_owner,
        "retired_at": Bson::Null,
        "record.retired_at": Bson::Null,
    };
    if allow_owned_inline {
        filter.insert("visibility", doc! { "$in": ["public", "private"] });
        filter.insert("record.visibility", doc! { "$in": ["public", "private"] });
    } else {
        filter.insert("visibility", "public");
        filter.insert("record.visibility", "public");
    }
    let result = collection
        .update_one(
            filter,
            doc! { "$inc": { "trigger_publication_fence": 1_i64 } },
        )
        .session(session)
        .await
        .map_err(TransactionProfileError::Driver)?;
    if result.matched_count == 1 {
        Ok(())
    } else {
        Err(transaction_unavailable("target profile"))
    }
}

fn require_public(
    trigger_owner: &SubjectId,
    profile_owner: &SubjectId,
    visibility: ProfileVisibility,
    retired_at: Option<&str>,
    field: &'static str,
    allow_owned_inline: bool,
) -> Result<(), TransactionProfileError> {
    if retired_at.is_some()
        || (visibility == ProfileVisibility::Private
            && (!allow_owned_inline || trigger_owner != profile_owner))
    {
        return Err(transaction_unavailable(field));
    }
    if visibility == ProfileVisibility::Private && !allow_owned_inline {
        return Err(TransactionProfileError::Domain(
            TriggerReferenceError::NotPublic { field }.into(),
        ));
    }
    Ok(())
}

fn unavailable(field: &'static str) -> TriggerStoreError {
    TriggerReferenceError::Unavailable { field }.into()
}

fn reference_unavailable(field: &'static str) -> TriggerReferenceError {
    TriggerReferenceError::Unavailable { field }
}

fn transaction_unavailable(field: &'static str) -> TransactionProfileError {
    TransactionProfileError::Domain(unavailable(field))
}

fn stored_profile_error(error: ProfileStoreError) -> TriggerStoreError {
    TriggerStoreError::Corrupt(format!("referenced profile record is invalid: {error}"))
}
