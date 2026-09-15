//! Narrow custody insertion inside the exact Channel allocation transaction.
use super::MongoSandboxCredentialStore;
use crate::sandbox::CredentialMaterial;
use mongodb::ClientSession;

impl MongoSandboxCredentialStore {
    pub(crate) async fn insert_for_channel(
        &self,
        database: &str,
        material: &CredentialMaterial,
        session: &mut ClientSession,
    ) -> Result<(), ChannelCredentialError> {
        if self.collection.namespace().db != database {
            return Err(ChannelCredentialError::Corrupt);
        }
        let document = self
            .document(material)
            .map_err(|_| ChannelCredentialError::Corrupt)?;
        self.collection
            .insert_one(document)
            .session(session)
            .await?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ChannelCredentialError {
    #[error("Channel credential custody is inconsistent")]
    Corrupt,
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}
