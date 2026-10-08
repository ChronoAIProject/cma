//! Operator audit of stored multi-consumer extensions, run before a rollback.
//!
//! Every stored shape the multi-consumer release adds is
//! `deny_unknown_fields`, so an image older than it answers a channel storing
//! a routing policy, consumer keys, an extended source list or a partition
//! `custom_key` with 503 `channel_invalid`. The audit pages through every
//! channel record and reports each definition that is not legacy-compatible,
//! with whether `default` would still receive every event once routing is
//! gone. A record it cannot open, or a definition the reader refuses, could
//! still store an extension, so the audit fails closed on it.
//!
//! Output and logs carry only the record ID, the channel hash and
//! classifications: never a definition, profile, secret, key, blob, token,
//! owner subject or channel ID.
use super::{
    audit::{AuditError, AuditReport},
    audit_pages::{for_each_definition, Listed},
};
use crate::records::RecordClient;
use cmaeg_core::{
    channel::{default_receives_all, ChannelDefinition, Extensions},
    ids::RecordId,
    seal::MasterKeyRing,
};
use serde::Serialize;
use std::collections::BTreeSet;

/// What the audit did with a definition that is not legacy-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingAuditAction {
    /// Reported, not cleared.
    Found,
    /// Returned to the legacy shape by this run.
    Cleared,
    /// Left unchanged because it needs an owner decision; `reason` says why.
    Blocked,
    /// The record changed again while it was being cleared; it remains.
    Conflict,
}

/// One definition an older image cannot read. It serializes as the
/// operator's JSON line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutingFinding {
    pub record_id: RecordId,
    /// Present only when the definition retains its creation channel ID; an
    /// ID is sealed with a random nonce and cannot be recomputed from a record.
    pub channel_hash: Option<String>,
    /// `routing` and/or `sources`, in that order.
    pub extensions: Vec<&'static str>,
    /// Whether every event the stored policy can admit reaches `default`.
    pub default_receives_all: bool,
    pub action: RoutingAuditAction,
    /// Why a definition is `blocked` or in `conflict`.
    pub reason: Option<&'static str>,
}

/// What the audit may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingAuditMode {
    /// Report only: no write and no provider call.
    Report,
}

impl AuditReport<RoutingFinding> {
    /// Definitions left unchanged because they need an owner decision.
    pub fn blocked(&self) -> u64 {
        self.findings
            .iter()
            .filter(|finding| finding.action == RoutingAuditAction::Blocked)
            .count() as u64
    }

    /// The operator output: one JSON line per finding, then the summary.
    pub fn lines(&self) -> Vec<String> {
        #[derive(Serialize)]
        struct Summary {
            scanned: u64,
            channels: u64,
            unreadable: u64,
            found: u64,
            cleared: u64,
            blocked: u64,
            remaining: u64,
        }
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|finding| serde_json::to_string(finding).expect("a routing finding serializes"))
            .collect();
        lines.push(
            serde_json::to_string(&Summary {
                scanned: self.scanned,
                channels: self.channels,
                unreadable: self.unreadable,
                found: self.findings.len() as u64,
                cleared: self.cleared,
                blocked: self.blocked(),
                remaining: self.remaining,
            })
            .expect("a routing audit summary serializes"),
        );
        lines
    }
}

/// Scan every channel record and report each definition that is not
/// legacy-compatible. `non_human` is the configured non-human provider set,
/// which decides whether `default` answers custom events.
pub async fn audit_routing(
    records: &RecordClient,
    keys: &MasterKeyRing,
    non_human: &BTreeSet<String>,
    mode: RoutingAuditMode,
) -> Result<AuditReport<RoutingFinding>, AuditError> {
    let mut report = AuditReport::default();
    for_each_definition(records, keys, &mut report, async |report, listed| {
        audit_record(listed, non_human, mode, report);
        Ok(())
    })
    .await?;
    tracing::info!(
        scanned = report.scanned,
        found = report.findings.len(),
        cleared = report.cleared,
        blocked = report.blocked(),
        remaining = report.remaining,
        unreadable = report.unreadable,
        "routing audit finished"
    );
    Ok(report)
}

fn audit_record(
    listed: Listed<'_>,
    non_human: &BTreeSet<String>,
    mode: RoutingAuditMode,
    report: &mut AuditReport<RoutingFinding>,
) {
    let Listed {
        record_id,
        definition,
        ..
    } = listed;
    let Some(finding) = classify(record_id, &definition, non_human, report) else {
        return;
    };
    match mode {
        RoutingAuditMode::Report => {
            tracing::debug!(
                record_id = %finding.record_id,
                channel_hash = ?finding.channel_hash,
                extensions = ?finding.extensions,
                default_receives_all = finding.default_receives_all,
                "routing extensions found"
            );
            report.remaining += 1;
        }
    }
    report.findings.push(finding);
}

/// The finding for a definition an older image cannot read, or `None` for a
/// legacy-compatible one. A definition the reader refuses cannot be
/// classified, so it is counted as unreadable and the audit fails closed.
fn classify(
    record_id: RecordId,
    definition: &ChannelDefinition,
    non_human: &BTreeSet<String>,
    report: &mut AuditReport<RoutingFinding>,
) -> Option<RoutingFinding> {
    if definition.validate().is_err() {
        report.unreadable += 1;
        tracing::warn!(record_id = %record_id, "channel definition is invalid and cannot be classified");
        return None;
    }
    let extensions = Extensions::of(definition);
    if extensions.legacy_compatible() {
        return None;
    }
    Some(RoutingFinding {
        channel_hash: channel_hash(definition),
        extensions: names(extensions),
        default_receives_all: default_receives_all(definition, non_human),
        record_id,
        action: RoutingAuditAction::Found,
        reason: None,
    })
}

/// The extension names an operator sees, in a fixed order.
fn names(extensions: Extensions) -> Vec<&'static str> {
    [
        (extensions.routing, "routing"),
        (extensions.sources, "sources"),
    ]
    .into_iter()
    .filter_map(|(present, name)| present.then_some(name))
    .collect()
}

fn channel_hash(definition: &ChannelDefinition) -> Option<String> {
    definition
        .creation
        .as_ref()
        .map(|creation| crate::telemetry::channel_hash(creation.channel_id.as_str()))
}
