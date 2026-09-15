use super::*;

#[async_trait::async_trait]
impl SandboxCredentialCustody for MongoSandboxCredentialStore {
    async fn insert(&self, material: CredentialMaterial) -> Result<(), CredentialCustodyError> {
        let document = self.document(&material).map_err(custody_store_error)?;
        self.collection
            .insert_one(document)
            .await
            .map(|_| ())
            .map_err(|error| {
                if crate::store::sandbox_mongo::is_duplicate_key(&error) {
                    CredentialCustodyError::Duplicate
                } else {
                    CredentialCustodyError::Unavailable(error.to_string())
                }
            })
    }

    async fn get(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialCustodyError> {
        self.collection
            .find_one(doc! { "_id": credential_id(sandbox_id, generation) })
            .await
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))?
            .map(|document| {
                self.material(document)
                    .map_err(|error| CredentialCustodyError::Corrupt(error.to_string()))
            })
            .transpose()
    }

    async fn list(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialCustodyError> {
        use futures_util::TryStreamExt as _;

        let mut cursor = self
            .collection
            .find(doc! { "sandbox_id": sandbox_id.as_str() })
            .sort(doc! { "generation": 1 })
            .await
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))?;
        let mut rows = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))?
        {
            rows.push(
                self.material(document)
                    .map_err(|error| CredentialCustodyError::Corrupt(error.to_string()))?,
            );
        }
        Ok(rows)
    }

    async fn delete(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<(), CredentialCustodyError> {
        self.collection
            .delete_one(doc! { "_id": credential_id(sandbox_id, generation) })
            .await
            .map(|_| ())
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))
    }

    async fn queue_revocation(
        &self,
        revocation: CredentialRevocation,
    ) -> Result<(), CredentialCustodyError> {
        validate_revocation(&revocation).map_err(custody_store_error)?;
        let document = RevocationDocument {
            id: revocation_id(&revocation.sandbox_id, &revocation.provider_key_id),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            sandbox_id: revocation.sandbox_id.0,
            owner: revocation.owner.0,
            provider_key_id: revocation.provider_key_id,
        };
        match self.revocations.insert_one(document.clone()).await {
            Ok(_) => Ok(()),
            Err(error) if crate::store::sandbox_mongo::is_duplicate_key(&error) => {
                let existing = self
                    .revocations
                    .find_one(doc! { "_id": &document.id })
                    .await
                    .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))?;
                match existing {
                    Some(existing) if existing == document => Ok(()),
                    Some(_) => Err(CredentialCustodyError::Corrupt(
                        "credential revocation identity was reused".into(),
                    )),
                    None => Err(CredentialCustodyError::Unavailable(
                        "credential revocation raced but no durable debt remained".into(),
                    )),
                }
            }
            Err(error) => Err(CredentialCustodyError::Unavailable(error.to_string())),
        }
    }

    async fn revocations(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialRevocation>, CredentialCustodyError> {
        use futures_util::TryStreamExt as _;

        let mut cursor = self
            .revocations
            .find(doc! { "sandbox_id": sandbox_id.as_str() })
            .sort(doc! { "_id": 1 })
            .await
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))?;
        let mut rows = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))?
        {
            rows.push(
                self.revocation(document)
                    .map_err(|error| CredentialCustodyError::Corrupt(error.to_string()))?,
            );
        }
        Ok(rows)
    }

    async fn delete_revocation(
        &self,
        sandbox_id: &SandboxId,
        provider_key_id: &str,
    ) -> Result<(), CredentialCustodyError> {
        self.revocations
            .delete_one(doc! { "_id": revocation_id(sandbox_id, provider_key_id) })
            .await
            .map(|_| ())
            .map_err(|error| CredentialCustodyError::Unavailable(error.to_string()))
    }
}
