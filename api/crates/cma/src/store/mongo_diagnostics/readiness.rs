//! Feed only already-sanitized metadata to the bounded readiness recorder.
use super::Command;
#[cfg(test)]
use super::Diagnostics;
use crate::readiness_diagnostics::{Metadata, Recorder};
use layer2_core::diagnostic_context::DiagnosticContext;
use mongodb::event::cmap::CmapEvent;
use std::{sync::Arc, time::Duration};

pub(super) fn event(
    recorder: Option<&Arc<Recorder>>,
    command: &Command,
    event: &'static str,
    elapsed: Duration,
) {
    let recorder = recorder.or_else(|| command.check.as_ref().map(|p| &p.recorder));
    let Some(recorder) = recorder else {
        return;
    };
    let mut row = Metadata::new(command.pool, command.context.clone());
    row.command = command.command;
    row.collection = command.collection;
    row.session = command.session;
    row.transaction = command.transaction;
    row.transaction_known = command.transaction_known;
    row.outcome_known = command.outcome_known;
    row.hold_hash = command.hold_hash.clone();
    row.operation = command.operation;
    if let Some(check) = &command.check {
        row.check = check.check;
        check.event(event, row.operation);
    }
    recorder.observe(row, event, elapsed);
}

pub(super) fn lifecycle(recorder: Option<&Arc<Recorder>>, pool: &'static str, event: &CmapEvent) {
    let Some(recorder) = recorder else {
        return;
    };
    let (name, elapsed) = match event {
        CmapEvent::ConnectionCreated(_) => ("connection_created", Duration::ZERO),
        CmapEvent::ConnectionReady(e) => ("connection_ready", e.duration),
        CmapEvent::PoolCreated(_) => ("pool_created", Duration::ZERO),
        CmapEvent::PoolReady(_) => ("pool_ready", Duration::ZERO),
        CmapEvent::PoolCleared(_) => ("pool_cleared", Duration::ZERO),
        CmapEvent::PoolClosed(_) => ("pool_closed", Duration::ZERO),
        _ => return,
    };
    recorder.observe(
        Metadata::new(pool, DiagnosticContext::background("mongo_pool")),
        name,
        elapsed,
    );
}

#[cfg(test)]
pub(crate) fn attach(
    options: &mut mongodb::options::ClientOptions,
    recorder: Arc<Recorder>,
) -> impl Fn(&CmapEvent) + Send + Sync + 'static {
    let diagnostics = Diagnostics {
        2: Some(recorder),
        ..Default::default()
    };
    super::metadata_hook::install(options, diagnostics.clone());
    move |event| diagnostics.cmap(event)
}
