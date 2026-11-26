use super::*;
use layer2_core::sandbox::agent_io::AdmittedAgentCommand;
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write};

pub(super) const RESULTS: [&str; 10] = [
    "unfinished",
    "verified_terminal",
    "receipt_terminal",
    "status_mismatch",
    "duplicate_admission",
    "duplicate_terminal",
    "evidence_missing",
    "evidence_invalid",
    "evidence_unavailable",
    "evidence_deferred",
];
#[derive(Serialize)]
pub(super) struct Entry {
    pub command_id: String,
    pub agent_id: Option<String>,
    pub admitted_at: Option<String>,
    pub status: Option<&'static str>,
    pub age_seconds: Option<u64>,
    pub overdue: bool,
    pub result: &'static str,
}
// Identifiers appear only in an explicitly requested private exception artifact.
impl Entry {
    pub(super) fn has_terminal(&self) -> bool {
        matches!(self.result, "verified_terminal" | "receipt_terminal")
    }

    pub(super) fn invalid(id: &str) -> Self {
        Self {
            command_id: id.into(),
            agent_id: None,
            admitted_at: None,
            status: None,
            age_seconds: None,
            overdue: false,
            result: "invalid_record",
        }
    }
    pub(super) fn from_command(command: &AdmittedAgentCommand, observed: u64) -> Self {
        let at = layer2_core::seconds_from_rfc3339(&command.receipt.admitted_at).unwrap();
        let age = observed.saturating_sub(at);
        let status = match command.status {
            layer2_core::sandbox::agent_io::AgentResponseStatus::Queued => "queued",
            layer2_core::sandbox::agent_io::AgentResponseStatus::InProgress => "in_progress",
            layer2_core::sandbox::agent_io::AgentResponseStatus::WaitingAction => "waiting_action",
            layer2_core::sandbox::agent_io::AgentResponseStatus::Completed => "completed",
            layer2_core::sandbox::agent_io::AgentResponseStatus::Failed => "failed",
            layer2_core::sandbox::agent_io::AgentResponseStatus::Cancelled => "cancelled",
        };
        Self {
            command_id: command.receipt.envelope.command_id.as_str().into(),
            agent_id: Some(command.receipt.envelope.agent_id.as_str().into()),
            admitted_at: Some(command.receipt.admitted_at.clone()),
            status: Some(status),
            age_seconds: Some(age),
            overdue: false,
            result: "unfinished",
        }
    }
}
#[derive(Serialize)]
pub(in crate::store) struct Report {
    format: &'static str,
    selection: Selection,
    from: u64,
    to: u64,
    observed: u64,
    pub(super) operation_time_watermark: Option<(u32, u32)>,
    pub(super) accepted: u64,
    pub(super) classes: BTreeMap<&'static str, u64>,
    statuses: BTreeMap<&'static str, u64>,
    pub(super) overdue: u64,
    oldest_unfinished_age: u64,
    pub(super) exceptions: Vec<Entry>,
    writes: bool,
}
impl Report {
    pub(super) fn new(
        selection: Selection,
        from: u64,
        to: u64,
        observed: u64,
        accepted: u64,
    ) -> Self {
        Self {
            format: "cma-message-task-audit-v1",
            selection,
            from,
            to,
            observed,
            operation_time_watermark: None,
            accepted,
            classes: BTreeMap::new(),
            statuses: BTreeMap::new(),
            overdue: 0,
            oldest_unfinished_age: 0,
            exceptions: Vec::new(),
            writes: false,
        }
    }
    pub(super) fn push(&mut self, entry: Entry) {
        *self.classes.entry(entry.result).or_default() += 1;
        if let Some(status) = entry.status {
            *self.statuses.entry(status).or_default() += 1;
        }
        self.overdue += u64::from(entry.overdue);
        if !entry.has_terminal() {
            self.oldest_unfinished_age = self
                .oldest_unfinished_age
                .max(entry.age_seconds.unwrap_or(0));
        }
        if entry.overdue
            || !matches!(
                entry.result,
                "unfinished" | "verified_terminal" | "receipt_terminal"
            )
        {
            self.exceptions.push(entry);
        }
    }
    pub(in crate::store) fn metrics(&self) -> String {
        let mut text = String::new();
        for (name, value) in [
            ("from_timestamp_seconds", self.from),
            ("to_timestamp_seconds", self.to),
            ("observed_timestamp_seconds", self.observed),
            ("accepted_tasks", self.accepted),
            ("unfinished_oldest_age_seconds", self.oldest_unfinished_age),
            ("overdue_threshold_seconds", self.selection.overdue_seconds),
            ("overdue_tasks", self.overdue),
            (
                "classification_complete",
                u64::from(!self.classes.iter().any(|(key, count)| {
                    *count > 0
                        && !matches!(
                            *key,
                            "unfinished" | "verified_terminal" | "receipt_terminal"
                        )
                })),
            ),
        ] {
            writeln!(text, "cma_task_cohort_{name}{{window=\"release\"}} {value}").unwrap();
        }
        for result in RESULTS.into_iter().chain(["invalid_record"]) {
            writeln!(
                text,
                "cma_task_cohort_audit_tasks{{window=\"release\",result=\"{result}\"}} {}",
                self.classes.get(result).unwrap_or(&0)
            )
            .unwrap();
        }
        for status in [
            "queued",
            "in_progress",
            "waiting_action",
            "completed",
            "failed",
            "cancelled",
        ] {
            writeln!(
                text,
                "cma_task_cohort_tasks{{window=\"release\",status=\"{status}\"}} {}",
                self.statuses.get(status).unwrap_or(&0)
            )
            .unwrap();
        }
        text
    }
}
