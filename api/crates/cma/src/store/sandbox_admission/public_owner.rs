use std::num::NonZeroU16;

use layer2_core::SubjectId;
use mongodb::ClientSession;

use crate::store::{registration_capacity, sandbox_mongo::MongoSandboxStore};

impl MongoSandboxStore {
    pub(crate) fn with_public_owner_sandbox_limit(mut self, limit: Option<NonZeroU16>) -> Self {
        self.public_owner_sandbox_limit = limit;
        self
    }

    pub(in crate::store) async fn public_owner_sandbox_limit(
        &self,
        owner: &SubjectId,
        session: Option<&mut ClientSession>,
    ) -> Result<Option<u16>, mongodb::error::Error> {
        let Some(limit) = self.public_owner_sandbox_limit else {
            return Ok(None);
        };
        // The registration receipt exists before a new public owner can create
        // resources. Existing consumers retain the operator's general allowance.
        let public =
            registration_capacity::is_public(&self.sandbox_admissions, owner, session).await?;
        Ok(public.then_some(limit.get()))
    }
}
