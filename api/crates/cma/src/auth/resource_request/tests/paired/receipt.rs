use serde::Serialize;
use std::time::Instant;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Ordinary,
    Mcp,
    RequestBinding,
    Rest,
    PermissionsRevoked,
    PermissionsRestored,
    ServiceRevoked,
    ServiceRestored,
    OrdinaryRevoked,
    ProofRevoked,
    ProofRestored,
    Stop,
    ShutdownAcknowledged,
    ListenerClosed,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Started,
    Passed,
    Failed,
}

#[derive(Serialize)]
struct Event<'a> {
    phase: Phase,
    status: Status,
    elapsed_ms: u128,
    http_statuses: &'a [u16],
}

pub(super) struct Receipt {
    phase: Phase,
    start: Instant,
    statuses: Vec<u16>,
    finished: bool,
}

impl Receipt {
    pub(super) fn start(phase: Phase) -> Self {
        let receipt = Self {
            phase,
            start: Instant::now(),
            statuses: Vec::new(),
            finished: false,
        };
        receipt.emit(Status::Started);
        receipt
    }

    pub(super) fn status(&mut self, status: u16) {
        self.statuses.push(status);
    }

    pub(super) fn pass(mut self) {
        self.emit(Status::Passed);
        self.finished = true;
    }

    fn emit(&self, status: Status) {
        // Only closed enums, timings and status codes enter this output channel.
        // The supervisor discards all other test output, including panic payloads.
        eprintln!(
            "CMA_NYXID_PAIR_V1 {}",
            serde_json::to_string(&Event {
                phase: self.phase,
                status,
                elapsed_ms: self.start.elapsed().as_millis(),
                http_statuses: &self.statuses,
            })
            .unwrap()
        );
    }
}

impl Drop for Receipt {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(Status::Failed);
        }
    }
}
