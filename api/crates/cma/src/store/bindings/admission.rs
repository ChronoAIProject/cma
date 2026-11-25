use super::*;
use crate::store::agent_admission::helper_error::HelperError;
use layer2_core::sandbox::binding::BindingAdmissionFence;
use layer2_core::sandbox::store::{AgentAdmission, SandboxAdmission, WorkspaceAdmission};
use mongodb::ClientSession;

impl BindingCollections {
    async fn fence(
        &self,
        fence: &BindingAdmissionFence,
        session: &mut ClientSession,
    ) -> Result<ProviderBinding, SandboxStoreError> {
        self.fence_transaction(fence, session)
            .await
            .map_err(|error| error.into_store_error(driver))
    }

    async fn fence_transaction(
        &self,
        fence: &BindingAdmissionFence,
        session: &mut ClientSession,
    ) -> Result<ProviderBinding, HelperError> {
        // Every conversation mutation also writes this document, so this fence
        // serializes creation with both parent and conversation retirement.
        let result = self
            .records
            .update_one(
                doc! {
                    "_id": &fence.binding_id, "revision": integer(fence.revision)?,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                },
                doc! {"$inc": {"admission_fence": 1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(HelperError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
        self.records
            .find_one(doc! {"_id": &fence.binding_id})
            .session(&mut *session)
            .await
            .map_err(HelperError::Driver)?
            .map(read_binding)
            .transpose()?
            .ok_or_else(|| SandboxStoreError::Conflict.into())
    }

    pub(in crate::store) async fn admit_sandbox(
        &self,
        admission: &SandboxAdmission,
        session: &mut ClientSession,
    ) -> Result<Option<layer2_core::resource_access::ResourcePolicyRecord>, SandboxStoreError> {
        if let Some(fence) = &admission.binding {
            let binding = self.fence(fence, session).await?;
            fence.require_sandbox(&binding, &admission.sandbox)?;
            return binding.sandbox_resource_policy(&admission.sandbox);
        }
        Ok(None)
    }

    pub(in crate::store) async fn admit_workspace(
        &self,
        admission: &WorkspaceAdmission,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        if let Some(fence) = &admission.binding {
            let binding = self.fence(fence, session).await?;
            fence.require_workspace(&binding, &admission.workspace)?;
        }
        Ok(())
    }

    pub(in crate::store) async fn admit_agent(
        &self,
        admission: &AgentAdmission,
        session: &mut ClientSession,
    ) -> Result<(), HelperError> {
        if let Some(fence) = &admission.binding {
            let binding = self.fence_transaction(fence, session).await?;
            let proof = fence
                .conversation
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?;
            let key = format!(
                "{}:{:020}:{}",
                fence.binding_id, fence.generation, proof.conversation_id
            );
            let conversation = self
                .conversations
                .find_one(doc! {"_id": key})
                .session(&mut *session)
                .await
                .map_err(HelperError::Driver)?
                .map(read_conversation)
                .transpose()?
                .ok_or(SandboxStoreError::Conflict)?;
            fence.require_agent(&binding, &conversation, admission)?;
        }
        Ok(())
    }
}

impl BindingCollections {
    pub(in crate::store) async fn keep_alive(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
        session: &mut ClientSession,
    ) -> Result<Option<bool>, SandboxStoreError> {
        let layer2_core::sandbox::SandboxCredentialSource::Binding { binding_id, .. } =
            &sandbox.credential.source
        else {
            return Ok(None);
        };
        let binding = self
            .records
            .find_one(doc! {"_id": binding_id})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .map(read_binding)
            .transpose()?
            .ok_or(SandboxStoreError::NotFound)?;
        if binding.sandbox_id != sandbox.id || binding.owner != sandbox.owner {
            return Err(SandboxStoreError::Conflict);
        }
        if binding
            .profile
            .environment
            .as_ref()
            .is_some_and(|environment| environment.always_keep_alive)
        {
            return Ok(Some(true));
        }
        let fence = BindingAdmissionFence::environment(&binding);
        self.fence(&fence, session).await?;
        Ok(Some(false))
    }
}
