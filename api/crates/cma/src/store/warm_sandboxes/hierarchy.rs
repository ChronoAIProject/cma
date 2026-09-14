//! Read pinned child profiles inside the same transaction as the warm pool plan.

use layer2_core::profiles::{AgentProfileSnapshot, WorkspaceProfileRef, WorkspaceProfileSnapshot};
use layer2_core::sandbox::{SandboxStoreError, WarmWorkspacePlan};
use mongodb::{bson::doc, ClientSession};

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(super) async fn resolve(
    store: &MongoSandboxStore,
    references: &[WorkspaceProfileRef],
    session: &mut ClientSession,
) -> Result<Vec<WarmWorkspacePlan>, SandboxStoreError> {
    let mut result = Vec::with_capacity(references.len());
    for reference in references {
        let document = store
            .warm_workspace_revisions
            .find_one(doc! {
                "profile_id": reference.id.as_str(), "revision": i64::from(reference.revision),
            })
            .session(&mut *session)
            .await
            .map_err(super::unavailable)?
            .ok_or_else(|| invalid("warm workspace profile revision missing"))?;
        let revision = crate::store::workspace_profiles::read_revision(&document)
            .map_err(|error| invalid(&error.to_string()))?;
        let mut agents = Vec::with_capacity(revision.content.agent_profiles.len());
        for reference in &revision.content.agent_profiles {
            let document = store
                .warm_agent_revisions
                .find_one(doc! {
                    "profile_id": reference.id.as_str(), "revision": i64::from(reference.revision),
                })
                .session(&mut *session)
                .await
                .map_err(super::unavailable)?
                .ok_or_else(|| invalid("warm Agent profile revision missing"))?;
            let revision = crate::store::agent_profiles::read_revision(&document)
                .map_err(|error| invalid(&error.to_string()))?;
            agents.push(AgentProfileSnapshot::from(&revision));
        }
        result.push(WarmWorkspacePlan {
            profile: WorkspaceProfileSnapshot::from(&revision),
            agents,
        });
    }
    Ok(result)
}

fn invalid(detail: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(detail.into())
}
