//! Private, ignored six-case full-report measurement; exact scoped local DBs only.
//! Compare this identical fixture in two separately built source revisions.
use super::allocated::request;
use super::*;
use mongodb::event::{command::CommandEvent, EventHandler};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{atomic::Ordering, Mutex},
    time::Instant,
};

mod rss;
use rss::Rss;

const DAY: u64 = 86_400;
const MEMBERS: usize = 421;
const PERSONAL: usize = 221;
const CADENCE: u64 = 300;
const HISTORY_DAYS: u64 = 31;
const PRINCIPAL: &str = "hotfix-personal";

#[derive(Default)]
struct Commands {
    pending: Mutex<BTreeMap<i32, (String, Option<i64>)>>,
    totals: Mutex<BTreeMap<String, Value>>,
}
impl Commands {
    fn record(&self, key: String, limit: Option<i64>, duration: Duration, failed: bool) {
        let mut totals = self.totals.lock().unwrap();
        let row = totals.entry(key).or_insert_with(|| {
            json!({"count":0,"failed":0,
            "sum_us":0,"max_us":0,"max_time_ms_counts":{}})
        });
        row["count"] = json!(row["count"].as_u64().unwrap() + 1);
        row["failed"] = json!(row["failed"].as_u64().unwrap() + u64::from(failed));
        row["sum_us"] = json!(row["sum_us"].as_u64().unwrap() + duration.as_micros() as u64);
        row["max_us"] = json!(row["max_us"]
            .as_u64()
            .unwrap()
            .max(duration.as_micros() as u64));
        let limit = limit
            .map(|n| n.to_string())
            .unwrap_or_else(|| "absent".into());
        let count = row["max_time_ms_counts"][&limit].as_u64().unwrap_or(0) + 1;
        row["max_time_ms_counts"][limit] = json!(count);
    }
}
async fn observed(
    uri: &str,
    database: &str,
    capture_commands: bool,
) -> (Arc<ReportingStore>, Arc<Commands>) {
    let original = ReportingStore::connect(uri, database, &[19; 32])
        .await
        .unwrap();
    assert!(!original.view_policy.enabled);
    assert!(!original.view_policy.allocation_rollups_enabled);
    assert!(!original.view_policy.warm_enabled);
    let commands = Arc::new(Commands::default());
    // The ordinary store installs metadata-only driver tracing. A standard
    // command event handler eagerly decodes whole BSON replies in driver3.9.1,
    // after the command's recorded duration. Keep it out of the primary clock.
    if !capture_commands {
        return (original, commands);
    }
    // Explicit diagnostic control only; its timing includes eager reply decode.
    let capture = commands.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    configure_pool(&mut options, 4);
    // Retain the actual source revision's checkout/hold/circuit policy.
    super::super::budget::observe(&mut options, original.circuit.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Started(e) => {
            let collection = e
                .command
                .get_str("find")
                .ok()
                .or_else(|| e.command.get_str("collection").ok())
                .or_else(|| e.command.get_str("update").ok())
                .unwrap_or("other");
            let limit = e
                .command
                .get_i64("maxTimeMS")
                .ok()
                .or_else(|| e.command.get_i32("maxTimeMS").ok().map(i64::from));
            capture.pending.lock().unwrap().insert(
                e.request_id,
                (format!("{}:{collection}", e.command_name), limit),
            );
        }
        CommandEvent::Succeeded(e) => {
            if let Some((key, limit)) = capture.pending.lock().unwrap().remove(&e.request_id) {
                capture.record(key, limit, e.duration, false);
            }
        }
        CommandEvent::Failed(e) => {
            if let Some((key, limit)) = capture.pending.lock().unwrap().remove(&e.request_id) {
                capture.record(key, limit, e.duration, true);
            }
        }
        _ => {}
    }));
    let client = Client::with_options(options).unwrap();
    let mut store = (*original).clone();
    store.persistence = store.persistence.with_client(client);
    (Arc::new(store), commands)
}
fn save(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
async fn case(store: &Arc<ReportingStore>, admin: bool, days: u64, to: u64) -> Value {
    let prefix = if admin {
        "/api/v1/admin/usage/metrics/reports"
    } else {
        "/api/v1/usage/metrics/reports"
    };
    let began = Instant::now();
    let (status, pending) = request(
        store,
        "POST",
        prefix,
        PRINCIPAL,
        admin,
        json!({"from":to-days*DAY,"to":to}),
    )
    .await;
    let mut out = json!({"scope":if admin {"admin"} else {"personal"},"days":days,
        "samples":1,"cache":"cold_legacy_allocation_cache","admission_status":status,
        "admission_us":began.elapsed().as_micros(),"completed":false});
    if status != 202 {
        out["failure"] = json!({"stage":"admission","body":pending});
        return out;
    }
    let id = pending["report_id"].as_str().unwrap();
    let lease = match store.claim_report("report:0", "hotfix-matrix").await {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            out["failure"] = json!({"stage":"claim","error":"no work lease"});
            return out;
        }
        Err(error) => {
            out["failure"] = json!({"stage":"claim","error":format!("{error:?}")});
            return out;
        }
    };
    assert_eq!(id, lease.report.report_id);
    let generation = Instant::now();
    let result =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await;
    out["generation_us"] = json!(generation.elapsed().as_micros());
    let success = result.is_ok();
    if let Err(error) = &result {
        out["failure"] = json!({"stage":"generation","error":format!("{error:?}")});
    }
    let report = result.unwrap_or_else(|_| lease.report.clone());
    let finalize = Instant::now();
    let finish = store.finish_report(&lease, report, success).await;
    out["finalization_us"] = json!(finalize.elapsed().as_micros());
    out["finalized"] = json!(finish.is_ok());
    if let Err(error) = finish {
        out["finalization_error"] = json!(format!("{error:?}"));
    }
    let (descriptor_status, descriptor) = request(
        store,
        "GET",
        &format!("{prefix}/{id}"),
        PRINCIPAL,
        admin,
        Value::Null,
    )
    .await;
    out["descriptor_status"] = json!(descriptor_status);
    if descriptor_status != 200 || !success {
        out["descriptor_body"] = descriptor;
        out["failure_observation_us"] = json!(began.elapsed().as_micros());
        return out;
    }
    assert_eq!(descriptor["report_id"], id);
    assert_eq!(descriptor["report_complete"], true);
    assert_eq!(descriptor["status"], "ready");
    let mut path = format!("{prefix}/{id}/sandboxes?limit=50");
    let mut rows = Vec::new();
    for page_n in 0..20 {
        let (status, page) = request(store, "GET", &path, PRINCIPAL, admin, Value::Null).await;
        if page_n == 0 {
            out["first_page_us"] = json!(began.elapsed().as_micros());
        }
        if status != 200 {
            out["failure"] = json!({"stage":"page","page":page_n,"status":status,"body":page});
            return out;
        }
        assert_eq!(page["report_id"], id);
        assert_eq!(page["total_count"], if admin { MEMBERS } else { PERSONAL });
        rows.extend(page["items"].as_array().unwrap().iter().cloned());
        match page["next"].as_str() {
            Some(cursor) => {
                path = format!("{prefix}/{id}/sandboxes?limit=50&cursor={cursor}");
            }
            None => break,
        }
    }
    let count = if admin { MEMBERS } else { PERSONAL };
    assert_eq!(rows.len(), count);
    rows.sort_by(|a, b| a["sandbox_id"].as_str().cmp(&b["sandbox_id"].as_str()));
    assert!(rows
        .windows(2)
        .all(|w| w[0]["sandbox_id"] != w[1]["sandbox_id"]));
    let seconds = days * DAY;
    assert_eq!(descriptor["summary"]["sandbox_count"], count);
    assert_eq!(descriptor["summary"]["allocated_period"]["complete"], true);
    assert_eq!(
        descriptor["summary"]["allocated_period"]["uptime_seconds"]
            .as_f64()
            .unwrap(),
        count as f64 * seconds as f64
    );
    for row in &rows {
        assert_eq!(row["allocated_period"]["complete"], true);
        assert_eq!(
            row["allocated_period"]["uptime_seconds"].as_f64().unwrap(),
            seconds as f64
        );
        assert!(
            (row["allocated_period"]["cpu_core_hours"].as_f64().unwrap()
                - seconds as f64 * 2. / 3600.)
                .abs()
                < 1e-7
        );
    }
    out["all_pages_us"] = json!(began.elapsed().as_micros());
    out["completed"] = json!(true);
    out["observed_under_2s"] = json!(out["first_page_us"].as_u64().unwrap() <= 2_000_000);
    out["values"] = json!({"summary":descriptor["summary"],"rows":rows,
        "report_complete":descriptor["report_complete"],"effective_range":descriptor["effective_range"],
        "source_health":descriptor["source_health"],"warnings":descriptor["warnings"]});
    out
}

#[tokio::test]
#[ignore = "requires manager-bound local Mongo/build lane and exact full-report database grants"]
async fn native_usage_hotfix_full_report_six_case_matrix() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("private local endpoint required");
    let slot = std::env::var("CHRONO_USAGE_MATRIX_DB_SLOT").expect("01 or 02 required");
    assert!(matches!(slot.as_str(), "01" | "02"));
    let database = std::env::var(format!("CHRONO_TEST_ALLOCATION_DB_{slot}")).unwrap();
    assert_eq!(database, format!("metrics_2140_native05_{slot}"));
    let source = std::env::var("CHRONO_USAGE_MATRIX_SOURCE").expect("actual source SHA required");
    assert!(source.len() == 40 && source.bytes().all(|b| b.is_ascii_hexdigit()));
    let to: u64 = std::env::var("CHRONO_USAGE_MATRIX_TO")
        .expect("same frozen endpoint for both binaries")
        .parse()
        .unwrap();
    assert!(to < now() && to.is_multiple_of(CADENCE) && !to.is_multiple_of(DAY));
    let observer_mode =
        std::env::var("CHRONO_USAGE_MATRIX_OBSERVER").unwrap_or_else(|_| "metadata_only".into());
    assert!(matches!(
        observer_mode.as_str(),
        "metadata_only" | "eager_command"
    ));
    let capture_commands = observer_mode == "eager_command";
    let output =
        std::env::var("CHRONO_USAGE_MATRIX_OUTPUT").expect("private output directory required");
    let output = Path::new(&output);
    std::fs::create_dir_all(output).unwrap();
    let trace_installed = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok();
    let setup = Client::with_uri_str(&uri).await.unwrap();
    let db = setup.database(&database);
    // Sole disposable DB reset. No wildcard or other database is ever accessed.
    db.drop().await.unwrap();
    let began = Instant::now();
    let source_store = crate::store::sandbox_mongo::MongoSandboxStore::open(&setup, &database)
        .await
        .unwrap();
    drop(source_store);
    let startup_us = began.elapsed().as_micros();
    let startup_indexes: Vec<_> = db
        .collection::<Document>("sandbox_usage_events")
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let began = Instant::now();
    drop(
        crate::store::sandbox_mongo::MongoSandboxStore::open(&setup, &database)
            .await
            .unwrap(),
    );
    let second_startup_us = began.elapsed().as_micros();
    // Reuse this one fixture to exercise an upgrade over retained history.
    // Only the newly added index is removed from this exact disposable DB;
    // all baseline indexes remain installed during the identical seed.
    let interval_keys = doc! {"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1};
    let mut removed_empty_interval = false;
    for index in &startup_indexes {
        if index.keys == interval_keys {
            let name = index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref());
            assert_eq!(name, Some("sandbox_usage_events_interval"));
            db.collection::<Document>("sandbox_usage_events")
                .drop_index(name.unwrap())
                .await
                .unwrap();
            removed_empty_interval = true;
        }
    }
    let from = to - HISTORY_DAYS * DAY;
    let checkpoints = HISTORY_DAYS * DAY / CADENCE;
    let preparation = Instant::now();
    let mut states = Vec::new();
    for id in 0..MEMBERS {
        let mut s = state(id, from);
        s.owner = SubjectId(if id < PERSONAL {
            PRINCIPAL.into()
        } else {
            format!("other-{}", id % 12)
        });
        s.last_observed_at = to;
        s.source_seq = checkpoints;
        s.source_revision = checkpoints;
        s.revision = checkpoints;
        s.totals = s.allocation.usage(HISTORY_DAYS * DAY).unwrap();
        for first in (0..checkpoints).step_by(2000) {
            let docs = (first..(first + 2000).min(checkpoints))
                .map(|n| {
                    let mut event = super::history_batch::event(&s, n, from + n * CADENCE);
                    event.ended_at = event.started_at + CADENCE;
                    event.quantity = event.allocation.usage(CADENCE).unwrap();
                    crate::store::sandbox_usage::event_document(&event).unwrap()
                })
                .collect::<Vec<_>>();
            db.collection::<Document>("sandbox_usage_events")
                .insert_many(docs)
                .await
                .unwrap();
        }
        states.push(crate::store::sandbox_usage::state_document(&s).unwrap());
        if id % 50 == 0 {
            eprintln!("hotfix_matrix_seed members_done={} total={MEMBERS}", id + 1);
        }
    }
    db.collection::<Document>("sandbox_usage")
        .insert_many(states)
        .await
        .unwrap();
    let actual_events = db
        .collection::<Document>("sandbox_usage_events")
        .count_documents(doc! {})
        .await
        .unwrap();
    assert_eq!(actual_events, MEMBERS as u64 * checkpoints);
    let preparation_us = preparation.elapsed().as_micros();
    let populated_began = Instant::now();
    let populated_startup =
        crate::store::sandbox_mongo::MongoSandboxStore::open(&setup, &database).await;
    let populated_startup_us = populated_began.elapsed().as_micros();
    let populated_startup_record = json!({"source":source,"events":actual_events,
        "removed_empty_interval":removed_empty_interval,
        "populated_startup_us":populated_startup_us,"success":populated_startup.is_ok(),
        "error":populated_startup.as_ref().err().map(|error|format!("{error:?}")),
        "scope":"actual startup over retained fixture history; seed excluded"});
    save(
        &output.join("populated-startup.json"),
        &populated_startup_record,
    );
    drop(populated_startup.unwrap());
    let populated_indexes: Vec<_> = db
        .collection::<Document>("sandbox_usage_events")
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut result = json!({"source":source,"database":database,"fixture":{
        "members":MEMBERS,"personal_members":PERSONAL,"history_days":HISTORY_DAYS,
        "cadence_seconds":CADENCE,"events":actual_events,"from":from,"to":to,
        "rollups":false,"views":false,"warmer":false,"backfill":false,
        "observer_mode":observer_mode},
        "startup_us":startup_us,"second_startup_us":second_startup_us,
        "startup_indexes":startup_indexes,"preparation_us":preparation_us,
        "populated_startup":populated_startup_record,"populated_indexes":populated_indexes,
        "trace_subscriber_installed":trace_installed,"cases":[],
        "clock":"native authenticated route admission through generated/finalized descriptor and first page; no external authentication, network, poll scheduling, or browser render",
        "sampling":"N1 per case; no percentile claim; legacy cache cleared before each case; Mongo cache not reset"});
    save(&output.join("matrix.json"), &result);
    for (admin, days) in [
        (false, 1),
        (false, 7),
        (false, 30),
        (true, 1),
        (true, 7),
        (true, 30),
    ] {
        // Reset only disposable output/cache collections; retain source data/indexes.
        // A fresh reporting client gives independent scenarios; failed circuits are
        // retained in each case rather than cleared inside a failing report.
        for name in [JOBS, ROWS, CHARTS, CONTROL, "sandbox_usage_rollups"] {
            db.collection::<Document>(name)
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        let (store, commands) = observed(&uri, &database, capture_commands).await;
        let sample = Rss::start();
        let mut value = case(&store, admin, days, to).await;
        value["rss"] = sample.finish();
        value["command_capture_available"] = json!(capture_commands);
        value["commands"] = json!(*commands.totals.lock().unwrap());
        value["circuit_until"] = json!(store.circuit.load(Ordering::Relaxed));
        eprintln!("hotfix_matrix_case scope={} days={days} completed={} first_page_us={} generation_us={}",if admin {"admin"} else {"personal"},value["completed"],value["first_page_us"],value["generation_us"]);
        save(
            &output.join(format!(
                "{}-{days}d.json",
                if admin { "admin" } else { "personal" }
            )),
            &value,
        );
        result["cases"].as_array_mut().unwrap().push(value);
        save(&output.join("matrix.json"), &result);
        drop(store);
    }
    result["all_six_completed"] = json!(result["cases"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["completed"] == true));
    result["all_six_under_2s"] = json!(result["cases"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["observed_under_2s"] == true));
    save(&output.join("matrix.json"), &result);
    db.drop().await.unwrap();
    result["database_dropped"] = json!(true);
    save(&output.join("matrix.json"), &result);
    // The measurement test records failed cases, not a false functional PASS.
    assert_eq!(
        result["all_six_completed"], true,
        "all six outcomes saved before failure"
    );
}
