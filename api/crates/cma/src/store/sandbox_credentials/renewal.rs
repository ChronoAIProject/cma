//! Delete only the encrypted generation whose exact prior-key revocation was
//! observed inside a fenced Channel renewal operation.
use super::{channel::ChannelCredentialError, credential_id, MongoSandboxCredentialStore};
use layer2_core::{sandbox::SandboxCredentialAuthority, SandboxId, SubjectId};
use mongodb::{bson::doc, ClientSession};

impl MongoSandboxCredentialStore {
    pub(crate) async fn delete_for_channel(
        &self,
        database: &str,
        sandbox_id: &SandboxId,
        owner: &SubjectId,
        authority: &SandboxCredentialAuthority,
        session: &mut ClientSession,
    ) -> Result<(), ChannelCredentialError> {
        if self.collection.namespace().db != database {
            return Err(ChannelCredentialError::Corrupt);
        }
        let id = credential_id(sandbox_id, authority.generation);
        let document = self
            .collection
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await?
            .ok_or(ChannelCredentialError::Corrupt)?;
        let current = self
            .material(document)
            .map_err(|_| ChannelCredentialError::Corrupt)?;
        if &current.sandbox_id != sandbox_id
            || &current.owner != owner
            || &current.authority != authority
        {
            return Err(ChannelCredentialError::Corrupt);
        }
        let deleted = self.collection.delete_one(doc! {"_id":id,"sandbox_id":sandbox_id.as_str(),
            "owner":owner.as_str(),"provider_key_id":&authority.provider_key_id,"provider_state_version":authority.provider_state_version,
            "grant_fingerprint":&authority.grant_fingerprint}).session(&mut *session).await?;
        if deleted.deleted_count != 1 {
            return Err(ChannelCredentialError::Corrupt);
        }
        Ok(())
    }
}
