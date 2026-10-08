//! Record paging shared by the operator audits, which run before a rollback.
//!
//! Every audit pages through every channel record with the gateway's service
//! credential, opens each one with the mounted master keys and hands each
//! readable definition to its own per-definition step. A record that cannot
//! be opened could still store whatever the audit looks for, so it is counted
//! as unreadable and the audit fails closed on it. Rewrites go through
//! compare-and-swap, re-reading once after a version conflict.
//!
//! Nothing here prints or logs a definition, secret, blob or owner subject.
use super::{
    audit::{AuditError, AuditReport},
    management::seal_definition,
};
use crate::records::{RecordClient, RecordError, RecordHead, RecordValue};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::RecordId,
    seal::{open_blob, MasterKeyRing},
};

/// Records requested per listing page; a shorter page is the last one.
const PAGE: usize = 100;

/// One readable channel definition, as the record store listed it.
pub(super) struct Listed<'a> {
    pub(super) head: &'a RecordHead,
    pub(super) record_id: RecordId,
    pub(super) definition: ChannelDefinition,
    pub(super) version: u64,
}

/// Page through every channel record, counting each one, and hand every
/// readable definition to `visit`. A record deleted since it was listed
/// stores nothing and is skipped.
pub(super) async fn for_each_definition<F>(
    records: &RecordClient,
    keys: &MasterKeyRing,
    report: &mut AuditReport<F>,
    mut visit: impl AsyncFnMut(&mut AuditReport<F>, Listed<'_>) -> Result<(), AuditError>,
) -> Result<(), AuditError> {
    let mut after: Option<String> = None;
    loop {
        let page = records.list(None, after.as_deref(), PAGE).await?;
        for head in &page {
            report.scanned += 1;
            let Ok(record_id) = RecordId::new(head.record_id.clone()) else {
                report.unreadable += 1;
                tracing::warn!("channel record with an invalid identifier cannot be audited");
                continue;
            };
            let Read::Definition(definition, version) =
                read(records, keys, &record_id, report).await?
            else {
                continue;
            };
            report.channels += 1;
            let listed = Listed {
                head,
                record_id,
                definition: *definition,
                version,
            };
            visit(report, listed).await?;
        }
        if page.len() < PAGE {
            break;
        }
        after = page.last().map(|head| head.record_id.clone());
    }
    Ok(())
}

enum Read {
    Definition(Box<ChannelDefinition>, u64),
    /// Deleted since it was listed, so it stores nothing.
    Deleted,
    /// Counted in the report; it may still store what the audit looks for.
    Unreadable,
}

/// Read and open one channel record.
async fn read<F>(
    records: &RecordClient,
    keys: &MasterKeyRing,
    record_id: &RecordId,
    report: &mut AuditReport<F>,
) -> Result<Read, AuditError> {
    let Ok(url) = records.record_url(record_id.as_str()) else {
        report.unreadable += 1;
        tracing::warn!(record_id = %record_id, "channel record has no valid record URL");
        return Ok(Read::Unreadable);
    };
    let RecordValue { blob, version } = match records.get(url.as_str()).await {
        Ok(record) => record,
        Err(RecordError::NotFound) => return Ok(Read::Deleted),
        Err(error) => return Err(error.into()),
    };
    let definition = open_blob(keys, &blob)
        .ok()
        .and_then(|opened| serde_json::from_slice::<ChannelDefinition>(&opened.payload).ok());
    match definition {
        Some(definition) => Ok(Read::Definition(Box::new(definition), version)),
        None => {
            report.unreadable += 1;
            tracing::warn!(record_id = %record_id, "channel record cannot be opened as a channel definition");
            Ok(Read::Unreadable)
        }
    }
}

/// What an audit's step wants done with the current definition.
pub(super) enum Step {
    /// Nothing is left to change, for example after a concurrent update.
    Done,
    /// Store this definition in place of the current one.
    Write(Box<ChannelDefinition>),
}

/// How a compare-and-swap rewrite ended.
pub(super) enum Rewritten {
    /// Written, found already done, or deleted meanwhile: nothing remains.
    Cleared,
    /// The record changed again, or became unreadable, after one re-read.
    Conflict,
}

/// Rewrite one definition under compare-and-swap. `step` decides what to
/// write from the current definition; after one version conflict the record
/// is re-read and `step` decides again from what is stored now. The write is
/// keyed by the listed owner, and the stored definition records the version
/// it was written over, as updates do.
pub(super) async fn rewrite<F>(
    records: &RecordClient,
    keys: &MasterKeyRing,
    head: &RecordHead,
    mut definition: ChannelDefinition,
    mut version: u64,
    report: &mut AuditReport<F>,
    mut step: impl AsyncFnMut(ChannelDefinition) -> Result<Step, AuditError>,
) -> Result<Rewritten, AuditError> {
    for attempt in 0..2 {
        let mut next = match step(definition).await? {
            Step::Done => return Ok(Rewritten::Cleared),
            Step::Write(next) => *next,
        };
        next.version = version;
        let blob = seal_definition(keys, &next).map_err(|_| AuditError::Seal)?;
        match records
            .put(&next.record_id, &head.owner, &blob, Some(version))
            .await
        {
            Ok(_) => return Ok(Rewritten::Cleared),
            Err(RecordError::Conflict { .. }) if attempt == 0 => {
                match read(records, keys, &next.record_id, report).await? {
                    Read::Definition(current, current_version) => {
                        definition = *current;
                        version = current_version;
                    }
                    Read::Deleted => return Ok(Rewritten::Cleared),
                    Read::Unreadable => return Ok(Rewritten::Conflict),
                }
            }
            Err(RecordError::Conflict { .. }) => return Ok(Rewritten::Conflict),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Rewritten::Conflict)
}
