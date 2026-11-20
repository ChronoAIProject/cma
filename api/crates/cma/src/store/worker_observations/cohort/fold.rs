use super::Window;
use layer2_core::sandbox::{agent_io::CommandWorkState, AgentOperationDelivery};
use mongodb::bson::{self, doc, Document, Timestamp};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

const STATES: [&str; 6] = [
    "queued",
    "in_progress",
    "waiting_action",
    "completed",
    "failed",
    "cancelled",
];
const SOURCES: [&str; 5] = ["user_interface", "api", "trigger", "channel", "unknown"];
const DIAGNOSTICS: [&str; 8] = [
    "waiting_action",
    "recovery_required",
    "delivery_uncertain",
    "terminal_evidence_unchecked",
    "native_timestamp_missing",
    "consumer_unknown",
    "protocol_unknown",
    "environment_unknown",
];
const AGE: [u64; 5] = [60, 300, 3_600, 86_400, 604_800];

// The inclusion projection deliberately never fetches input text, metadata,
// owner, credentials, authority IDs or arbitrary request/source references.
pub(super) fn projection() -> Document {
    doc! {"_id":1,"agent_id":1,"status":1,"revision":1,
    "command_id":"$record.receipt.envelope.command_id",
    "record_agent_id":"$record.receipt.envelope.agent_id",
    "kind":"$record.receipt.envelope.command.type",
    "source":"$record.receipt.envelope.source.kind",
    "continuation":{"$ne":[{"$ifNull":["$record.receipt.envelope.continuation",null]},null]},
    "admitted_at":"$record.receipt.admitted_at",
    "record_status":"$record.status","record_revision":"$record.revision",
    "work":"$record.work"}
}

pub(super) struct Denominator {
    pub records: u64,
    pub tasks: u64,
}
pub(super) struct Summary {
    window: Window,
    denominator: Denominator,
    pub rows: u64,
    ids: BTreeSet<String>,
    states: [u64; 6],
    controls: u64,
    invalid: u64,
    task_rows: u64,
    sources: BTreeMap<&'static str, u64>,
    diagnostics: BTreeMap<&'static str, u64>,
    continuation: u64,
    ages: [u64; 6],
    oldest: u64,
}
impl Summary {
    pub(super) fn new(window: Window, denominator: Denominator) -> Self {
        Self {
            window,
            denominator,
            rows: 0,
            ids: BTreeSet::new(),
            states: [0; 6],
            controls: 0,
            invalid: 0,
            task_rows: 0,
            sources: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            continuation: 0,
            ages: [0; 6],
            oldest: 0,
        }
    }
    pub(super) fn observe(&mut self, row: Document) -> Result<(), &'static str> {
        self.rows += 1;
        // This is independent from successful classification, including bad
        // message rows in the denominator cross-check rather than losing them.
        if row.get_str("kind").ok() == Some("message") {
            self.task_rows += 1;
        }
        let id = row.get_str("_id").map_err(|_| "invalid_snapshot")?;
        if id.is_empty() || !self.ids.insert(id.to_owned()) {
            return Err("invalid_snapshot");
        }
        if self.classify(&row).is_err() {
            self.invalid += 1;
        }
        Ok(())
    }
    fn classify(&mut self, row: &Document) -> Result<(), ()> {
        let text = |key| row.get_str(key).map_err(|_| ());
        let integer = |key| {
            row.get_i64(key)
                .or_else(|_| row.get_i32(key).map(i64::from))
                .map_err(|_| ())
        };
        let id = text("_id")?;
        let agent = text("agent_id")?;
        let state = STATES
            .iter()
            .position(|state| Some(*state) == row.get_str("status").ok())
            .ok_or(())?;
        let at = layer2_core::seconds_from_rfc3339(text("admitted_at")?).ok_or(())?;
        if id != text("command_id")?
            || agent.is_empty()
            || agent != text("record_agent_id")?
            || text("status")? != text("record_status")?
            || integer("revision")? < 1
            || integer("revision")? != integer("record_revision")?
            || at < self.window.from
            || at >= self.window.to
        {
            return Err(());
        }
        let kind = text("kind")?;
        if !matches!(kind, "message" | "steer" | "interrupt" | "answer_approval") {
            return Err(());
        }
        let work: CommandWorkState = match row.get("work") {
            Some(value) => bson::deserialize_from_bson(value.clone()).map_err(|_| ())?,
            None => CommandWorkState::default(), // Retained pre-worker receipts.
        };
        if work.dispatch.as_ref().is_some_and(|dispatch| {
            dispatch.claim.command_id.as_str() != id
                || dispatch.claim.agent_id.as_str() != agent
                || dispatch.operation_sequence == 0
                || dispatch.turn_sequence == 0
                || dispatch.runtime_generation == 0
                || dispatch.claim.fence == 0
                || layer2_core::seconds_from_rfc3339(&dispatch.fenced_at).is_none()
        }) || (state == 0 && work.dispatch.is_some())
        {
            return Err(());
        }
        let continuation = row.get_bool("continuation").map_err(|_| ())?;
        if kind != "message" {
            self.controls += 1;
            return Ok(());
        }
        self.states[state] += 1;
        let source = SOURCES
            .iter()
            .copied()
            .find(|source| Some(*source) == row.get_str("source").ok())
            .unwrap_or("unknown");
        *self.sources.entry(source).or_default() += 1;
        self.continuation += u64::from(continuation);
        let mut diagnostic = |reason| *self.diagnostics.entry(reason).or_default() += 1;
        diagnostic("consumer_unknown");
        diagnostic("protocol_unknown");
        diagnostic("environment_unknown");
        if state < 3 {
            let age = self.window.observed.saturating_sub(at);
            self.oldest = self.oldest.max(age);
            self.ages[AGE.iter().position(|bound| age <= *bound).unwrap_or(5)] += 1;
            if state == 2 {
                diagnostic("waiting_action");
            }
        } else {
            diagnostic("terminal_evidence_unchecked");
            diagnostic("native_timestamp_missing");
        }
        if let Some(dispatch) = &work.dispatch {
            let delivery = work
                .observation
                .as_ref()
                .map_or(dispatch.delivery, |observation| observation.delivery);
            if delivery == AgentOperationDelivery::RecoveryRequired {
                diagnostic("recovery_required");
            }
            if matches!(
                delivery,
                AgentOperationDelivery::DispatchFenced
                    | AgentOperationDelivery::RecoveryRequired
                    | AgentOperationDelivery::RuntimeLost
            ) {
                diagnostic("delivery_uncertain");
            }
        }
        Ok(())
    }
    pub(super) fn finish(self, snapshot: Timestamp) -> Result<String, &'static str> {
        if self.rows != self.denominator.records || self.task_rows != self.denominator.tasks {
            return Err("invalid_snapshot");
        }
        let window = self.window.name;
        let mut text = String::new();
        for (name, value) in [
            ("from_timestamp_seconds", self.window.from),
            ("to_timestamp_seconds", self.window.to),
            ("observed_timestamp_seconds", self.window.observed),
            (
                "snapshot_cluster_timestamp_seconds",
                u64::from(snapshot.time),
            ),
            ("snapshot_cluster_increment", u64::from(snapshot.increment)),
            ("records", self.rows),
            ("accepted_tasks", self.denominator.tasks),
            ("control_commands", self.controls),
            (
                "unclassified_tasks",
                self.denominator.tasks - self.states.iter().sum::<u64>(),
            ),
            ("invalid_records", self.invalid),
            ("classification_complete", u64::from(self.invalid == 0)),
            ("unfinished_oldest_age_seconds", self.oldest),
            ("continuation_tasks", self.continuation),
        ] {
            writeln!(
                text,
                "cma_task_cohort_{name}{{window=\"{window}\"}} {value}"
            )
            .unwrap();
        }
        for (state, count) in STATES.iter().zip(self.states) {
            writeln!(
                text,
                "cma_task_cohort_tasks{{window=\"{window}\",status=\"{state}\"}} {count}"
            )
            .unwrap();
        }
        for source in SOURCES {
            writeln!(
                text,
                "cma_task_cohort_source_tasks{{window=\"{window}\",source=\"{source}\"}} {}",
                self.sources.get(source).unwrap_or(&0)
            )
            .unwrap();
        }
        for reason in DIAGNOSTICS {
            writeln!(
                text,
                "cma_task_cohort_diagnostic_tasks{{window=\"{window}\",reason=\"{reason}\"}} {}",
                self.diagnostics.get(reason).unwrap_or(&0)
            )
            .unwrap();
        }
        // Disjoint snapshot age bands (gauges), not observations repeatedly
        // appended to a process histogram. Never rate/increase these values.
        for (band, count) in [
            "le_60",
            "61_300",
            "301_3600",
            "3601_86400",
            "86401_604800",
            "gt_604800",
        ]
        .iter()
        .zip(self.ages)
        {
            writeln!(text, "cma_task_cohort_unfinished_age_items{{window=\"{window}\",band=\"{band}\"}} {count}").unwrap();
        }
        // These inputs have no trusted durable source in the command receipt.
        // Missing coverage is explicit even for an empty cohort; it cannot be
        // interpreted as a measured zero mismatches or zero native latency.
        for source in [
            "terminal_evidence",
            "native_timestamp",
            "consumer",
            "protocol",
            "environment",
            "edge_requests",
            "unledgered_tasks",
        ] {
            writeln!(
                text,
                "cma_task_cohort_evidence_supported{{window=\"{window}\",source=\"{source}\"}} 0"
            )
            .unwrap();
        }
        Ok(text)
    }
}
