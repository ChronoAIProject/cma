//! Operator audit of stored custom attachment settings, run before a rollback.
//!
//! Channel definitions are `deny_unknown_fields`, so an image older than the
//! custom `attachments` setting cannot parse a definition that stores it. The
//! audit pages through every channel record, reports each definition carrying
//! the setting and, when asked, rewrites it without the field. A record it
//! cannot open could still carry the field, so the audit fails closed on it.
//!
//! Nothing here prints or logs a definition, secret, blob or owner subject.
use super::audit_pages::{for_each_definition, rewrite, Listed, Rewritten, Step};
use crate::records::{RecordClient, RecordError, RecordHead};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ids::RecordId,
    seal::MasterKeyRing,
};
use serde::Serialize;
use std::fmt;

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

/// An audit's result: the shared record counters and its own findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport<F = AuditFinding> {
    pub scanned: u64,
    pub channels: u64,
    pub unreadable: u64,
    pub findings: Vec<F>,
    pub cleared: u64,
    pub remaining: u64,
}

impl<F> Default for AuditReport<F> {
    fn default() -> Self {
        Self {
            scanned: 0,
            channels: 0,
            unreadable: 0,
            findings: Vec::new(),
            cleared: 0,
            remaining: 0,
        }
    }
}

impl<F> AuditReport<F> {
    /// Whether a rollback below what the audit looks for is safe: nothing
    /// remains and every record could be read.
    pub fn complete(&self) -> bool {
        self.remaining == 0 && self.unreadable == 0
    }
}

impl AuditReport {
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
    for_each_definition(records, keys, &mut report, async |report, listed| {
        audit_record(records, keys, listed, clear, report).await
    })
    .await?;
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
    listed: Listed<'_>,
    clear: bool,
    report: &mut AuditReport,
) -> Result<(), AuditError> {
    let Listed {
        head,
        record_id,
        definition,
        version,
    } = listed;
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
        finding.action = clear_record(records, keys, head, definition, version, report).await?;
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

/// Rewrite the definition without the setting under compare-and-swap,
/// re-reading once after a conflict. Attachments do not affect the provider
/// binding, so no rebinding is needed.
async fn clear_record(
    records: &RecordClient,
    keys: &MasterKeyRing,
    head: &RecordHead,
    definition: ChannelDefinition,
    version: u64,
    report: &mut AuditReport,
) -> Result<AuditAction, AuditError> {
    let rewritten = rewrite(
        records,
        keys,
        head,
        definition,
        version,
        report,
        async |mut definition| {
            if !carries_attachments(&definition) {
                // A concurrent update already removed it.
                return Ok(Step::Done);
            }
            remove_attachments(&mut definition);
            Ok(Step::Write(Box::new(definition)))
        },
    )
    .await?;
    Ok(match rewritten {
        Rewritten::Cleared => AuditAction::Cleared,
        Rewritten::Conflict => AuditAction::Conflict,
    })
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
