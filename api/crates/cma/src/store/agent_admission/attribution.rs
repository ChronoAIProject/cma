use super::*;
#[async_trait::async_trait]
impl AgentAdmissionStore for MongoSandboxStore {
    async fn admit_agent_with_trigger_initiator(
        &self,
        admission: AgentAdmission,
        initiation: layer2_core::trigger_domain::LaunchInitiation,
        reuse_existing: bool,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, reuse_existing, Some(initiation))
            .await
    }

    async fn admit_agent_reusing_profile(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, true, None).await
    }

    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, false, None).await
    }
}

/// Resolve the exact durable source again in the Agent creation transaction.
/// Caller-supplied provenance or another launch's sidecar cannot supply attribution.
pub(in crate::store) async fn fence_initiation(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    expected: &layer2_core::trigger_domain::LaunchInitiation,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    use crate::store::sandbox_mongo::{read_record, record_bson};
    use layer2_core::trigger_domain::{Launch, LaunchEffectKind, LaunchEffectState};
    use mongodb::bson::Document;
    expected
        .require_agent_admission(admission)
        .map_err(ApplyError::Domain)?;
    let launches = crate::store::durability::majority_collection::<Document>(
        &store.client.database(&store.agents.namespace().db),
        "trigger_launches",
    );
    let document = launches
        .find_one(doc! {"_id":expected.launch_id().as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let mut launch: Launch =
        read_record(&document).map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    launch.initiation = document
        .get("launch_initiation")
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()))
        .transpose()
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    launch.input_protocol = document
        .get("trigger_input_protocol")
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()))
        .transpose()
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    launch.input_binding_sha256 = document
        .get("trigger_input_binding_sha256")
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()))
        .transpose()
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    launch
        .validate()
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339());
    if launch.initiation.as_ref() != Some(expected)
        || launch.resolution.workspace.id() != Some(&admission.workspace_id)
        || launch
            .work
            .lease
            .as_ref()
            .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
            .zip(now)
            .is_none_or(|(expires, now)| expires <= now)
        || launch.work.effect.as_ref().is_none_or(|effect| {
            effect.kind != LaunchEffectKind::CreateAgent
                || !matches!(
                    effect.state,
                    LaunchEffectState::Executing | LaunchEffectState::RecoveryRequired
                )
        })
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let record = document
        .get("record")
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let changed = launches.update_one(doc! {
        "_id":expected.launch_id().as_str(), "record":record.clone(),
        "launch_initiation":record_bson(expected).map_err(|_|ApplyError::Domain(SandboxStoreError::Conflict))?,
    }, doc! {"$inc":{"agent_initiation_fence":1_i64}})
        .session(session).await.map_err(ApplyError::Driver)?;
    if changed.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}
