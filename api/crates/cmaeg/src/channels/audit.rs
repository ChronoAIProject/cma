//! Operator audit of stored custom attachment settings, run before a rollback.
//!
//! Channel definitions are `deny_unknown_fields`, so an image older than the
//! custom `attachments` setting cannot parse a definition that stores it. The
//! audit pages through every channel record, reports each definition carrying
//! the setting and, when asked, rewrites it without the field. A record it
//! cannot open could still carry the field, so the audit fails closed on it.
//!
//! Nothing here prints or logs a definition, secret, blob or owner subject.
use super::management::seal_definition;
use crate::records::{RecordClient, RecordError, RecordHead, RecordValue};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ids::RecordId,
    seal::{open_blob, MasterKeyRing},
};
use serde::Serialize;
use std::fmt;

/// Records requested per listing page; a shorter page is the last one.
const PAGE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub record_id: RecordId,
    /// Present only when the definition retains its creation channel ID; an
    /// ID is sealed with a random nonce and cannot be recomputed from a record.
    pub channel_hash: Option<String>,
    pub action: AuditAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Found,
    Cleared,
    Conflict,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditReport {
    pub scanned: u64,
    pub channels: u64,
    pub unreadable: u64,
    pub findings: Vec<AuditFinding>,
    pub cleared: u64,
    pub remaining: u64,
}

impl AuditReport {
    /// Whether a rollback below the setting is safe: nothing remains and
    /// every record could be read.
    pub fn complete(&self) -> bool {
        self.remaining == 0 && self.unreadable == 0
    }

    /// The operator output: one JSON line per finding, then the summary.
    pub fn lines(&self) -> Vec<String> {
        #[derive(Serialize)]
        struct Finding<'a> {
            record_id: &'a str,
            channel_hash: Option<&'a str>,
            action: AuditAction,
        }
        #[derive(Serialize)]
        struct Summary {
            scanned: u64,
            channels: u64,
            unreadable: u64,
            found: u64,
            cleared: u64,
            remaining: u64,
        }
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|finding| {
                serde_json::to_string(&Finding {
                    record_id: finding.record_id.as_str(),
                    channel_hash: finding.channel_hash.as_deref(),
                    action: finding.action,
                })
                .expect("an audit finding serializes")
            })
            .collect();
        lines.push(
            serde_json::to_string(&Summary {
                scanned: self.scanned,
                channels: self.channels,
                unreadable: self.unreadable,
                found: self.findings.len() as u64,
                cleared: self.cleared,
                remaining: self.remaining,
            })
            .expect("an audit summary serializes"),
        );
        lines
    }
}

/// Why the audit stopped, classified so the message never carries a URL,
/// token or upstream body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditError {
    Configuration,
    ServiceAuth,
    RecordStoreUnavailable,
    RecordStoreRejected,
    RecordStoreMalformed,
    Seal,
}

impl fmt::Display for AuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Configuration => "configuration invalid",
            Self::ServiceAuth => "service authentication unavailable",
            Self::RecordStoreUnavailable => "record store unavailable",
            Self::RecordStoreRejected => "record store rejected a request",
            Self::RecordStoreMalformed => "record store response malformed",
            Self::Seal => "channel definition cannot be sealed",
        })
    }
}

impl std::error::Error for AuditError {}

impl From<RecordError> for AuditError {
    fn from(error: RecordError) -> Self {
        match error {
            RecordError::Unavailable(_) => Self::RecordStoreUnavailable,
            RecordError::Malformed(_) => Self::RecordStoreMalformed,
            RecordError::NotFound | RecordError::Conflict { .. } | RecordError::Rejected(_) => {
                Self::RecordStoreRejected
            }
        }
    }
}

/// Scan every channel record and, with `clear`, remove each stored setting.
pub async fn audit_custom_attachments(
    records: &RecordClient,
    keys: &MasterKeyRing,
    clear: bool,
) -> Result<AuditReport, AuditError> {
    let mut report = AuditReport::default();
    let mut after: Option<String> = None;
    loop {
        let page = records.list(None, after.as_deref(), PAGE).await?;
        for head in &page {
            report.scanned += 1;
            audit_record(records, keys, head, clear, &mut report).await?;
        }
        if page.len() < PAGE {
            break;
        }
        after = page.last().map(|head| head.record_id.clone());
    }
    tracing::info!(
        scanned = report.scanned,
        found = report.findings.len(),
        cleared = report.cleared,
        remaining = report.remaining,
        unreadable = report.unreadable,
        "custom attachment audit finished"
    );
    Ok(report)
}

async fn audit_record(
    records: &RecordClient,
    keys: &MasterKeyRing,
    head: &RecordHead,
    clear: bool,
    report: &mut AuditReport,
) -> Result<(), AuditError> {
    let Ok(record_id) = RecordId::new(head.record_id.clone()) else {
        report.unreadable += 1;
        tracing::warn!("channel record with an invalid identifier cannot be audited");
        return Ok(());
    };
    let Read::Definition(definition, version) = read(records, keys, &record_id, report).await?
    else {
        return Ok(());
    };
    report.channels += 1;
    if !carries_attachments(&definition) {
        return Ok(());
    }
    let channel_hash = definition
        .creation
        .as_ref()
        .map(|creation| crate::telemetry::channel_hash(creation.channel_id.as_str()));
    let mut finding = AuditFinding {
        record_id: record_id.clone(),
        channel_hash,
        action: AuditAction::Found,
    };
    if !clear {
        tracing::debug!(record_id = %record_id, channel_hash = ?finding.channel_hash, "custom attachments found");
        report.remaining += 1;
    } else if head.owner != definition.owner.subject {
        // The write is keyed by the listed owner; a definition naming another
        // owner is left for an operator rather than rewritten under it.
        tracing::warn!(record_id = %record_id, "channel record owner does not match its definition; not cleared");
        report.remaining += 1;
    } else {
        finding.action = clear_record(records, keys, head, *definition, version, report).await?;
        if finding.action == AuditAction::Cleared {
            report.cleared += 1;
            tracing::info!(record_id = %record_id, channel_hash = ?finding.channel_hash, "custom attachments cleared");
        } else {
            report.remaining += 1;
            tracing::warn!(record_id = %record_id, channel_hash = ?finding.channel_hash, "custom attachments not cleared after a repeated version conflict");
        }
    }
    report.findings.push(finding);
    Ok(())
}

enum Read {
    Definition(Box<ChannelDefinition>, u64),
    /// Deleted since it was listed, so it stores nothing.
    Deleted,
    /// Counted in the report; it may still carry the setting.
    Unreadable,
}

/// Read and open one channel record.
async fn read(
    records: &RecordClient,
    keys: &MasterKeyRing,
    record_id: &RecordId,
    report: &mut AuditReport,
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

/// Rewrite the definition without the setting under compare-and-swap,
/// re-reading once after a conflict. Attachments do not affect the provider
/// binding, so no rebinding is needed.
async fn clear_record(
    records: &RecordClient,
    keys: &MasterKeyRing,
    head: &RecordHead,
    mut definition: ChannelDefinition,
    mut version: u64,
    report: &mut AuditReport,
) -> Result<AuditAction, AuditError> {
    for attempt in 0..2 {
        if !carries_attachments(&definition) {
            // A concurrent update already removed it.
            return Ok(AuditAction::Cleared);
        }
        remove_attachments(&mut definition);
        definition.version = version;
        let blob = seal_definition(keys, &definition).map_err(|_| AuditError::Seal)?;
        match records
            .put(&definition.record_id, &head.owner, &blob, Some(version))
            .await
        {
            Ok(_) => return Ok(AuditAction::Cleared),
            Err(RecordError::Conflict { .. }) if attempt == 0 => {
                let record_id = definition.record_id.clone();
                match read(records, keys, &record_id, report).await? {
                    Read::Definition(current, current_version) => {
                        definition = *current;
                        version = current_version;
                    }
                    Read::Deleted => return Ok(AuditAction::Cleared),
                    Read::Unreadable => return Ok(AuditAction::Conflict),
                }
            }
            Err(RecordError::Conflict { .. }) => return Ok(AuditAction::Conflict),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(AuditAction::Conflict)
}

fn carries_attachments(definition: &ChannelDefinition) -> bool {
    definition.sources.iter().any(|source| {
        matches!(
            source,
            Source::Custom {
                attachments: Some(_),
                ..
            }
        )
    })
}

fn remove_attachments(definition: &mut ChannelDefinition) {
    for source in &mut definition.sources {
        if let Source::Custom { attachments, .. } = source {
            *attachments = None;
        }
    }
}
