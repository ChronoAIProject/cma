//! Atomic partition change plus permanent receipt and original source archive.
use super::*;
use layer2_core::bridge::{partition_commands::*, BridgeError, BridgeManagementLease};
use mongodb::Database;
mod journal;
mod reads;
mod sources;
mod write;
pub(in crate::store) use journal::{
    require_detached, require_detached_then_reset, retained_fixed_source,
};

enum Query<'a> {
    Recover {
        id: &'a str,
        digest: &'a str,
    },
    Snapshot(&'a str),
    Apply {
        write: &'a PartitionCommandWrite,
        reset: Option<&'a AllocationReservation>,
    },
}
enum Value {
    Receipt(Option<Box<PartitionCommandReceipt>>),
    Snapshot(Box<PartitionCommandSnapshot>),
}

impl MongoBridgeAllocationStore {
    fn command_database(&self) -> Database {
        self.bridges.client.database(&self.records.namespace().db)
    }
    async fn command_fence(
        &self,
        manager: &BridgeManagementLease,
        session: &mut ClientSession,
    ) -> Result<(), Error> {
        manager.require_current()?;
        let row = self
            .bridges
            .definitions
            .find_one(doc! {"_id":manager.record().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(BridgeError::NotFound)?;
        if crate::store::bridge_definitions::decode_bridge(&row)? != *manager.record() {
            return Err(PartitionCommandError::Conflict.into());
        }
        let changed = self.bridges.definitions.update_one(
            doc! {"_id":manager.record().id.as_str(),"revision":manager.record().revision as i64},
            doc! {"$inc":{"partition_command_fence":1_i64}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(PartitionCommandError::Conflict.into());
        }
        manager.require_current()?;
        Ok(())
    }
    async fn command_run(
        &self,
        manager: &BridgeManagementLease,
        query: Query<'_>,
    ) -> Result<Value, PartitionCommandError> {
        for _ in 0..8 {
            manager.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| PartitionCommandError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| PartitionCommandError::Unavailable)?;
            let result = async {
                self.command_fence(manager, &mut session).await?;
                let value = match &query {
                    Query::Recover { id, digest } => Value::Receipt(
                        self.command_recover_in(manager, id, digest, &mut session)
                            .await?
                            .map(Box::new),
                    ),
                    Query::Snapshot(id) => Value::Snapshot(Box::new(
                        self.command_snapshot_in(manager, id, &mut session).await?,
                    )),
                    Query::Apply { write, reset } => Value::Receipt(Some(Box::new(
                        self.command_apply_in(write, *reset, &mut session).await?,
                    ))),
                };
                manager.require_current()?;
                Ok::<_, Error>(value)
            }
            .await;
            let value = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if matches!(&error, Error::Mongo(e) if is_transient(e) || is_duplicate_key(e)) {
                        continue;
                    }
                    return Err(error.core());
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(value),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(PartitionCommandError::Unavailable),
            }
        }
        Err(PartitionCommandError::Unavailable)
    }
    pub(crate) async fn recover_partition_command(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
    ) -> Result<Option<PartitionCommandReceipt>, PartitionCommandError> {
        match self
            .command_run(manager, Query::Recover { id, digest })
            .await?
        {
            Value::Receipt(value) => Ok(value.map(|receipt| *receipt)),
            _ => Err(PartitionCommandError::Corrupt),
        }
    }
    pub(crate) async fn partition_command_snapshot(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<PartitionCommandSnapshot, PartitionCommandError> {
        match self.command_run(manager, Query::Snapshot(id)).await? {
            Value::Snapshot(value) => Ok(*value),
            _ => Err(PartitionCommandError::Corrupt),
        }
    }
    pub(crate) async fn apply_partition_command(
        &self,
        write: &PartitionCommandWrite,
        reset: Option<&AllocationReservation>,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError> {
        match self
            .command_run(write.management(), Query::Apply { write, reset })
            .await?
        {
            Value::Receipt(Some(value)) => Ok(*value),
            _ => Err(PartitionCommandError::Corrupt),
        }
    }
    pub(super) async fn require_detached_head(
        &self,
        allocation: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        require_detached(&self.command_database(), allocation, true, session).await
    }
}

enum Error {
    Core(PartitionCommandError),
    Mongo(mongodb::error::Error),
}
impl Error {
    fn core(self) -> PartitionCommandError {
        match self {
            Self::Core(e) => e,
            Self::Mongo(_) => PartitionCommandError::Unavailable,
        }
    }
}
impl From<PartitionCommandError> for Error {
    fn from(e: PartitionCommandError) -> Self {
        Self::Core(e)
    }
}
impl From<BridgeError> for Error {
    fn from(e: BridgeError) -> Self {
        Self::Core(e.into())
    }
}
impl From<AllocationError> for Error {
    fn from(e: AllocationError) -> Self {
        Self::Core(e.into())
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(e: mongodb::error::Error) -> Self {
        Self::Mongo(e)
    }
}
impl From<MutationError> for Error {
    fn from(e: MutationError) -> Self {
        match e {
            MutationError::Core(e) => e.into(),
            MutationError::Mongo(e) => e.into(),
        }
    }
}
impl From<crate::store::bridge_invokes::MutationError> for Error {
    fn from(e: crate::store::bridge_invokes::MutationError) -> Self {
        use layer2_core::bridge::invoke::BridgeInvokeError;
        match e {
            crate::store::bridge_invokes::MutationError::Mongo(e) => e.into(),
            crate::store::bridge_invokes::MutationError::Invoke(e) => Self::Core(match e {
                BridgeInvokeError::Corrupt => PartitionCommandError::Corrupt,
                BridgeInvokeError::Conflict => PartitionCommandError::Conflict,
                _ => PartitionCommandError::Unavailable,
            }),
        }
    }
}
