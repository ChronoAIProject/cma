use super::*;
use crate::store::{agent_commands::read_command, bridge_definitions::decode_bridge};

impl MongoBridgeAllocationStore {
    pub(super) async fn partition_transaction(
        &self,
        manager: &BridgeManagementLease,
        query: &Query<'_>,
        session: &mut ClientSession,
    ) -> Result<ResultValue, ReadError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let bridge = self
            .bridges
            .definitions
            .find_one(doc! {"_id":manager.record().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(PartitionStatusError::NotFound)?;
        if decode_bridge(&bridge)? != *manager.record() {
            return Err(PartitionStatusError::Conflict.into());
        }
        match query {
            Query::Usage => {
                // Existing Bridge-prefix index, no sort before the hard bound:
                // even retained generations beyond the view limit cannot cause
                // an unbounded resource walk or a full inventory sort.
                let mut cursor = self
                    .records
                    .find(doc! {"bridge_id":manager.record().id.as_str()})
                    .hint(mongodb::options::Hint::Keys(
                        doc! {"bridge_id":1,"creates_sandbox":1},
                    ))
                    .limit((MAX_USAGE_ALLOCATIONS + 1) as i64)
                    .max_time(Duration::from_secs(2))
                    .session(&mut *session)
                    .await?;
                let mut rows = Vec::new();
                while let Some(row) = cursor.next(&mut *session).await.transpose()? {
                    manager.require_current()?;
                    rows.push(row);
                }
                if rows.len() > MAX_USAGE_ALLOCATIONS {
                    return Ok(ResultValue::Usage(None));
                }
                let mut records = Vec::new();
                for row in rows {
                    records.push(
                        self.partition_evidence(manager, &database, &row, false, session)
                            .await?,
                    );
                }
                let mut heads = self
                    .heads
                    .find(doc! {"bridge_id":manager.record().id.as_str()})
                    .hint(mongodb::options::Hint::Keys(doc! {"bridge_id":1}))
                    .limit((MAX_USAGE_ALLOCATIONS + 1) as i64)
                    .max_time(Duration::from_secs(2))
                    .session(&mut *session)
                    .await?;
                let mut head_ids = std::collections::BTreeSet::new();
                while let Some(head) = heads.next(&mut *session).await.transpose()? {
                    manager.require_current()?;
                    let id = head
                        .get_str("allocation_id")
                        .map_err(|_| PartitionStatusError::Corrupt)?;
                    if !head_ids.insert(id.to_owned()) {
                        return Err(PartitionStatusError::Corrupt.into());
                    }
                    if head_ids.len() > MAX_USAGE_ALLOCATIONS {
                        return Ok(ResultValue::Usage(None));
                    }
                    let current = records
                        .iter()
                        .find(|record| record.current && record.allocation.id == id)
                        .ok_or(PartitionStatusError::Corrupt)?;
                    decode_head(&head, &current.allocation.identity)
                        .map_err(|_| PartitionStatusError::Corrupt)?;
                }
                if head_ids.len() != records.iter().filter(|record| record.current).count() {
                    return Err(PartitionStatusError::Corrupt.into());
                }
                records.sort_by(|a, b| a.allocation.id.cmp(&b.allocation.id));
                Ok(ResultValue::Usage(Some(records)))
            }
            Query::List { after, limit } => {
                let mut filter = doc! {"bridge_id":manager.record().id.as_str()};
                if let Some(after) = after {
                    filter.insert("_id", doc! {"$gt":*after});
                }
                let mut cursor = self
                    .records
                    .find(filter)
                    .sort(doc! {"_id":1})
                    .limit(i64::from(*limit) + 1)
                    .max_time(Duration::from_secs(2))
                    .session(&mut *session)
                    .await?;
                let mut result = Vec::new();
                while let Some(row) = cursor.next(&mut *session).await.transpose()? {
                    manager.require_current()?;
                    result.push(
                        self.partition_evidence(manager, &database, &row, true, session)
                            .await?,
                    );
                }
                Ok(ResultValue::Partitions(result))
            }
            Query::Detail(id) => {
                let row = self
                    .records
                    .find_one(doc! {"_id":*id,"bridge_id":manager.record().id.as_str()})
                    .session(&mut *session)
                    .await?;
                let mut result = Vec::new();
                if let Some(row) = row {
                    result.push(
                        self.partition_evidence(manager, &database, &row, true, session)
                            .await?,
                    );
                }
                Ok(ResultValue::Partitions(result))
            }
            Query::Activity(allocation) => {
                let row = self
                    .records
                    .find_one(doc! {"_id":&allocation.id,"bridge_id":manager.record().id.as_str()})
                    .session(&mut *session)
                    .await?
                    .ok_or(PartitionStatusError::Conflict)?;
                if managed(manager, &row)? != **allocation {
                    return Err(PartitionStatusError::Conflict.into());
                }
                Ok(ResultValue::Activity(Box::new(
                    activity(&database, allocation, session).await?,
                )))
            }
        }
    }

    async fn partition_evidence(
        &self,
        manager: &BridgeManagementLease,
        database: &Database,
        row: &Document,
        include_source: bool,
        session: &mut ClientSession,
    ) -> Result<PartitionEvidence, ReadError> {
        let allocation = managed(manager, row)?;
        let head = self.heads.find_one(doc! {"_id":allocation.identity.head_id().map_err(|_| PartitionStatusError::Corrupt)?}).session(&mut *session).await?.ok_or(PartitionStatusError::Corrupt)?;
        let current = decode_head(&head, &allocation.identity)
            .map_err(|_| PartitionStatusError::Corrupt)?
            == allocation.id;
        let ids = allocation.placement.target();
        let mut present = Vec::new();
        for (kind, collection, id) in [
            (
                PartitionResource::Sandbox,
                "sandboxes",
                ids.sandbox_id.as_str(),
            ),
            (
                PartitionResource::Workspace,
                "workspaces",
                ids.workspace_id.as_str(),
            ),
            (
                PartitionResource::Worktree,
                "worktrees",
                ids.worktree_id.as_str(),
            ),
            (PartitionResource::Agent, "agents", ids.agent_id.as_str()),
        ] {
            if database
                .collection::<Document>(collection)
                .find_one(doc! {"_id":id})
                .projection(doc! {"_id":1})
                .session(&mut *session)
                .await?
                .is_some()
            {
                present.push(kind);
            }
        }
        let first_source = if include_source {
            sources::first(database, &allocation, session).await?
        } else {
            None
        };
        Ok(PartitionEvidence {
            allocation,
            current,
            present,
            first_source,
            observed_at_ms: mongodb::bson::DateTime::now().timestamp_millis(),
        })
    }
}

fn managed(manager: &BridgeManagementLease, row: &Document) -> Result<AllocationRecord, ReadError> {
    manager.require_current()?;
    let allocation = decode_record(row).map_err(|_| PartitionStatusError::Corrupt)?;
    if allocation.owner != manager.record().owner
        || allocation.identity.bridge_id != manager.record().id
        || manager.record().pinned(&allocation.pin)?.config != allocation.config
    {
        return Err(PartitionStatusError::Corrupt.into());
    }
    Ok(allocation)
}

async fn activity(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<PartitionActivityEvidence, ReadError> {
    let commands = database.collection::<Document>("agent_commands");
    let exact = doc! {"channel_admission.allocation_id":&allocation.id,"agent_id":allocation.placement.target().agent_id.as_str()};
    let mut filter = exact.clone();
    filter.insert(
        "status",
        doc! {"$in":["queued","in_progress","waiting_action"]},
    );
    let mut cursor = commands
        .find(filter)
        .sort(doc! {"agent_order":1})
        .limit((MAX_PARTITION_PENDING + 1) as i64)
        .max_time(Duration::from_secs(2))
        .session(&mut *session)
        .await?;
    let mut pending = Vec::new();
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        pending.push(command(database, allocation, &row, session).await?);
    }
    let latest = commands
        .find_one(exact)
        .sort(doc! {"agent_order":-1})
        .max_time(Duration::from_secs(2))
        .session(&mut *session)
        .await?;
    let latest = match latest {
        Some(row) => Some(command(database, allocation, &row, session).await?),
        None => None,
    };
    Ok(PartitionActivityEvidence {
        pending,
        latest,
        observed_at_ms: mongodb::bson::DateTime::now().timestamp_millis(),
    })
}

async fn command(
    database: &Database,
    allocation: &AllocationRecord,
    row: &Document,
    session: &mut ClientSession,
) -> Result<PartitionCommandEvidence, ReadError> {
    let command = read_command(row).map_err(|_| PartitionStatusError::Corrupt)?;
    let sidecar = sources::sidecar(row, allocation)?;
    let source = sources::command_source(database, row, &sidecar, session).await?;
    Ok(PartitionCommandEvidence {
        command,
        sidecar,
        source,
    })
}
