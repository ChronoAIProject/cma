use super::*;
use layer2_core::sandbox::binding::BindingAdmissionFence;
use layer2_core::sandbox::store::{AgentAdmission, SandboxAdmission, WorkspaceAdmission};
use mongodb::ClientSession;

impl BindingCollections {
    async fn fence(
        &self,
        fence: &BindingAdmissionFence,
        session: &mut ClientSession,
    ) -> Result<ProviderBinding, SandboxStoreError> {
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
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        self.records
            .find_one(doc! {"_id": &fence.binding_id})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .map(read_binding)
            .transpose()?
            .ok_or(SandboxStoreError::Conflict)
    }

    pub(in crate::store) async fn admit_sandbox(
        &self,
        admission: &SandboxAdmission,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        if let Some(fence) = &admission.binding {
            let binding = self.fence(fence, session).await?;
            fence.require_sandbox(&binding, &admission.sandbox)?;
        }
        Ok(())
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
    ) -> Result<(), SandboxStoreError> {
        if let Some(fence) = &admission.binding {
            let binding = self.fence(fence, session).await?;
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
                .map_err(driver)?
                .map(read_conversation)
                .transpose()?
                .ok_or(SandboxStoreError::Conflict)?;
            fence.require_agent(&binding, &conversation, admission)?;
        }
        Ok(())
    }
}
