//! Public windows share one snapshot with their integrity proof.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::agent_log::read_entry;
use super::agent_log_integrity::{scan_collection, TransactionLogError};
use super::agent_log_telemetry::ScanCaller;
use super::sandbox_mongo::{signed, MongoSandboxStore};

pub(super) mod integrity;
mod segmented;
use integrity::{LogReadIntegrity, PreparedIntegrity, ReadHead};

#[derive(Clone, Copy)]
pub(super) enum Window {
    Page {
        after: u64,
        limit: u16,
    },
    Range {
        after: u64,
        through: u64,
        limit: u16,
    },
    Prefix {
        through: u64,
    },
}

impl Window {
    fn validate(self) -> Result<(), SandboxStoreError> {
        match self {
            Self::Page { limit, .. } | Self::Range { limit, .. }
                if limit == 0 || limit > MAX_PAGE_SIZE =>
            {
                Err(SandboxStoreError::InvalidPage)
            }
            Self::Range { after, through, .. } if after > through => {
                Err(SandboxStoreError::InvalidPage)
            }
            _ => Ok(()),
        }
    }

    fn contains(self, sequence: u64) -> bool {
        match self {
            Self::Page { after, .. } => sequence > after,
            Self::Range { after, through, .. } => sequence > after && sequence <= through,
            Self::Prefix { through } => sequence <= through,
        }
    }

    fn limit(self) -> u64 {
        match self {
            Self::Page { limit, .. } | Self::Range { limit, .. } => u64::from(limit),
            Self::Prefix { through } => through,
        }
    }

    fn bounds(self, head: u64) -> Result<(u64, u64), SandboxStoreError> {
        let (after, through) = match self {
            Self::Page { after, limit } => {
                let after = after.min(head);
                (after, after.saturating_add(u64::from(limit)).min(head))
            }
            Self::Range {
                after,
                through,
                limit,
            } => (after, after + (through - after).min(u64::from(limit))),
            Self::Prefix { through } => (0, through),
        };
        // An empty range may be beyond the head, but still needs its proof.
        if after != through && through > head {
            return Err(SandboxStoreError::SequenceConflict);
        }
        Ok((after, through))
    }
}

pub(super) async fn read_window(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    window: Window,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    read_with_integrity(store, agent_id, window, &PreparedIntegrity).await
}

pub(super) async fn read_with_integrity<P: LogReadIntegrity>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    window: Window,
    integrity: &P,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    window.validate()?;
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let Some(head) = integrity.read_head(store, agent_id, &mut session).await? else {
        window.bounds(0)?;
        return Ok(Vec::new());
    };
    let entries = if let Some(segmented) = &head.segmented {
        let (after, through) = window.bounds(head.last_seq)?;
        segmented::read_compatibility_window(store, &head, segmented, after, through, &mut session)
            .await?
    } else if head.trusted {
        let (after, through) = window.bounds(head.last_seq)?;
        let entries = read_bounded(
            store,
            agent_id,
            after,
            through,
            &head,
            &mut session,
            usize::MAX,
        )
        .await?
        .ok_or(SandboxStoreError::AgentLogCapacityExceeded)?;
        if !matches!(window, Window::Prefix { .. }) {
            super::agent_log_integrity::fleet::telemetry::record(
                super::agent_log_integrity::fleet::telemetry::Event::WindowScanAvoided,
            );
        }
        entries
    } else {
        if store.bounded_log_enabled(agent_id) {
            super::agent_log_integrity::fleet::telemetry::record(
                super::agent_log_integrity::fleet::telemetry::Event::Fallback,
            );
        }
        let mut entries = Vec::new();
        scan_collection(
            ScanCaller::PublicWindowPrefix,
            &store.agent_log,
            agent_id,
            head.last_seq,
            &mut session,
            crate::store::agent_log_integrity::ScanOptions::new(true, store.scan_cooperation),
            |entry| {
                head.validate_lineage(&entry)?;
                if window.contains(entry.seq) && (entries.len() as u64) < window.limit() {
                    entries.push(entry);
                }
                Ok(())
            },
        )
        .await
        .map_err(transaction_error)?;
        entries
    };
    let (after, through) = window.bounds(head.last_seq)?;
    if entries.len() as u64 != through - after {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(entries)
}

pub(super) async fn read_bounded(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    after: u64,
    through: u64,
    head: &ReadHead,
    session: &mut ClientSession,
    byte_budget: usize,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
    let count = through - after;
    if count == 0 {
        return Ok(Some(Vec::new()));
    }
    let mut read = super::mongo_read_telemetry::Read::find(store.agent_log.name());
    let mut cursor = store
        .agent_log
        .find(doc! {
            "agent_id": agent_id.as_str(),
            "seq": {
                "$gt": signed(after, "window start").map_err(SandboxStoreError::Corrupt)?,
                "$lte": signed(through, "window end").map_err(SandboxStoreError::Corrupt)?,
            },
        })
        .sort(doc! { "seq": 1 })
        .limit(signed(count, "window count").map_err(SandboxStoreError::Corrupt)?)
        .batch_size(u32::try_from(count).map_err(|_| SandboxStoreError::SequenceConflict)?)
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut entries = Vec::new();
    let mut bytes = 0_usize;
    while let Some(document) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(unavailable)?
    {
        let entry = read_entry(&document)?;
        head.validate_lineage(&entry)?;
        if entry.agent_id != *agent_id || entry.seq != after + entries.len() as u64 + 1 {
            return Err(SandboxStoreError::SequenceConflict);
        }
        if byte_budget != usize::MAX {
            bytes = bytes.saturating_add(cursor.current().as_bytes().len());
            if bytes > byte_budget {
                read.complete();
                return Ok(None);
            }
        }
        entries.push(entry);
    }
    if entries.len() as u64 != count {
        return Err(SandboxStoreError::SequenceConflict);
    }
    read.complete();
    Ok(Some(entries))
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

fn transaction_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => unavailable(error),
    }
}

#[cfg(test)]
mod tests;
