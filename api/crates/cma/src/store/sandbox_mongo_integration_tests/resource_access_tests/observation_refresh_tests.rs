use super::*;
use crate::store::agent_observation::COLLECTION;
use layer2_core::sandbox::{
    agent_observation::{observation_scope, AgentObservationStore as _},
    AgentReadError,
};

#[derive(Clone, Default)]
struct ObservationLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for ObservationLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn stream_end(mut response: reqwest::Response) -> Result<&'static str, reqwest::Error> {
    loop {
        match response.chunk().await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok("eof"),
            // The forwarded socket may surface the same termination as a
            // truncated chunked body. An HTTP client deadline isn't closure.
            Err(error) if !error.is_timeout() && (error.is_body() || error.is_decode()) => {
                return Ok("truncated_body");
            }
            Err(error) => return Err(error),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the disposable Mongo replica-set runner with failpoints"]
async fn observation_delayed_mongo_refresh_closes_streams_and_revokes_custody() {
    let logs = ObservationLog::default();
    let writer = logs.clone();
    // This dispatcher also covers the worker spawned on the test's executor.
    let _capture = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_env_filter("cma.agent_observation=warn")
            .with_writer(move || writer.clone())
            .finish(),
    );
    let mut f = Fixture::semantic(100).await;
    agents(&mut f, false, false).await;
    let replicas = budget_servers(&f).await;
    let destination = replicas[0]
        .observation
        .destination(&f.agent.id, &f.agent.created_at)
        .await
        .unwrap();
    let owner = replicas
        .iter()
        .position(|replica| replica.address == destination.member.relay_address)
        .unwrap();
    let peer = 1 - owner;
    let custody = destination.authority;
    let credential = format!("observer:{}", f.agent.owner);
    let mut fence = replicas[owner]
        .reads
        .authorize_event_stream(
            &fixture::context(f.agent.owner.clone()),
            Some(&credential),
            &f.agent.id,
        )
        .await
        .unwrap();
    // The first subscriber pins the same local authority the HTTP streams use.
    let witness = observation_scope(custody.clone(), async {
        replicas[owner]
            .reads
            .observe_event_stream(&mut fence)
            .unwrap()
            .unwrap()
    })
    .await;
    replicas[owner]
        .reads
        .validate_event_stream(&fence)
        .await
        .unwrap();

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let identity = Sha256::digest(
        serde_json::to_vec(&("cma.agent.cursor.v1", &f.agent.id, &f.agent.created_at)).unwrap(),
    );
    let (local, forwarded) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(
            client
                .get(format!(
                    "{}/api/v1/agui/events?thread_id={}&starting_after=100&follow=true",
                    replicas[owner].address, f.agent.id
                ))
                .bearer_auth(&credential)
                .send(),
            client
                .get(format!(
                    "{}/api/v2/agents/{}/events?after_cursor=ag1.{identity:x}.100&follow=true",
                    replicas[peer].address, f.agent.id
                ))
                .bearer_auth(&credential)
                .send(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    for response in [&local, &forwarded] {
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
    }
    let mut occupied = [99, 99];
    occupied[owner] = 98;
    observation_capacity::available(&replicas, &occupied).await;
    assert!(witness.current() && custody.current());

    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap()
        .database("admin");
    let configured = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["update"], "namespace": format!("{}.{COLLECTION}", f.name),
                      "blockConnection": true, "blockTimeMS": 2500_i32 }
        })
        .await
        .unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            admin.run_command(doc! { "waitForFailPoint": "failCommand",
            "timesEntered": count + 1, "maxTimeMS": 7000_i32 }),
            stream_end(local),
            stream_end(forwarded),
        )
    })
    .await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let (entered, local_end, forwarded_end) =
        ended.expect("the injected refresh should terminate both existing observers");
    entered.expect("Mongo should confirm entry into the scoped update failpoint");
    let local_end = local_end.expect("the local observer should close its stream");
    let forwarded_end = forwarded_end.expect("the forwarded observer should close its stream");

    assert!(!witness.current() && !custody.current());
    // Durable placement can still be valid after a timed-out acknowledgement.
    // The old subscriber must fail its release fence even in that case.
    assert!(replicas[owner]
        .observation
        .fresh(std::slice::from_ref(&custody.token))
        .await
        .unwrap()[0]
        .is_ok());
    assert!(matches!(
        replicas[owner].reads.validate_event_stream(&fence).await,
        Err(AgentReadError::Unavailable(_))
    ));
    observation_capacity::available(&replicas, &[100, 100]).await;
    let captured = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(captured.contains("mongo_refresh_timeout"));
    assert!(captured.lines().any(|line| {
        line.contains("Revoking Agent observation cohort") && line.contains("store_error")
    }));
    eprintln!("OBSERVATION_REFRESH_REVOCATION reason=mongo_refresh_timeout local={local_end} forwarded={forwarded_end} release=denied");
    drop(witness);
    drop(replicas);
    f.close().await;
}
