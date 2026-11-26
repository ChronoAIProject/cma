use super::*;
use mongodb::{
    event::{cmap::CmapEvent, command::CommandEvent, EventHandler},
    options::ClientOptions,
    Database,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering::SeqCst},
        Arc, Mutex,
    },
};

#[derive(Default, Serialize)]
pub(super) struct Wire {
    pub commands: BTreeMap<String, u64>,
    pub failed_commands: u64,
    pub reply_bytes: u64,
    pub checkouts: u64,
    pub checkout_wait_us: u128,
}

#[derive(Default)]
pub(super) struct Trace {
    enabled: AtomicBool,
    wire: Mutex<Wire>,
}

#[derive(Serialize)]
pub(super) struct Observation {
    pub operation: &'static str,
    pub case: &'static str,
    pub round: usize,
    pub before: Shape,
    pub after: Shape,
    pub rollover: bool,
    pub outcome: String,
    pub returned_rows: Option<usize>,
    pub returned_json_bytes: Option<usize>,
    pub consumed_source: [u64; 4],
    pub wire: Wire,
    pub docs_examined: i64,
    pub keys_examined: i64,
    pub elapsed_us: u128,
    pub docs_budget: Option<i64>,
    pub within_docs_budget: Option<bool>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(super) struct Shape {
    pub active_count: i64,
    pub active_bytes: i64,
    pub active_ordinal: i64,
    pub last_seq: i64,
    pub revision: i64,
}

async fn shape(db: &Database, fixture: &fixture::Fixture) -> Shape {
    let head = db
        .collection::<Head>(HEADS)
        .find_one(doc! {"_id":fixture.agent.id.as_str()})
        .await
        .unwrap()
        .expect("persisted active head");
    Shape {
        active_count: head.active_count,
        active_bytes: head.active_bytes,
        active_ordinal: head.active_ordinal,
        last_seq: head.last_seq,
        revision: head.revision,
    }
}

pub(super) async fn client() -> (Client, Arc<Trace>) {
    let trace = Arc::new(Trace::default());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let seen = trace.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if !seen.enabled.load(SeqCst) {
            return;
        }
        if let CmapEvent::ConnectionCheckedOut(event) = event {
            let mut wire = seen.wire.lock().unwrap();
            wire.checkouts += 1;
            wire.checkout_wait_us += event.duration.as_micros();
        }
    }));
    let seen = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if !seen.enabled.load(SeqCst) {
            return;
        }
        let mut wire = seen.wire.lock().unwrap();
        match event {
            CommandEvent::Started(event) => {
                *wire.commands.entry(event.command_name).or_default() += 1
            }
            CommandEvent::Succeeded(event) => {
                wire.reply_bytes += bson::serialize_to_vec(&event.reply).unwrap().len() as u64;
            }
            CommandEvent::Failed(_) => wire.failed_commands += 1,
            _ => {}
        }
    }));
    (Client::with_options(options).unwrap(), trace)
}

pub(super) trait Returned: Serialize {
    fn returned_rows(&self) -> usize;
}

impl Returned for Vec<AgentLogEntry> {
    fn returned_rows(&self) -> usize {
        self.len()
    }
}

impl Returned for AgentLogEntry {
    fn returned_rows(&self) -> usize {
        1
    }
}

pub(super) async fn sample<T: Returned>(
    db: &Database,
    trace: &Trace,
    case: fixture::Case,
    fixture: &fixture::Fixture,
    round: usize,
    operation: &'static str,
    future: impl Future<Output = Result<T, SandboxStoreError>>,
) -> (Result<T, SandboxStoreError>, Observation) {
    db.run_command(doc! {"profile":0}).await.unwrap();
    let before = shape(db, fixture).await;
    db.collection::<Document>("system.profile")
        .drop()
        .await
        .unwrap();
    db.create_collection("system.profile")
        .capped(true)
        .size(16 * 1024 * 1024)
        .await
        .unwrap();
    db.run_command(doc! {"profile":2,"slowms":0}).await.unwrap();
    *trace.wire.lock().unwrap() = Wire::default();
    trace.enabled.store(true, SeqCst);
    let started = std::time::Instant::now();
    let (result, consumed_source) = crate::store::mongo_read_telemetry::isolated_test(async {
        let result = future.await;
        (
            result,
            crate::store::mongo_read_telemetry::snapshot("agent_log"),
        )
    })
    .await;
    let elapsed_us = started.elapsed().as_micros();
    trace.enabled.store(false, SeqCst);
    let wire = std::mem::take(&mut *trace.wire.lock().unwrap());
    db.run_command(doc! {"profile":0}).await.unwrap();
    let after = shape(db, fixture).await;
    let profile = db
        .collection::<Document>("system.profile")
        .find(doc! {"ns":{"$regex":format!("^{}\\.",db.name())}})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert!(
        !profile.is_empty(),
        "actual operation must reach Mongo profiler"
    );
    let total = |key| {
        profile
            .iter()
            .map(|entry| {
                entry
                    .get_i64(key)
                    .or_else(|_| entry.get_i32(key).map(i64::from))
                    .unwrap_or(0)
            })
            .sum::<i64>()
    };
    let docs_examined = total("docsExamined");
    let docs_budget = match operation {
        "page" => Some(250),
        "append" => Some(100),
        _ => None,
    };
    let observation = Observation {
        operation,
        case: case.name,
        round,
        rollover: before.active_ordinal != after.active_ordinal,
        before,
        after,
        outcome: result
            .as_ref()
            .map(|_| "success".to_owned())
            .unwrap_or_else(|error| format!("{error:?}")),
        returned_rows: result.as_ref().ok().map(Returned::returned_rows),
        returned_json_bytes: result
            .as_ref()
            .ok()
            .map(|rows| serde_json::to_vec(rows).unwrap().len()),
        consumed_source,
        wire,
        docs_examined,
        keys_examined: total("keysExamined"),
        elapsed_us,
        docs_budget,
        within_docs_budget: docs_budget.map(|budget| docs_examined <= budget),
    };
    eprintln!(
        "active_tail_observation {}",
        serde_json::to_string(&observation).unwrap()
    );
    (result, observation)
}
