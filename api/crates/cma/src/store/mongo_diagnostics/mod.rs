//! Metadata-only Mongo diagnostics. Never format a command, reply, error or address.
use super::mongo_latency::connection_key;
use layer2_core::diagnostic_context::DiagnosticContext;
use mongodb::{
    bson::Document,
    event::{cmap::CmapEvent, command::CommandEvent},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
mod periodic;
mod summary;
use sha2::{Digest, Sha256};
use summary::Summary;
const MAX_PENDING: usize = 4096;
const SLOW: Duration = Duration::from_millis(100);
const SAMPLE_EVERY: u64 = 100;

#[derive(Clone)]
struct Command {
    context: DiagnosticContext,
    pool: &'static str,
    hold_hash: Arc<str>,
    command: &'static str,
    collection: &'static str,
    session: bool,
    transaction: bool,
    sampled: bool,
}
struct Hold {
    hash: Arc<str>,
    start: Instant,
    context: DiagnosticContext,
    last: Option<Command>,
    commands: u64,
    sampled: bool,
}
struct State {
    salt: [u8; 16],
    commands: HashMap<(u64, i32), Command>,
    holds: HashMap<u64, Hold>,
    command_sequence: u64,
    checkout_sequence: u64,
    summary: Summary,
}
#[derive(Clone)]
pub(super) struct Diagnostics(Arc<Mutex<State>>, &'static str);
impl Default for Diagnostics {
    fn default() -> Self {
        Self(
            Arc::new(Mutex::new(State {
                salt: *uuid::Uuid::new_v4().as_bytes(),
                commands: HashMap::new(),
                holds: HashMap::new(),
                command_sequence: 0,
                checkout_sequence: 0,
                summary: Summary::default(),
            })),
            "general",
        )
    }
}
pub(super) fn install(
    options: &mut mongodb::options::ClientOptions,
    pool: &'static str,
) -> Option<Diagnostics> {
    if std::env::var("CHRONO_MONGO_DIAGNOSTICS_ENABLED").as_deref() == Ok("false") {
        return None;
    }
    let mut diagnostics = Diagnostics::default();
    diagnostics.1 = pool;
    let commands = diagnostics.clone();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        commands.command(event)
    }));
    // Dropping the handle detaches; the task only keeps a Weak client state.
    drop(periodic::start(&diagnostics));
    Some(diagnostics)
}
impl Diagnostics {
    pub(super) fn command(&self, event: CommandEvent) {
        match event {
            CommandEvent::Started(event) => {
                let key = connection_key(&event.connection.address, event.connection.id);
                let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
                state.command_sequence = state.command_sequence.wrapping_add(1);
                let mut command = metadata(
                    &event.command_name,
                    &event.command,
                    state.command_sequence.is_multiple_of(SAMPLE_EVERY),
                );
                command.pool = self.1;
                if let Some(hold) = state.holds.get_mut(&key) {
                    command.hold_hash = hold.hash.clone();
                    hold.commands = hold.commands.saturating_add(1);
                    hold.last = Some(command.clone());
                }
                if state.commands.len() < MAX_PENDING {
                    state
                        .commands
                        .insert((key, event.request_id), command.clone());
                } else {
                    state.summary.untracked = state.summary.untracked.saturating_add(1);
                }
                drop(state);
                if command.sampled {
                    command.emit("started", Duration::ZERO, None, 0, false);
                }
            }
            CommandEvent::Succeeded(event) => self.finished(
                connection_key(&event.connection.address, event.connection.id),
                event.request_id,
                event.duration,
                docs_returned(&event.reply),
                false,
            ),
            CommandEvent::Failed(event) => self.finished(
                connection_key(&event.connection.address, event.connection.id),
                event.request_id,
                event.duration,
                None,
                true,
            ),
            _ => {}
        }
    }
    fn finished(&self, key: u64, id: i32, duration: Duration, docs: Option<u64>, failed: bool) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut command = state
            .commands
            .remove(&(key, id))
            .unwrap_or_else(|| metadata("other", &Document::new(), false));
        command.pool = self.1;
        let hold_us = state
            .holds
            .get(&key)
            .map_or(0, |h| micros(h.start.elapsed()));
        let completed = state
            .summary
            .record(&command, "command", duration, docs, failed);
        drop(state);
        summary::emit(completed);
        if command.sampled || duration >= SLOW || failed {
            // Unsampled starts are deferred until completion so every slow command
            // retains its started metadata without logging every fast command.
            if !command.sampled {
                command.emit("started", Duration::ZERO, None, 0, true);
            }
            command.emit(
                if failed { "failed" } else { "succeeded" },
                duration,
                docs,
                hold_us,
                false,
            );
        }
    }
    pub(super) fn cmap(&self, event: &CmapEvent) {
        match event {
            CmapEvent::ConnectionCheckedOut(event) => {
                let context = DiagnosticContext::current();
                let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
                state.checkout_sequence = state.checkout_sequence.wrapping_add(1);
                let sampled = state.checkout_sequence.is_multiple_of(SAMPLE_EVERY);
                let mut hash = Sha256::new();
                hash.update(state.salt);
                hash.update(state.checkout_sequence.to_le_bytes());
                let hold_hash: Arc<str> = format!("{:x}", hash.finalize()).into();
                if state.holds.len() < MAX_PENDING {
                    state.holds.insert(
                        connection_key(&event.address, event.connection_id),
                        Hold {
                            hash: hold_hash.clone(),
                            start: Instant::now(),
                            context: context.clone(),
                            last: None,
                            commands: 0,
                            sampled,
                        },
                    );
                } else {
                    state.summary.untracked = state.summary.untracked.saturating_add(1);
                }
                let mut metadata = context_command(context);
                metadata.pool = self.1;
                metadata.hold_hash = hold_hash;
                let completed =
                    state
                        .summary
                        .record(&metadata, "checkout", event.duration, None, false);
                drop(state);
                summary::emit(completed);
                if sampled || event.duration >= SLOW {
                    metadata.emit("checked_out", event.duration, None, 0, false);
                }
            }
            CmapEvent::ConnectionCheckedIn(event) => {
                self.returned(connection_key(&event.address, event.connection_id), false)
            }
            CmapEvent::ConnectionClosed(event) => {
                self.returned(connection_key(&event.address, event.connection_id), true)
            }
            CmapEvent::ConnectionCheckoutFailed(event) => {
                let mut command = context_command(DiagnosticContext::current());
                command.pool = self.1;
                let completed = self
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .summary
                    .record(&command, "checkout", event.duration, None, true);
                summary::emit(completed);
                command.emit("checkout_failed", event.duration, None, 0, false);
            }
            _ => {}
        }
    }
    fn returned(&self, key: u64, closed: bool) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(hold) = state.holds.remove(&key) else {
            return;
        };
        state
            .commands
            .retain(|(connection, _), _| *connection != key);
        let duration = hold.start.elapsed();
        let mut command = hold
            .last
            .unwrap_or_else(|| context_command(hold.context.clone()));
        command.pool = self.1;
        command.hold_hash = hold.hash;
        // A pinned transaction may run several commands/phases. Do not assign
        // its whole physical hold to the last command or count it repeatedly.
        if hold.commands > 1 {
            command.command = "multiple";
            command.collection = "multiple";
            command.context = hold.context;
        }
        let completed = state
            .summary
            .record(&command, "hold", duration, None, closed);
        drop(state);
        summary::emit(completed);
        if hold.sampled || duration >= SLOW || closed {
            command.emit(
                if closed {
                    "connection_closed"
                } else {
                    "checked_in"
                },
                duration,
                None,
                micros(duration),
                false,
            );
        }
    }
}
fn context_command(context: DiagnosticContext) -> Command {
    Command {
        context,
        pool: "general",
        hold_hash: "none".into(),
        command: "none",
        collection: "none",
        session: false,
        transaction: false,
        sampled: false,
    }
}
fn metadata(name: &str, body: &Document, sampled: bool) -> Command {
    let command = match name {
        "find" => "find",
        "getMore" => "getMore",
        "aggregate" => "aggregate",
        "insert" => "insert",
        "update" => "update",
        "delete" => "delete",
        "findAndModify" => "findAndModify",
        "commitTransaction" => "commitTransaction",
        "abortTransaction" => "abortTransaction",
        "killCursors" => "killCursors",
        "count" => "count",
        "distinct" => "distinct",
        "ping" => "ping",
        "hello" => "hello",
        "listCollections" => "listCollections",
        "listIndexes" => "listIndexes",
        "createIndexes" => "createIndexes",
        "collMod" => "collMod",
        "create" => "create",
        "drop" => "drop",
        "dropDatabase" => "dropDatabase",
        _ => "other",
    };
    let collection = body
        .get_str(if command == "getMore" {
            "collection"
        } else {
            command
        })
        .ok()
        .and_then(|value| {
            super::isolation::COLLECTIONS
                .iter()
                .copied()
                .find(|known| *known == value)
        })
        .unwrap_or("other");
    Command {
        context: DiagnosticContext::current(),
        pool: "general",
        hold_hash: "none".into(),
        command,
        collection,
        session: body.contains_key("lsid"),
        transaction: matches!(body.get_bool("autocommit"), Ok(false)),
        sampled,
    }
}
fn docs_returned(reply: &Document) -> Option<u64> {
    let cursor = reply.get_document("cursor").ok()?;
    cursor
        .get_array("firstBatch")
        .or_else(|_| cursor.get_array("nextBatch"))
        .ok()
        .map(|batch| batch.len() as u64)
}
fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
impl Command {
    fn emit(
        &self,
        event: &'static str,
        duration: Duration,
        docs: Option<u64>,
        hold_us: u64,
        deferred: bool,
    ) {
        tracing::info!(target: "mongo_diagnostic", parent: None,
            event, pool = self.pool, hold_hash = %self.hold_hash, route = self.context.route.as_ref(), request_hash = %self.context.request_hash,
            worker = self.context.worker, phase = self.context.phase,
            command = self.command, collection = self.collection,
            duration_us = micros(duration), docs_returned = docs.unwrap_or(0), docs_returned_known = docs.is_some(), session = self.session,
            transaction = self.transaction, connection_hold_elapsed_us = hold_us,
            sample_every = SAMPLE_EVERY, slow = duration >= SLOW, deferred,
            "MongoDB diagnostic event");
    }
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod mongo_tests;
