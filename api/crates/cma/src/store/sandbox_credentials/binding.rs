use super::*;
use layer2_core::CoreError;
use mongodb::ClientSession;

impl MongoSandboxCredentialStore {
    /// Read the existing generation inside the caller's parent-fenced
    /// transaction. Recovery must never create or replace missing custody.
    pub(crate) async fn current_binding_credential(
        &self,
        database: &str,
        sandbox: &layer2_core::sandbox::Sandbox,
        session: &mut ClientSession,
    ) -> Result<layer2_core::sandbox::binding::ProvidedCredential, CoreError> {
        if self.collection.namespace().db != database {
            return Err(corrupt());
        }
        let authority = sandbox.credential.current.as_ref().ok_or_else(corrupt)?;
        let document = self
            .collection
            .find_one(doc! {"_id": credential_id(&sandbox.id, authority.generation)})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let material = self.material(document).map_err(|_| corrupt())?;
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.source != sandbox.credential.source
            || material.authority != *authority
        {
            return Err(corrupt());
        }
        layer2_core::sandbox::binding::ProvidedCredential::new(material.value.expose().to_owned())
            .map_err(|_| corrupt())
    }

    pub(crate) async fn retain_binding_material(
        &self,
        database: &str,
        material: &CredentialMaterial,
        session: &mut ClientSession,
    ) -> Result<(), CoreError> {
        if self.collection.namespace().db != database {
            return Err(corrupt());
        }
        let document = self.document(material).map_err(|_| corrupt())?;
        if let Some(existing) = self
            .collection
            .find_one(doc! {"_id": &document.id})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
        {
            let existing = self.material(existing).map_err(|_| corrupt())?;
            if existing.sandbox_id != material.sandbox_id
                || existing.owner != material.owner
                || existing.source != material.source
                || existing.authority.grant_fingerprint != material.authority.grant_fingerprint
                || existing.value.expose() != material.value.expose()
            {
                return Err(corrupt());
            }
        } else {
            self.collection
                .insert_one(document)
                .session(&mut *session)
                .await
                .map_err(unavailable)?;
        }
        Ok(())
    }

    pub(crate) async fn forget_binding_material(
        &self,
        database: &str,
        id: &SandboxId,
        owner: &SubjectId,
        session: &mut ClientSession,
    ) -> Result<(), CoreError> {
        if self.collection.namespace().db != database {
            return Err(corrupt());
        }
        let mut cursor = self
            .collection
            .find(doc! {"sandbox_id": id.as_str()})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        while let Some(document) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let material = self.material(document).map_err(|_| corrupt())?;
            if material.owner != *owner
                || material.sandbox_id != *id
                || material.source.is_managed()
            {
                return Err(corrupt());
            }
        }
        self.collection
            .delete_many(doc! {"sandbox_id": id.as_str()})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}

fn corrupt() -> CoreError {
    CoreError::InvalidState("binding credential identity conflicts".into())
}
fn unavailable(_: mongodb::error::Error) -> CoreError {
    CoreError::RuntimeUnavailable("binding credential store is unavailable".into())
}
