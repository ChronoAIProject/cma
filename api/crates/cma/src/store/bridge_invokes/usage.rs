//! Actual allocation bindings and grant links compose with the same transaction
//! as grant changes and accepted common commands. History grants no authority.
use layer2_core::{
    bridge::allocation::{AllocationRecord, AllocationState},
    sandbox::{channel_usage::*, store::SandboxStoreError, AgentCreationProvenance},
};

use crate::store::agent_channel_usage::{
    self,
    bridge_append::{append_fenced_change, FencedUsageActor},
    bridge_detach::append_management_detach,
    UsageError,
};

use super::*;

mod admission;
mod attach;
mod model;
mod reset;
mod revoke;
use model::{BindingHead, GrantLink};

impl From<UsageError> for MutationError {
    fn from(error: UsageError) -> Self {
        match error {
            UsageError::Driver(error) => Self::Mongo(error),
            UsageError::Domain(SandboxStoreError::Conflict) => {
                Self::Invoke(BridgeInvokeError::Conflict)
            }
            UsageError::Domain(SandboxStoreError::NotFound) => {
                Self::Invoke(BridgeInvokeError::NotFound)
            }
            UsageError::Domain(_) => Self::Invoke(BridgeInvokeError::Corrupt),
        }
    }
}

fn digest(value: &impl serde::Serialize) -> Result<String, MutationError> {
    layer2_core::profiles::content_digest(value).map_err(|_| BridgeInvokeError::Corrupt.into())
}

impl MongoBridgeInvokeStore {
    pub(super) async fn apply_grant_usage(
        &self,
        write: &BridgeInvokeWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        if write.previous().is_some() {
            self.revoke_grant_usage(write, session).await
        } else {
            self.attach_grant_usage(write, session).await
        }
    }

    fn usage_allocations(&self) -> Collection<Document> {
        majority_collection(
            &self.bridges.client.database(&self.grants.namespace().db),
            "bridge_allocations",
        )
    }

    async fn usage_allocation(
        &self,
        id: &str,
        session: &mut ClientSession,
    ) -> Result<(AllocationRecord, Option<BindingHead>), MutationError> {
        let document = self
            .usage_allocations()
            .find_one(doc! {"_id":id})
            .session(session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let allocation = crate::store::bridge_allocations::decode_record(&document)
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        let head = BindingHead::read(&document)?;
        if let Some(head) = &head {
            head.require_allocation(&allocation)?;
        }
        Ok((allocation, head))
    }

    async fn write_usage_head(
        &self,
        allocation: &AllocationRecord,
        previous: Option<&BindingHead>,
        next: &BindingHead,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        next.require_allocation(allocation)?;
        let expected_revision = previous.map_or(1, |head| head.revision + 1);
        if next.revision != expected_revision {
            return Err(BridgeInvokeError::Corrupt.into());
        }
        let expected = match previous {
            Some(head) => record_bson(head).map_err(|_| BridgeInvokeError::Corrupt)?,
            None => mongodb::bson::Bson::Document(doc! {"$exists":false}),
        };
        let changed = self
            .usage_allocations()
            .update_one(
                doc! {"_id":&allocation.id,"revision":allocation.revision as i64,"channel_usage_binding":expected},
                doc! {"$set":{"channel_usage_binding":record_bson(next).map_err(|_|BridgeInvokeError::Corrupt)?}},
            )
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        Ok(())
    }
}
