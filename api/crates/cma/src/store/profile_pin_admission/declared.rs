//! Surviving durable roots can finish their exact declarations through erasure.
use super::*;
use layer2_core::profiles::{AgentProfileSnapshot, WorkspaceProfileSnapshot};

async fn existing(
    db: &Database,
    parent: &Sandbox,
    decision: Decision,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    if !matches!(decision, Decision::Pending) {
        return Ok(decision);
    }
    if let Some(row) = majority_collection::<Document>(db, "resource_policies")
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut *session)
        .await?
    {
        let Ok(policy) = crate::store::resource_access::records::read_policy(&row) else {
            return Ok(Decision::Refused);
        };
        match policy.personal_owner_of(parent) {
            Ok(false) => return Ok(Decision::Allowed),
            Ok(true) => {}
            Err(_) => return Ok(Decision::Refused),
        }
    }
    // Only surviving personal roots belong to discovery's retained closure.
    // Read committed owner evidence in this admission snapshot without taking
    // the separate new-Sandbox fence or modifying any source catalogue row.
    let erasing = majority_collection::<Document>(db, "account_deletions")
        .find_one(doc! {
            "subject_hash":layer2_core::account_deletion::subject_hash(&parent.owner),
            "$or":[{"active_subject_hash":{"$exists":true}},
                {"status":{"$ne":"completed"}},{"status":{"$type":"array"}}],
        })
        .projection(doc! {"_id":1})
        .session(&mut *session)
        .await?
        .is_some();
    if erasing {
        return Ok(Decision::Pending);
    }
    // The record executor also runs independently of account-deletion workers.
    // Its installed catalogue fences prove erasure only for their own owner;
    // another publisher's fence does not make this root a deleting resource.
    for name in ["sandbox_profiles", "workspace_profiles", "agent_profiles"] {
        if majority_collection::<Document>(db, name)
            .find_one(doc! {"owner":parent.owner.as_str(),"account_erasure_fenced":true})
            .projection(doc! {"_id":1})
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Ok(Decision::Pending);
        }
    }
    Ok(Decision::Allowed)
}

pub(super) async fn workspace(
    db: &Database,
    candidate: &Workspace,
    parent: &Sandbox,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let WorkspaceOrigin::SandboxProfile { index } = candidate.origin else {
        return Ok(Decision::Refused);
    };
    if candidate.owner != parent.owner || candidate.sandbox_id != parent.id {
        return Ok(Decision::Refused);
    }
    let snapshot = &parent.profile;
    let Some(row) = exact(
        db,
        "sandbox_profile_revisions",
        snapshot.id.as_str(),
        snapshot.revision,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let Ok(revision) = sandbox_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if revision.digest != snapshot.digest || revision.content != snapshot.content {
        return Ok(Decision::Refused);
    }
    let Some(reference) = snapshot.content.workspace_profiles.get(usize::from(index)) else {
        return Ok(Decision::Refused);
    };
    if reference.id != candidate.profile.id || reference.revision != candidate.profile.revision {
        return Ok(Decision::Refused);
    }
    let Some(row) = exact(
        db,
        "workspace_profile_revisions",
        reference.id.as_str(),
        reference.revision,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let Ok(revision) = workspace_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if WorkspaceProfileSnapshot::from(&revision) != candidate.profile {
        return Ok(Decision::Refused);
    }
    let decision = sandbox_pin(
        db,
        snapshot.id.as_str(),
        snapshot.revision,
        Some(&snapshot.digest),
        session,
    )
    .await?;
    existing(db, parent, decision, session).await
}

pub(super) async fn agent(
    db: &Database,
    admission: &AgentAdmission,
    parent: &Sandbox,
    workspace: &Workspace,
    session: &mut ClientSession,
) -> mongodb::error::Result<Decision> {
    let AgentOrigin::WorkspaceProfile { index } = admission.origin else {
        return Ok(Decision::Refused);
    };
    if admission.owner != workspace.owner
        || workspace.owner != parent.owner
        || admission.workspace_id != workspace.id
        || workspace.sandbox_id != parent.id
        || admission.creation_key != format!("workspace-profile:{}:{index}", workspace.id)
        || admission.title.is_some()
        || admission.first_message.is_some()
    {
        return Ok(Decision::Refused);
    }
    let snapshot = &workspace.profile;
    let Some(row) = exact(
        db,
        "workspace_profile_revisions",
        snapshot.id.as_str(),
        snapshot.revision,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let Ok(revision) = workspace_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if WorkspaceProfileSnapshot::from(&revision) != *snapshot {
        return Ok(Decision::Refused);
    }
    let Some(reference) = snapshot.content.agent_profiles.get(usize::from(index)) else {
        return Ok(Decision::Refused);
    };
    if reference.id != admission.profile.id || reference.revision != admission.profile.revision {
        return Ok(Decision::Refused);
    }
    let Some(row) = exact(
        db,
        "agent_profile_revisions",
        reference.id.as_str(),
        reference.revision,
        session,
    )
    .await?
    else {
        return Ok(Decision::Refused);
    };
    let Ok(revision) = agent_profiles::read_revision(&row) else {
        return Ok(Decision::Refused);
    };
    if AgentProfileSnapshot::from(&revision) != admission.profile {
        return Ok(Decision::Refused);
    }
    let decision = workspace_pin(
        db,
        snapshot.id.as_str(),
        snapshot.revision,
        Some(&snapshot.digest),
        session,
    )
    .await?;
    existing(db, parent, decision, session).await
}
