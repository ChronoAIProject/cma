//! Complete encrypted custody identity is checked inside each retirement fence.
use super::channel::ChannelCredentialError;
use super::*;
use layer2_core::sandbox::{Sandbox, SandboxCredentialAuthority};
use mongodb::ClientSession;

impl MongoSandboxCredentialStore {
    pub(crate) async fn require_retirement_custody(
        &self,
        database: &str,
        sandbox: &Sandbox,
        expected: &[SandboxCredentialAuthority],
        session: &mut ClientSession,
    ) -> Result<(), ChannelCredentialError> {
        if self.collection.namespace().db != database || self.revocations.namespace().db != database
        {
            return Err(ChannelCredentialError::Corrupt);
        }
        let mut cursor = self
            .collection
            .find(doc! {"sandbox_id":sandbox.id.as_str()})
            .limit(5)
            .session(&mut *session)
            .await?;
        let mut observed = Vec::new();
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            let value = self
                .material(row)
                .map_err(|_| ChannelCredentialError::Corrupt)?;
            if value.sandbox_id != sandbox.id || value.owner != sandbox.owner {
                return Err(ChannelCredentialError::Corrupt);
            }
            observed.push(value.authority);
        }
        observed.sort_by_key(|key| key.generation);
        if observed != expected
            || self
                .revocations
                .find_one(doc! {"sandbox_id":sandbox.id.as_str()})
                .session(&mut *session)
                .await?
                .is_some()
        {
            return Err(ChannelCredentialError::Corrupt);
        }
        Ok(())
    }
}
