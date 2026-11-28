use super::super::agent_admission_records::LoadError;
use super::*;
use layer2_core::resource_access::{ResourceOwner, ResourcePolicyFence};
use layer2_core::sandbox::store::AgentAdmission;
use layer2_core::trigger_domain::Launch;

impl MongoSandboxStore {
    pub(crate) fn with_onboarding_command_admission(mut self, enabled: bool) -> Self {
        self.onboarding_command_admission = enabled;
        self
    }
}

/// Shares the Agent creation/reuse transaction, after its aggregate is written.
pub(in crate::store) async fn attach(
    store: &MongoSandboxStore,
    launch: Option<&Launch>,
    admission: &AgentAdmission,
    agent: &mut Agent,
    session: &mut ClientSession,
) -> Result<(), LoadError> {
    if agent.conversation.first_message.is_none() {
        return Ok(());
    }
    if agent
        .conversation
        .first_message
        .as_ref()
        .is_some_and(|first| first.common_binding.is_some())
    {
        return require_command_receipt(store, agent, session)
            .await
            .map_err(LoadError::Domain);
    }
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?
        .as_ref()
        .map(super::super::sandboxes::read_sandbox)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    let policy = store
        .resource_policies
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?
        .as_ref()
        .map(super::super::resource_access::read_inventory_policy)
        .transpose()
        .map_err(|error| LoadError::Domain(unavailable(error)))?;
    let owner = policy.as_ref().map_or_else(
        || ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
        |policy| policy.owner.clone(),
    );
    let fence = ResourcePolicyFence {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        policy_revision: policy.as_ref().map_or(0, |policy| policy.revision),
    };
    let (mut binding, request) = match launch {
        Some(launch) => {
            TriggerFirstMessageBinding::prepare(launch, admission, agent, &parent, fence, owner)
        }
        None => {
            TriggerFirstMessageBinding::prepare_onboarding(admission, agent, &parent, fence, owner)
        }
    }
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let accepted = Box::pin(super::super::agent_commands::reserve::apply_with_cause(
        store,
        &request,
        layer2_core::admission::AdmissionCause::Continuation,
        session,
    ))
    .await
    .map_err(|error| match error {
        super::super::agent_commands::ApplyError::Domain(error) => {
            LoadError::Domain(super::super::agent_commands::binding::domain_error(error))
        }
        super::super::agent_commands::ApplyError::Driver(error) => LoadError::Driver(error),
    })?;
    let command = match accepted {
        layer2_core::sandbox::agent_io::CommandReservationResult::Created(command) => command,
        layer2_core::sandbox::agent_io::CommandReservationResult::Existing(_) => {
            return Err(LoadError::Domain(SandboxStoreError::Conflict))
        }
    };
    binding
        .bind_receipt(&command)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    collection(store)
        .insert_one(document(&binding)?)
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?;
    let changed = store.agents.update_one(doc! {"_id":agent.id.as_str(), "revision":agent.revision as i64, FIELD:{"$exists":false}},
        doc! {"$set": {FIELD:record_bson(&binding).map_err(SandboxStoreError::Corrupt)?}})
        .session(&mut *session).await.map_err(LoadError::Driver)?;
    if changed.matched_count != 1 {
        return Err(LoadError::Domain(SandboxStoreError::Conflict));
    }
    if let Some(launch) = launch {
        let launches = super::super::durability::majority_collection::<Document>(
            &store.client.database(&store.agents.namespace().db),
            "trigger_launches",
        );
        let revision = super::super::sandbox_mongo::signed(launch.revision, "Launch revision")
            .map_err(SandboxStoreError::Corrupt)?;
        let next_revision = revision.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        let changed = launches.update_one(doc! {"_id":launch.id.as_str(), "revision":revision, "trigger_input_binding_sha256":{"$exists":false},
        "trigger_input_protocol":record_bson(&launch.input_protocol.as_ref().ok_or(SandboxStoreError::Conflict)?).map_err(SandboxStoreError::Corrupt)?},
        doc! {"$set":{"trigger_input_binding_sha256":binding.digest().map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
            "revision":next_revision,"record.revision":next_revision}})
        .session(&mut *session).await.map_err(LoadError::Driver)?;
        if changed.matched_count != 1 {
            return Err(LoadError::Domain(SandboxStoreError::Conflict));
        }
    }
    agent
        .conversation
        .first_message
        .as_mut()
        .ok_or(SandboxStoreError::Conflict)?
        .common_binding = Some(binding);
    Ok(())
}
