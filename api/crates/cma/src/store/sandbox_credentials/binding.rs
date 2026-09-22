use super::*;
use layer2_core::CoreError;
use mongodb::ClientSession;

impl MongoSandboxCredentialStore {
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
