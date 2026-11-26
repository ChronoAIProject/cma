//! Run only against a fresh disposable replica set, never production.
use super::*;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    event::EventHandler,
    options::ClientOptions,
    Client,
};

#[derive(Default)]
struct Counts {
    checkouts: u64,
    holds: u64,
    hold_us: u64,
    open: HashMap<u64, Instant>,
}
async fn client(uri: &str, enabled: bool) -> (Client, Arc<Mutex<Counts>>) {
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.max_pool_size = Some(30);
    options.tracing = Some(super::metadata_hook::disabled());
    let diagnostics = enabled.then(Diagnostics::default);
    if let Some(commands) = diagnostics.clone() {
        drop(super::periodic::start(&commands));
        super::metadata_hook::install(&mut options, Some(commands));
    }
    observe_checkouts(options, diagnostics)
}

fn observe_checkouts(
    mut options: ClientOptions,
    diagnostics: Option<Diagnostics>,
) -> (Client, Arc<Mutex<Counts>>) {
    let counts = Arc::new(Mutex::new(Counts::default()));
    let observed = counts.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if let Some(diagnostics) = &diagnostics {
            diagnostics.cmap(&event);
        }
        let mut counts = observed.lock().unwrap();
        match event {
            CmapEvent::ConnectionCheckedOut(e) => {
                counts.checkouts += 1;
                counts
                    .open
                    .insert(connection_key(&e.address, e.connection_id), Instant::now());
            }
            CmapEvent::ConnectionCheckedIn(e) => {
                if let Some(start) = counts
                    .open
                    .remove(&connection_key(&e.address, e.connection_id))
                {
                    counts.holds += 1;
                    counts.hold_us += micros(start.elapsed());
                }
            }
            _ => {}
        }
    }));
    (Client::with_options(options).unwrap(), counts)
}
async fn drain(counts: &Mutex<Counts>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !counts.lock().unwrap().open.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pool check-ins must finish before resetting measurement");
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn real_mongo_monitoring_privacy_and_overhead() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let (off, off_counts) = client(&uri, false).await;
    let (on, on_counts) = client(&uri, true).await;
    let name = format!("diagnostic_test_{}", uuid::Uuid::new_v4().simple());
    let database = off.database(&name);
    let payload = "x".repeat(1024);
    let documents: Vec<_> = (0..10001).map(|i| doc! {"_id":i,"entry":{"kind":"output","seq":i,"payload":&payload,"meta":{"a":1,"b":2,"c":[1,2,3]}}}).collect();
    database
        .collection::<Document>("agents")
        .insert_many(documents)
        .await
        .unwrap();
    let output = super::tests::Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let context = DiagnosticContext {
        route: "GET /api/v1/sandboxes/{id}".into(),
        request_hash: "b".repeat(64).into(),
        worker: "request",
        phase: "hierarchy",
    };
    // Warm both pools before an interleaved ABBA comparison. Input and output
    // are identical; monitor installation is the only treatment.
    for client in [&off, &on] {
        client
            .database(&name)
            .run_command(doc! {"ping":1})
            .await
            .unwrap();
    }
    for size in [25_i64, 200, 10001] {
        let mut elapsed = [Vec::new(), Vec::new()];
        let mut holds = [0_u64; 2];
        let mut checkouts = [0_u64; 2];
        for mode in [0, 1, 1, 0] {
            let (client, counts) = if mode == 0 {
                (&off, &off_counts)
            } else {
                (&on, &on_counts)
            };
            let collection = client.database(&name).collection::<Document>("agents");
            drain(counts).await;
            *counts.lock().unwrap() = Counts::default();
            for _ in 0..20 {
                let started = Instant::now();
                let rows: Vec<Document> = context
                    .clone()
                    .scope(async {
                        collection
                            .find(doc! {"_id":{"$lt":size}})
                            .batch_size(2048)
                            .await
                            .unwrap()
                            .try_collect()
                            .await
                            .unwrap()
                    })
                    .await;
                elapsed[mode].push(micros(started.elapsed()));
                assert_eq!(rows.len(), size as usize);
            }
            drain(counts).await;
            let counts = counts.lock().unwrap();
            holds[mode] += counts.hold_us;
            checkouts[mode] += counts.checkouts;
        }
        for values in &mut elapsed {
            values.sort_unstable();
        }
        let means = elapsed
            .each_ref()
            .map(|v| v.iter().sum::<u64>() / v.len() as u64);
        let p50 = elapsed.each_ref().map(|v| v[v.len() / 2]);
        eprintln!("MONITOR_ABBA docs={size} n_per_mode=40 off_mean_us={} on_mean_us={} off_p50_us={} on_p50_us={} off_hold_us={} on_hold_us={} off_checkouts={} on_checkouts={}",means[0],means[1],p50[0],p50[1],holds[0],holds[1],checkouts[0],checkouts[1]);
        assert_eq!(checkouts[0], checkouts[1]);
        // Bound absolute diagnostic cost in this native debug fixture; retain
        // ratios for reviewer judgement, do not hide relative decode overhead.
        assert!(means[1] < means[0].saturating_mul(2).saturating_add(5_000));
    }
    context
        .scope(async {
            let mut session = on.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            on.database(&name)
                .collection::<Document>("agents")
                .find_one(doc! {"_id":0})
                .session(&mut session)
                .await
                .unwrap();
            session.abort_transaction().await.unwrap();
            assert!(on
                .database(&name)
                .run_command(doc! {"unknown_private_command":1})
                .await
                .is_err());
        })
        .await;
    drain(&on_counts).await;
    super::output::flush();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    for sensitive in [&name, &payload, "unknown_private_command", "127.0.0.1"] {
        assert!(!logs.contains(sensitive));
    }
    assert!(logs.contains("event=\"failed\""));
    // Check-in runs in the driver's pool task: prove its retained labels and
    // physical-hold hash still join to the originating command after that hop.
    let request_hash = format!("request_hash={}", "b".repeat(64));
    let returned: Vec<_> = logs
        .lines()
        .filter(|line| {
            line.contains("event=\"checked_in\"")
                && line.contains("route=\"GET /api/v1/sandboxes/{id}\"")
                && line.contains("phase=\"hierarchy\"")
        })
        .collect();
    assert!(
        !returned.is_empty(),
        "physical holds must retain request context"
    );
    // Hold time includes decoding/callback work. A slow hold can contain a fast,
    // unsampled command, so the first hold need not have a logged completion.
    let joined = returned
        .iter()
        .filter(|returned| {
            let hold_hash = returned
                .split_whitespace()
                .find_map(|field| field.strip_prefix("hold_hash="))
                .unwrap();
            assert_eq!(hold_hash.len(), 64);
            assert!(returned.contains(&request_hash));
            logs.lines().any(|line| {
                // getMore exposes an input cursor ID, not a positive result
                // marker. Its completion is explicitly unknown, but must still
                // join to the same physical hold and originating request.
                (line.contains("event=\"succeeded\"")
                    || (line.contains("event=\"completed\"")
                        && line.contains("outcome_known=false")))
                    && line.contains(&format!("hold_hash={hold_hash}"))
                    && line.contains(&request_hash)
                    && line.contains("phase=\"hierarchy\"")
                    && line.contains("docs_returned_known=false")
                    && line.contains("transaction_known=false")
            })
        })
        .count();
    eprintln!(
        "MONITOR_CORRELATION holds={} joined={joined}",
        returned.len()
    );
    assert!(joined > 0, "a sampled command and physical hold must join");
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn application_modes_count_command_attempts_independently_of_checkouts() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let (unmonitored, _) = client(&uri, false).await;
    let name = format!("diagnostic_counts_{}", uuid::Uuid::new_v4().simple());
    let fixture = unmonitored.database(&name);
    fixture
        .collection::<Document>("agents")
        .insert_many((0..5).map(|id| doc! {"_id": id}))
        .await
        .unwrap();

    let mut physical = Vec::new();
    for (mode, configured, expected_observed) in [
        ("driver_off", None, 0),
        ("application_off", Some("false"), 7),
        ("application_on", Some("true"), 7),
    ] {
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.max_pool_size = Some(30);
        let diagnostics = if let Some(value) = configured {
            let diagnostics = install_configured(&mut options, "general", Some(value));
            assert_eq!(diagnostics.is_some(), value == "true");
            diagnostics
        } else {
            options.tracing = Some(super::metadata_hook::disabled());
            None
        };
        assert!(options.command_event_handler.is_none());
        let (client, counts) = observe_checkouts(options, diagnostics);
        let db = client.database(&name);
        db.run_command(doc! {"ping": 1}).await.unwrap();
        drain(&counts).await;
        *counts.lock().unwrap() = Counts::default();

        let (_, attempts) = crate::telemetry::agent_output::count_commands(async {
            let agents = db.collection::<Document>("agents");
            let rows: Vec<Document> = agents
                .find(doc! {})
                .sort(doc! {"_id": 1})
                .batch_size(2)
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            let ids: Vec<_> = rows.iter().map(|row| row.get_i32("_id").unwrap()).collect();
            assert_eq!(ids, [0, 1, 2, 3, 4]);

            let mut session = client.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            for id in [0, 4] {
                let row = agents
                    .find_one(doc! {"_id": id})
                    .session(&mut session)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(row.get_i32("_id").unwrap(), id);
            }
            session.abort_transaction().await.unwrap();
            assert!(db
                .run_command(doc! {"unknown_private_command": 1})
                .await
                .is_err());
        })
        .await;
        drain(&counts).await;
        let counts = counts.lock().unwrap();
        assert_eq!(attempts, expected_observed, "{mode}");
        assert!(counts.checkouts > 0);
        assert_eq!(counts.checkouts, counts.holds);
        physical.push(counts.holds);
        eprintln!(
            "MONITOR_WORK mode={mode} observed_attempts={attempts} checkouts={} physical_hold_us={}",
            counts.checkouts, counts.hold_us
        );
    }
    assert!(physical.windows(2).all(|pair| pair[0] == pair[1]));
    fixture.drop().await.unwrap();
}
