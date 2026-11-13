use super::{config, writer, Operation, Stage, Writer};
use std::sync::OnceLock;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::writer::{EitherWriter, MakeWriter};

static PROCESS: OnceLock<Process> = OnceLock::new();

struct Process {
    writer: Writer,
    logs: writer::BoundedLogs,
    _guard: WorkerGuard,
}

/// OFF returns before thread, queue, instance ID, timestamp or header work.
pub(crate) fn log_writer() -> ServiceLogWriter {
    configured_writer(config::enabled().unwrap_or(false))
}

fn configured_writer(enabled: bool) -> ServiceLogWriter {
    if !enabled {
        return ServiceLogWriter(None);
    }
    if PROCESS.get().is_none() {
        match writer::start(std::io::stdout()) {
            Ok(output) => {
                let _ = PROCESS.set(Process {
                    writer: output.writer,
                    logs: output.logs,
                    _guard: output.guard,
                });
            }
            Err(_) => return ServiceLogWriter(None),
        }
    }
    ServiceLogWriter(PROCESS.get().map(|process| process.logs.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_gate_constructs_no_process_or_operation() {
        assert!(PROCESS.get().is_none());
        assert!(configured_writer(false).0.is_none());
        assert!(start(Stage::Gateway).is_none());
        assert!(PROCESS.get().is_none());
    }
}

pub(crate) fn writer() -> Option<Writer> {
    #[cfg(test)]
    if let Some(writer) = super::testing::writer() {
        return Some(writer);
    }
    PROCESS.get().map(|process| process.writer.clone())
}

pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if super::testing::writer().is_some() {
        return true;
    }
    PROCESS.get().is_some()
}

pub(crate) fn start(stage: Stage) -> Option<Operation> {
    start_part(stage, None)
}

pub(crate) fn start_part(stage: Stage, part_index: Option<u32>) -> Option<Operation> {
    let writer = writer()?;
    let parent = super::scope::context(&tracing::Span::current())?;
    Some(Operation::start(writer, stage, Some(parent), part_index))
}

pub(crate) fn milestone(span: &tracing::Span, stage: Stage, outcome: super::Outcome) {
    let Some(writer) = writer() else {
        return;
    };
    let Some(context) = super::scope::context(span) else {
        return;
    };
    writer.send(super::record::Record {
        timestamp: std::time::SystemTime::now(),
        context,
        stage,
        phase: super::record::Phase::Milestone,
        outcome: Some(outcome),
        duration_ms: None,
        http_status: None,
        part_index: None,
        related_turn_id: None,
    });
}

#[derive(Clone)]
pub(crate) struct ServiceLogWriter(Option<writer::BoundedLogs>);

impl<'a> MakeWriter<'a> for ServiceLogWriter {
    type Writer = EitherWriter<writer::BoundedLogs, std::io::Stdout>;

    fn make_writer(&'a self) -> Self::Writer {
        match &self.0 {
            Some(output) => EitherWriter::A(output.clone()),
            None => EitherWriter::B(std::io::stdout()),
        }
    }
}
