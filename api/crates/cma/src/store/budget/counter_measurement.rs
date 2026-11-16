//! Instrumented local comparison, with no wall-clock pass threshold.
use super::*;
use mongodb::event::{cmap::CmapEvent, command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
struct Trace {
    active: bool,
    commands: u64,
    checkouts: u64,
    request_bytes: usize,
    reply_bytes: usize,
    root_reads: u64,
    root_writes: u64,
    counter_reads: u64,
    counter_writes: u64,
}

fn classify(filter: &Document) -> Option<bool> {
    match filter.get("_id")? {
        mongodb::bson::Bson::String(key) if key.starts_with("platform:") => Some(false),
        mongodb::bson::Bson::String(key) if key.starts_with("platform-counter:") => Some(true),
        mongodb::bson::Bson::Document(ids) => ids
            .get_array("$in")
            .ok()?
            .first()?
            .as_str()
            .filter(|key| key.starts_with("platform-counter:"))
            .map(|_| true),
        _ => None,
    }
}

fn read_kinds(filter: &Document) -> (bool, bool) {
    let keys: Vec<&str> = match filter.get("_id") {
        Some(mongodb::bson::Bson::String(key)) => vec![key.as_str()],
        Some(mongodb::bson::Bson::Document(ids)) => ids
            .get_array("$in")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
            .collect(),
        _ => Vec::new(),
    };
    (
        keys.iter().any(|key| key.starts_with("platform:")),
        keys.iter().any(|key| key.starts_with("platform-counter:")),
    )
}

async fn client(trace: &Arc<Mutex<Trace>>) -> mongodb::Client {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let command_trace = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut trace = command_trace.lock().unwrap();
        if !trace.active {
            return;
        }
        match event {
            CommandEvent::Started(event) => {
                trace.commands += 1;
                trace.request_bytes += mongodb::bson::serialize_to_vec(&event.command)
                    .unwrap()
                    .len();
                if event.command.get_str("find").ok() == Some(COLLECTION) {
                    if let Ok(filter) = event.command.get_document("filter") {
                        let (root, counter) = read_kinds(filter);
                        trace.root_reads += u64::from(root);
                        trace.counter_reads += u64::from(counter);
                    }
                }
                if event.command.get_str("update").ok() == Some(COLLECTION) {
                    for update in event.command.get_array("updates").unwrap() {
                        match update
                            .as_document()
                            .and_then(|update| update.get_document("q").ok())
                            .and_then(classify)
                        {
                            Some(false) => trace.root_writes += 1,
                            Some(true) => trace.counter_writes += 1,
                            None => {}
                        }
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                trace.reply_bytes += mongodb::bson::serialize_to_vec(&event.reply).unwrap().len();
            }
            CommandEvent::Failed(_) => {}
            _ => {}
        }
    }));
    let checkout_trace = trace.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            let mut trace = checkout_trace.lock().unwrap();
            if trace.active {
                trace.checkouts += 1;
            }
        }
    }));
    mongodb::Client::with_options(options).unwrap()
}

fn partitions() -> Vec<String> {
    let mut owners = std::collections::BTreeMap::new();
    for index in 0..1000 {
        let owner = format!("counter-measurement-{index}");
        owners
            .entry(counters::partition(&layer2_core::SubjectId::new(&owner)))
            .or_insert(owner);
        if owners.len() == usize::from(counters::COUNT - 1) {
            break;
        }
    }
    assert_eq!(owners.len(), usize::from(counters::COUNT - 1));
    owners.into_values().collect()
}

async fn begin(trace: &Arc<Mutex<Trace>>, db: &mongodb::Database) {
    db.run_command(doc! {"profile": 0_i32}).await.unwrap();
    if db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| name == "system.profile")
    {
        db.collection::<Document>("system.profile")
            .drop()
            .await
            .unwrap();
    }
    db.create_collection("system.profile")
        .capped(true)
        .size(10 * 1024 * 1024)
        .await
        .unwrap();
    db.run_command(doc! {"profile": 2_i32}).await.unwrap();
    *trace.lock().unwrap() = Trace {
        active: true,
        ..Default::default()
    };
}

async fn report(
    trace: &Arc<Mutex<Trace>>,
    db: &mongodb::Database,
    enabled: bool,
    concurrent: bool,
    phase: &str,
    mut latencies: Vec<u128>,
    elapsed: u128,
) {
    trace.lock().unwrap().active = false;
    db.run_command(doc! {"profile": 0_i32}).await.unwrap();
    let scans = db
        .collection::<Document>("system.profile")
        .count_documents(doc! {
            "ns": format!("{}.{}", db.name(), COLLECTION), "planSummary": {"$regex": "COLLSCAN"},
        })
        .await
        .unwrap();
    assert_eq!(
        scans, 0,
        "counter queries must stay on the primary-key index"
    );
    latencies.sort_unstable();
    let p50 = latencies[(latencies.len() - 1) / 2];
    let p95 = latencies[(latencies.len() * 95).div_ceil(100) - 1];
    let trace = trace.lock().unwrap();
    println!("counter_measurement mode={} concurrent={} phase={} n={} commands={} checkouts={} request_bytes={} reply_bytes={} root_reads={} root_writes={} counter_reads={} counter_writes={} full_scans={} p50_us={} p95_us={} elapsed_us={}",
        if enabled { "distributed" } else { "original" }, concurrent, phase, latencies.len(),
        trace.commands, trace.checkouts, trace.request_bytes, trace.reply_bytes,
        trace.root_reads, trace.root_writes, trace.counter_reads, trace.counter_writes,
        scans, p50, p95, elapsed);
    if enabled {
        assert_eq!(trace.root_writes, 0);
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_measure_sequential_and_concurrent_accounting() {
    for enabled in [false, true] {
        let trace = Arc::new(Mutex::new(Trace::default()));
        let client = client(&trace).await;
        let (client, name, store, _) = fixture_with_client(client, 1_000_000, enabled).await;
        let db = client.database(&name);
        let seed = reserve(&store, "seed", 1).await.unwrap();
        change(
            &store,
            seed,
            Change {
                released: 1,
                ..Default::default()
            },
        )
        .await;
        let owners = partitions();
        for concurrent in [false, true] {
            let mut holds = Vec::new();
            let mut reserve_latencies = Vec::new();
            begin(&trace, &db).await;
            let elapsed = Instant::now();
            for _ in 0..2 {
                if concurrent {
                    let tasks = owners.iter().map(|owner| async {
                        let started = Instant::now();
                        let charge = reserve(&store, owner, 100).await.unwrap();
                        (charge, started.elapsed().as_micros())
                    });
                    for (charge, latency) in futures_util::future::join_all(tasks).await {
                        holds.push(charge);
                        reserve_latencies.push(latency);
                    }
                } else {
                    for owner in &owners {
                        let started = Instant::now();
                        holds.push(reserve(&store, owner, 100).await.unwrap());
                        reserve_latencies.push(started.elapsed().as_micros());
                    }
                }
            }
            report(
                &trace,
                &db,
                enabled,
                concurrent,
                "reserve",
                reserve_latencies,
                elapsed.elapsed().as_micros(),
            )
            .await;
            begin(&trace, &db).await;
            let elapsed = Instant::now();
            let mut settle_latencies = Vec::new();
            // One outstanding change per partition in each concurrent wave.
            for wave in holds.chunks(owners.len()) {
                if concurrent {
                    let tasks = wave.iter().map(|charge| async {
                        let started = Instant::now();
                        change(
                            &store,
                            *charge,
                            Change {
                                released: 100,
                                debit: 40,
                                ..Default::default()
                            },
                        )
                        .await;
                        started.elapsed().as_micros()
                    });
                    settle_latencies.extend(futures_util::future::join_all(tasks).await);
                } else {
                    for charge in wave {
                        let started = Instant::now();
                        change(
                            &store,
                            *charge,
                            Change {
                                released: 100,
                                debit: 40,
                                ..Default::default()
                            },
                        )
                        .await;
                        settle_latencies.push(started.elapsed().as_micros());
                    }
                }
            }
            report(
                &trace,
                &db,
                enabled,
                concurrent,
                "settle",
                settle_latencies,
                elapsed.elapsed().as_micros(),
            )
            .await;
        }
        client.database(&name).drop().await.unwrap();
    }
}
