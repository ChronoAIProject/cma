//! Resume exact admitted Launch effects only after retained pins are canonical.
use super::{agent_profiles, exact, majority_collection, sandbox_profiles, workspace_profiles};
use crate::store::{profile_documents, schema::DOCUMENT_SCHEMA_VERSION, trigger_definitions};
use layer2_core::{
    profiles::content_digest,
    sandbox::{
        store::{AgentAdmission, SandboxStoreError},
        AgentOrigin, Sandbox, SandboxOrigin, Workspace, WorkspaceOrigin,
    },
    trigger_domain::{
        AgentReusePolicy, Launch, LaunchEffectKind, LaunchEffectState, LaunchState, TriggerRevision,
    },
    SubjectId,
};
use mongodb::{
    bson::{doc, Bson, Document},
    ClientSession, Database,
};
#[path = "declared.rs"]
mod declared;

#[derive(Clone, Copy)]
pub(in crate::store) enum Decision {
    Allowed,
    Refused,
    Pending,
}
impl Decision {
    pub(in crate::store) fn require(self) -> Result<(), SandboxStoreError> {
        match self {
            Self::Allowed => Ok(()),
            Self::Refused => Err(SandboxStoreError::Conflict),
            Self::Pending => Err(SandboxStoreError::Unavailable(
                "frozen Launch Profile erasure has not finished retaining its pins".into(),
            )),
        }
    }
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Refused, _) | (_, Self::Refused) => Self::Refused,
            (Self::Pending, _) | (_, Self::Pending) => Self::Pending,
            _ => Self::Allowed,
        }
    }
}

pub(in crate::store) async fn sandbox(
    db: &Database,
    candidate: &Sandbox,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let SandboxOrigin::Trigger { launch_id } = &candidate.origin else {
        return Ok(Decision::Refused);
    };
    let Some((_, revision)) = accepted(
        db,
        &candidate.owner,
        launch_id,
        LaunchEffectKind::CreateSandbox,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let pin = &candidate.profile;
    let expected = &revision.content.target.sandbox_profile;
    if pin.id != expected.id || pin.revision != expected.revision {
        return Ok(Decision::Refused);
    }
    sandbox_pin(
        db,
        pin.id.as_str(),
        pin.revision,
        Some(&pin.digest),
        session,
    )
    .await
}

/// An admitted inline Launch can independently carry an Image Manager pin.
pub(in crate::store) async fn inline_image(
    db: &Database,
    sandbox: &Sandbox,
    launch_id: &str,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    let Some((_, revision)) = accepted(
        db,
        &sandbox.owner,
        launch_id,
        LaunchEffectKind::CreateSandbox,
        session,
    )
    .await?
    else {
        return Ok(false);
    };
    let target = &revision.content.target;
    let Some(pin) = target
        .inline
        .as_ref()
        .and_then(|inline| inline.image.as_ref())
        .and_then(|image| image.reference.as_ref())
    else {
        return Ok(false);
    };
    Ok(sandbox.profile.id == target.sandbox_profile.id
        && sandbox.profile.revision == target.sandbox_profile.revision
        && sandbox.profile.content.runtime_image.as_ref() == Some(pin))
}

pub(in crate::store) async fn workspace(
    db: &Database,
    candidate: &Workspace,
    parent: &Sandbox,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    if matches!(candidate.origin, WorkspaceOrigin::SandboxProfile { .. }) {
        return declared::workspace(db, candidate, parent, session).await;
    }
    let WorkspaceOrigin::Trigger { launch_id } = &candidate.origin else {
        return Ok(Decision::Refused);
    };
    let Some((launch, revision)) = accepted(
        db,
        &candidate.owner,
        launch_id,
        LaunchEffectKind::CreateWorkspace,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let pin = &candidate.profile;
    let expected = &revision.content.target.workspace_profile;
    if launch.resolution.sandbox.id() != Some(&candidate.sandbox_id)
        || pin.id != expected.id
        || pin.revision != expected.revision
    {
        return Ok(Decision::Refused);
    }
    workspace_pin(
        db,
        pin.id.as_str(),
        pin.revision,
        Some(&pin.digest),
        session,
    )
    .await
}

pub(in crate::store) async fn agent(
    db: &Database,
    admission: &AgentAdmission,
    parent: &Sandbox,
    workspace: &Workspace,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    if matches!(admission.origin, AgentOrigin::WorkspaceProfile { .. }) {
        return declared::agent(db, admission, parent, workspace, session).await;
    }
    let AgentOrigin::Trigger {
        trigger_id,
        launch_id,
    } = &admission.origin
    else {
        return Ok(Decision::Refused);
    };
    let Some((launch, revision)) = accepted(
        db,
        &admission.owner,
        launch_id,
        LaunchEffectKind::CreateAgent,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let Ok(digest) = content_digest(&admission.origin) else {
        return Ok(Decision::Refused);
    };
    let prefix = if revision.content.reuse.agent == AgentReusePolicy::ResumeExisting {
        "trigger-profile"
    } else {
        "trigger"
    };
    let pin = &admission.profile;
    let expected = &revision.content.target.agent_profile;
    if trigger_id != launch.trigger_id.as_str()
        || launch.resolution.workspace.id() != Some(&admission.workspace_id)
        || launch.resolution.sandbox.id() != Some(&parent.id)
        || admission.creation_key != format!("{prefix}:{digest}")
        || pin.id != expected.id
        || pin.revision != expected.revision
    {
        return Ok(Decision::Refused);
    }
    agent_pin(
        db,
        pin.id.as_str(),
        pin.revision,
        Some(&pin.digest),
        session,
    )
    .await
}

async fn accepted(
    db: &Database,
    owner: &SubjectId,
    id: &str,
    kind: LaunchEffectKind,
    session: &mut ClientSession,
) -> mongodb::error::Result<Option<(Launch, TriggerRevision)>> {
    let Some(row) = majority_collection::<Document>(db, "trigger_launches")
        .find_one(doc! {"_id":id,"owner":owner.as_str()})
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let Ok(launch) = trigger_definitions::decode_launch_aggregate(&row) else {
        return Ok(None);
    };
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339());
    if launch.owner != *owner
        || !matches!(
            launch.state,
            LaunchState::Resolving
                | LaunchState::SandboxPending
                | LaunchState::WorkspacePending
                | LaunchState::AgentPending
        )
        || launch
            .work
            .lease
            .as_ref()
            .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
            .zip(now)
            .is_none_or(|(expires, now)| expires <= now)
        || launch.work.effect.as_ref().is_none_or(|effect| {
            effect.kind != kind
                || effect.intent_key != kind.intent_key(&launch.id)
                || !matches!(
                    effect.state,
                    LaunchEffectState::Executing | LaunchEffectState::RecoveryRequired
                )
        })
    {
        return Ok(None);
    }
    let receipt = majority_collection::<Document>(db, "trigger_launch_input_protocols")
        .find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?;
    if trigger_definitions::require_input_protocol_document(&launch, receipt).is_err() {
        return Ok(None);
    }
    Ok(
        trigger_definitions::surviving_launch_revision(db, &launch, session)
            .await?
            .map(|revision| (launch, revision)),
    )
}

async fn sandbox_pin(
    db: &Database,
    id: &str,
    revision: u32,
    digest: Option<&str>,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let Some(row) = exact(db, "sandbox_profile_revisions", id, revision, session).await? else {
        return Ok(Decision::Refused);
    };
    let Ok(pin) = sandbox_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if digest.is_some_and(|expected| expected != pin.digest) {
        return Ok(Decision::Refused);
    }
    let mut decision = aggregate(db, "sandbox_profiles", id, session).await?;
    for child in &pin.content.workspace_profiles {
        decision = decision
            .and(workspace_pin(db, child.id.as_str(), child.revision, None, session).await?);
    }
    Ok(decision)
}

async fn workspace_pin(
    db: &Database,
    id: &str,
    revision: u32,
    digest: Option<&str>,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let Some(row) = exact(db, "workspace_profile_revisions", id, revision, session).await? else {
        return Ok(Decision::Refused);
    };
    let Ok(pin) = workspace_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if digest.is_some_and(|expected| expected != pin.digest) {
        return Ok(Decision::Refused);
    }
    let mut decision = aggregate(db, "workspace_profiles", id, session).await?;
    for child in &pin.content.agent_profiles {
        decision =
            decision.and(agent_pin(db, child.id.as_str(), child.revision, None, session).await?);
    }
    Ok(decision)
}

async fn agent_pin(
    db: &Database,
    id: &str,
    revision: u32,
    digest: Option<&str>,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let Some(row) = exact(db, "agent_profile_revisions", id, revision, session).await? else {
        return Ok(Decision::Refused);
    };
    let Ok(pin) = agent_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if digest.is_some_and(|expected| expected != pin.digest) {
        return Ok(Decision::Refused);
    }
    aggregate(db, "agent_profiles", id, session).await
}

async fn aggregate(
    db: &Database,
    collection: &str,
    id: &str,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let Some(row) = majority_collection::<Document>(db, collection)
        .find_one(doc! {"_id":id,"document_schema_version":DOCUMENT_SCHEMA_VERSION})
        .session(session)
        .await?
    else {
        return Ok(Decision::Refused);
    };
    let fenced = match row.get("account_erasure_fenced") {
        None | Some(Bson::Boolean(false)) => false,
        Some(Bson::Boolean(true)) if row.get_datetime("account_erasure_fenced_at").is_ok() => true,
        _ => return Ok(Decision::Refused),
    };
    let valid = match collection {
        "sandbox_profiles" => sandbox_profiles::read_profile(&row).is_ok(),
        "workspace_profiles" => workspace_profiles::read_profile(&row).is_ok(),
        "agent_profiles" => agent_profiles::read_profile(&row).is_ok(),
        _ => false,
    };
    if !valid {
        return Ok(Decision::Refused);
    }
    if !fenced || profile_documents::is_retained_profile(&row).unwrap_or(false) {
        Ok(Decision::Allowed)
    } else {
        // A pending fenced source cannot create a pin after graph discovery.
        // The unchanged worker retry path resumes after canonical retention.
        Ok(Decision::Pending)
    }
}
