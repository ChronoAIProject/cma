//! Streaming adapters for the current AgentLogStore implementation.
use crate::store::{
    agent_log::read_entry,
    agent_log_integrity::{scan_collection, TransactionLogError},
    agent_log_reads::integrity::{LogReadIntegrity, PreparedIntegrity},
    agent_log_segments::{self as segments, LogRange},
    agent_log_telemetry::ScanCaller,
    sandbox_mongo::MongoSandboxStore,
};
use layer2_core::{
    sandbox::{store::SandboxStoreError, AgentLogEntry},
    AgentId,
};
use mongodb::{
    bson::{doc, Document, RawDocumentBuf},
    options::ReadConcern,
};

type Result<T> = std::result::Result<T, SandboxStoreError>;
type Visitor<'a> = &'a mut (dyn FnMut(AgentLogEntry) -> Result<()> + Send);

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
fn domain(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(e) => e,
        TransactionLogError::Driver(e) => unavailable(e),
    }
}

/// The owning writer module delegates its visitor ports here. No visitor is
/// called for rows outside the requested interval except its explicit predecessor.
/// Tail absence belongs to this same snapshot: prepared legacy proof rejects
/// rows beyond its head, fallback scans validate the complete prefix, and the
/// segmented range reader probes past its captured head before visiting rows.
/// The buffered-page port relies on those proofs instead of reopening `point`.
pub(in crate::store) async fn visit_page(
    store: &MongoSandboxStore,
    id: &AgentId,
    after: u64,
    through: u64,
    limit: u16,
    predecessor: bool,
    visit: Visitor<'_>,
) -> Result<()> {
    if limit == 0 || limit > 200 || after > through {
        return Err(SandboxStoreError::InvalidPage);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let Some(head) = PreparedIntegrity.read_head(store, id, &mut session).await? else {
        return if through == 0 {
            Ok(())
        } else {
            Err(SandboxStoreError::SequenceConflict)
        };
    };
    if through > head.last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let end = after + (through - after).min(u64::from(limit));
    let start = if predecessor {
        after.saturating_sub(1)
    } else {
        after
    };
    let mut checked = 0_u64;
    let mut accept = |entry: AgentLogEntry| {
        head.validate_lineage(&entry)?;
        if entry.agent_id != *id || entry.seq != start + checked + 1 {
            return Err(SandboxStoreError::SequenceConflict);
        }
        checked += 1;
        visit(entry)
    };
    if let Some(segmented) = &head.segmented {
        segments::visit_range(
            store,
            segmented,
            LogRange {
                after: start,
                through: end,
                limit: (end - start) as u16,
            },
            &mut session,
            true,
            &mut accept,
        )
        .await
        .map_err(domain)?;
    } else if head.trusted {
        let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
        let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
        let mut cursor = source
            .find(doc! {"agent_id":id.as_str(),"seq":{"$gt":start as i64,"$lte":end as i64}})
            .sort(doc! {"seq":1})
            .batch_size(64)
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        while let Some(raw) = read
            .next_in_session(&mut cursor, &mut session)
            .await
            .map_err(unavailable)?
        {
            accept(read_entry(
                &Document::try_from(&raw).map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?,
            )?)?;
        }
        read.complete();
        crate::store::agent_log_integrity::fleet::telemetry::record(
            crate::store::agent_log_integrity::fleet::telemetry::Event::WindowScanAvoided,
        );
    } else {
        if store.bounded_log_enabled(id) {
            crate::store::agent_log_integrity::fleet::telemetry::record(
                crate::store::agent_log_integrity::fleet::telemetry::Event::Fallback,
            );
        }
        scan_collection(
            ScanCaller::PublicWindowPrefix,
            &store.agent_log,
            id,
            head.last_seq,
            &mut session,
            crate::store::agent_log_integrity::ScanOptions::new(true, store.scan_cooperation),
            |entry| {
                head.validate_lineage(&entry)?;
                if entry.seq > start && entry.seq <= end {
                    accept(entry)?;
                }
                Ok(())
            },
        )
        .await
        .map_err(domain)?;
    }
    if checked != end - start {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(())
}

/// Point proof shares the format dispatch; absence past the captured head is
/// distinct from a missing row within it. Legacy keeps its existing point contract.
pub(in crate::store) async fn point(
    store: &MongoSandboxStore,
    id: &AgentId,
    seq: u64,
) -> Result<Option<AgentLogEntry>> {
    if seq == 0 {
        return Ok(None);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let Some(head) = PreparedIntegrity.read_head(store, id, &mut session).await? else {
        return Ok(None);
    };
    if seq > head.last_seq {
        return Ok(None);
    }
    let Some(segmented) = &head.segmented else {
        let entry = store
            .agent_log
            .find_one(doc! {"agent_id":id.as_str(),"seq":seq as i64})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(read_entry)
            .transpose()?;
        if let Some(entry) = &entry {
            head.validate_lineage(entry)?;
            if entry.agent_id != *id || entry.seq != seq {
                return Err(SandboxStoreError::SequenceConflict);
            }
        }
        return Ok(entry);
    };
    let mut result = None;
    segments::visit_range(
        store,
        segmented,
        LogRange {
            after: seq - 1,
            through: seq,
            limit: 1,
        },
        &mut session,
        true,
        |entry| {
            head.validate_lineage(&entry)?;
            result = Some(entry);
            Ok(())
        },
    )
    .await
    .map_err(domain)?;
    Ok(result)
}
