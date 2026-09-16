//! Snapshot reads only: no source archival, resource mutation or runtime calls.
use super::*;
use layer2_core::bridge::{status::*, BridgeManagementLease};
use mongodb::Database;
use std::time::Duration;
mod reads;
mod sources;

enum Query<'a> {
    List { after: Option<&'a str>, limit: u32 },
    Detail(&'a str),
    Activity(&'a AllocationRecord),
}
enum ResultValue {
    Partitions(Vec<PartitionEvidence>),
    Activity(Box<PartitionActivityEvidence>),
}

impl MongoBridgeAllocationStore {
    pub(super) async fn initialize_partition_status(&self) -> Result<(), layer2_core::StoreError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let commands = majority_collection::<Document>(&database, "agent_commands");
        for (name, keys) in [
            (
                "channel_partition_pending",
                doc! {"channel_admission.allocation_id":1,"agent_id":1,"status":1,"agent_order":1},
            ),
            (
                "channel_partition_latest",
                doc! {"channel_admission.allocation_id":1,"agent_id":1,"agent_order":1},
            ),
            (
                "channel_partition_source",
                doc! {"channel_admission.allocation_id":1,"agent_id":1,"channel_admission.inbox_id":1},
            ),
        ] {
            commands
                .create_index(
                    IndexModel::builder()
                        .keys(keys)
                        .options(
                            mongodb::options::IndexOptions::builder()
                                .name(name.to_owned())
                                .build(),
                        )
                        .build(),
                )
                .await
                .map_err(|_| {
                    layer2_core::StoreError::Unavailable(
                        "create partition status lookup indexes".into(),
                    )
                })?;
        }
        Ok(())
    }

    async fn partition_read(
        &self,
        manager: &BridgeManagementLease,
        query: Query<'_>,
    ) -> Result<ResultValue, PartitionStatusError> {
        for _ in 0..8 {
            manager.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| PartitionStatusError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| PartitionStatusError::Unavailable)?;
            let result = match self
                .partition_transaction(manager, &query, &mut session)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if matches!(&error, ReadError::Mongo(error) if is_transient(error)) {
                        continue;
                    }
                    return Err(error.core());
                }
            };
            if let Err(error) = manager.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error.into());
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    manager.require_current()?;
                    return Ok(result);
                }
                CommitOutcome::Retry(_) => continue,
                _ => return Err(PartitionStatusError::Unavailable),
            }
        }
        Err(PartitionStatusError::Unavailable)
    }
}

#[async_trait::async_trait]
impl BridgePartitionStore for MongoBridgeAllocationStore {
    async fn list(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PartitionEvidence>, PartitionStatusError> {
        if !(1..=MAX_PARTITION_PAGE).contains(&limit) {
            return Err(PartitionStatusError::Invalid);
        }
        match self
            .partition_read(manager, Query::List { after, limit })
            .await?
        {
            ResultValue::Partitions(records) => Ok(records),
            _ => Err(PartitionStatusError::Corrupt),
        }
    }
    async fn detail(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<PartitionEvidence>, PartitionStatusError> {
        match self.partition_read(manager, Query::Detail(id)).await? {
            ResultValue::Partitions(mut records) => Ok(records.pop()),
            _ => Err(PartitionStatusError::Corrupt),
        }
    }
    async fn activity(
        &self,
        manager: &BridgeManagementLease,
        allocation: &AllocationRecord,
    ) -> Result<PartitionActivityEvidence, PartitionStatusError> {
        match self
            .partition_read(manager, Query::Activity(allocation))
            .await?
        {
            ResultValue::Activity(records) => Ok(*records),
            _ => Err(PartitionStatusError::Corrupt),
        }
    }
}

enum ReadError {
    Core(PartitionStatusError),
    Mongo(mongodb::error::Error),
}
impl From<PartitionStatusError> for ReadError {
    fn from(error: PartitionStatusError) -> Self {
        Self::Core(error)
    }
}
impl From<mongodb::error::Error> for ReadError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Mongo(error)
    }
}
impl From<layer2_core::bridge::BridgeError> for ReadError {
    fn from(error: layer2_core::bridge::BridgeError) -> Self {
        Self::Core(error.into())
    }
}
impl ReadError {
    fn core(self) -> PartitionStatusError {
        match self {
            Self::Core(error) => error,
            Self::Mongo(_) => PartitionStatusError::Unavailable,
        }
    }
}
