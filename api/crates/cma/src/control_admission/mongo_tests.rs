//! Isolated HTTP policy load with real quota/dependency I/O and delayed work
//! after 202. Real lifecycle finality has separate durable-control regressions.
use super::*;
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware,
    routing::{delete, get, post},
    Router,
};
use layer1_public::AuthenticatedSubject;
use mongodb::{
    bson::doc,
    event::{cmap::CmapEvent, EventHandler},
    options::ClientOptions,
    Client, Database,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

#[derive(Default)]
struct Pool {
    held: BTreeMap<u32, Instant>,
    peak: usize,
    connection_seconds: f64,
}

async fn settle_pool(pool: &Mutex<Pool>) {
    // Pool check-in runs after command completion in the driver's pool worker.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pool.lock().unwrap().held.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("all fixture connections must return to the pool");
}

struct Ingress {
    database: Database,
    accepted: [AtomicUsize; 2],
    deferred: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

async fn authenticated(
    mut request: Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let caller = request
        .headers()
        .get("x-fixture-subject")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(layer2_core::SubjectId::new(caller)));
    next.run(request).await
}

async fn accepted(State(state): State<Arc<Ingress>>, request: Request) -> StatusCode {
    let (class, _) = classify(&request).unwrap();
    state.database.run_command(doc! {"ping":1}).await.unwrap();
    state.accepted[class.index()].fetch_add(1, Ordering::SeqCst);
    let database = state.database.clone();
    state
        .deferred
        .lock()
        .unwrap()
        .push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            database.run_command(doc! {"ping":1}).await.unwrap();
        }));
    StatusCode::ACCEPTED
}

async fn read(State(state): State<Arc<Ingress>>) -> StatusCode {
    state.database.run_command(doc! {"ping":1}).await.unwrap();
    StatusCode::OK
}

struct Replica {
    address: String,
    state: Arc<Ingress>,
    pool: Arc<Mutex<Pool>>,
    policy: Arc<Policy>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Replica {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn replica(uri: &str, name: &str) -> Replica {
    let pool = Arc::new(Mutex::new(Pool::default()));
    let observed = pool.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.max_pool_size = Some(30);
    options.app_name = Some(name.into());
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        let mut pool = observed.lock().unwrap();
        match event {
            CmapEvent::ConnectionCheckedOut(event) => {
                assert!(pool
                    .held
                    .insert(event.connection_id, Instant::now())
                    .is_none());
                pool.peak = pool.peak.max(pool.held.len());
            }
            CmapEvent::ConnectionCheckedIn(event) => {
                let start = pool
                    .held
                    .remove(&event.connection_id)
                    .expect("matching checkout");
                pool.connection_seconds += start.elapsed().as_secs_f64();
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let database = client.database(name);
    let rates = Arc::new(
        crate::store::request_limits::MongoRequestLimits::open(&database)
            .await
            .unwrap(),
    );
    let admission = Arc::new(Policy::new(
        ControlAdmissionConfig {
            enabled: true,
            ..Default::default()
        },
        rates,
        Arc::new(SystemClock),
    ));
    let state = Arc::new(Ingress {
        database,
        accepted: Default::default(),
        deferred: Mutex::default(),
    });
    let router = Router::new()
        .route("/api/v1/agents/{id}", delete(accepted).get(read))
        .route("/api/v1/agents/{id}/stop", post(accepted))
        .route("/sentinel", get(read))
        .with_state(state.clone());
    let router = routes(router, Some(admission.clone())).layer(middleware::from_fn(authenticated));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    settle_pool(&pool).await;
    {
        let mut measured = pool.lock().unwrap();
        measured.peak = measured.held.len();
        measured.connection_seconds = 0.0;
    }
    Replica {
        address,
        state,
        pool,
        policy: admission,
        server,
    }
}

mod delayed;
mod generic;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn alternating_ingress_two_and_four_times_load_preserves_read_and_stop_sentinels() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for multiplier in [2, 4] {
        let name = format!("cma_control_load_{}", uuid::Uuid::new_v4().simple());
        let replicas = [replica(&uri, &name).await, replica(&uri, &name).await];
        let mut accepted = [0; 2];
        let mut refused = [0; 2];
        let start = tokio::time::Instant::now();
        for tick in 0..8 {
            // Slightly exceed the 2x/4x candidate load so a clock boundary does
            // not turn this into a sub-threshold traffic test.
            tokio::time::sleep_until(start + Duration::from_millis(tick * 2400)).await;
            for (class, period, method, path) in [
                (
                    Class::Stop,
                    if multiplier == 2 { 2 } else { 1 },
                    Method::POST,
                    "/api/v1/agents/a/stop",
                ),
                (
                    Class::Delete,
                    if multiplier == 2 { 4 } else { 2 },
                    Method::DELETE,
                    "/api/v1/agents/a",
                ),
            ] {
                if tick % period != 0 {
                    continue;
                }
                let replica = &replicas[((tick / period) % 2) as usize];
                let response = client
                    .request(method, format!("{}{path}", replica.address))
                    .header("x-fixture-subject", "noisy")
                    .header("idempotency-key", "same-operation")
                    .send()
                    .await
                    .unwrap();
                match response.status() {
                    StatusCode::ACCEPTED => accepted[class.index()] += 1,
                    StatusCode::TOO_MANY_REQUESTS => {
                        refused[class.index()] += 1;
                        let delay: u64 = response.headers()["retry-after"]
                            .to_str()
                            .unwrap()
                            .parse()
                            .unwrap();
                        assert!((1..=60).contains(&delay));
                    }
                    other => panic!("unexpected load response: {other}"),
                }
            }
            let replica = &replicas[(tick % 2) as usize];
            assert_eq!(
                client
                    .get(format!("{}/sentinel", replica.address))
                    .header("x-fixture-subject", "reader")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
            if tick % 5 == 0 {
                assert_eq!(
                    client
                        .post(format!("{}/api/v1/agents/sentinel/stop", replica.address))
                        .header("x-fixture-subject", "sentinel")
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    202
                );
            }
        }
        assert_eq!(accepted, [2, 1]);
        assert_eq!(refused, if multiplier == 2 { [2, 1] } else { [6, 3] });
        for (index, replica) in replicas.iter().enumerate() {
            let tasks = std::mem::take(&mut *replica.state.deferred.lock().unwrap());
            for task in tasks {
                task.await.unwrap();
            }
            settle_pool(&replica.pool).await;
            let pool = replica.pool.lock().unwrap();
            assert!(pool.held.is_empty());
            assert!(pool.peak > 0 && pool.peak <= 30);
            println!(
                "CONTROL_LOAD {}",
                serde_json::json!({"multiplier":multiplier,"pool":index,
                "http_accepted":accepted,"http_refused":refused,"peak_checkout":pool.peak,
                "connection_seconds":pool.connection_seconds,"post_202_joined":true,
                "guarantee":"HTTP containment only; synthetic dependency/deferred cost, excludes real worker cost"})
            );
        }
        admin.database(&name).drop().await.unwrap();
    }
}
