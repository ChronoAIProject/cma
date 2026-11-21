//! Real process loss at the shared renewal/store boundary, without clock edits.
//! The composed-worker tests cover adapters; this is not an AM deployment test.

use super::*;
use layer2_core::sandbox::store::{
    ClaimedWork, WorkAuthority, WorkLeaseGrant, WORK_LEASE_SECONDS, WORK_RENEW_SECONDS,
};
use std::process::{Child, Command, Stdio};
use tokio::time::{sleep, timeout, Instant};

mod claims;
use claims::{captured, complete, renew};

const PROCESS_DATABASE: &str = "CMA_TEST_WORK_PROCESS_DATABASE";
const PROCESS_KIND: &str = "CMA_TEST_WORK_PROCESS_KIND";
const CONTROL: &str = "test_work_process_control";
const POLL: Duration = Duration::from_millis(50);
// Normal/stressed calibration: claims <= 26 ms plus the 50 ms poll interval.
// Three seconds leaves about 39x headroom for scheduling and Mongo latency.
const SLACK: Duration = Duration::from_secs(3);

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn killed_sandbox_process_yields_renewed_work_to_one_survivor() {
    process_loss(Kind::Sandbox, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn killed_workspace_process_yields_renewed_work_to_one_survivor() {
    process_loss(Kind::Workspace, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resumed_sandbox_process_cannot_publish_after_takeover() {
    process_loss(Kind::Sandbox, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resumed_workspace_process_cannot_publish_after_takeover() {
    process_loss(Kind::Workspace, true).await;
}

async fn process_loss(kind: Kind, resume: bool) {
    assert_eq!(WORK_LEASE_SECONDS, 15);
    assert_eq!(WORK_RENEW_SECONDS, 3);
    let (client, name) = database("work_process_loss").await;
    let survivor = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_renewable_work(true);
    let competitor = peer(&name).await.with_renewable_work(true);
    seed(&survivor).await;
    let control = client.database(&name).collection::<Document>(CONTROL);
    let mut owner = OwnerProcess::start(&name, kind);

    timeout(Duration::from_secs(20), async {
        while control
            .find_one(doc! {"_id":"owner"})
            .await
            .unwrap()
            .is_none()
        {
            assert!(
                owner.0.try_wait().unwrap().is_none(),
                "owner exited before renewal"
            );
            sleep(POLL).await;
        }
    })
    .await
    .expect("owner must acquire and renew");
    let ready = control
        .find_one(doc! {"_id":"owner"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.get_i64("pid").unwrap(), i64::from(owner.0.id()));
    assert_ne!(owner.0.id(), std::process::id());
    let stale = captured(&survivor, kind).await;
    let original = kind.row(&survivor).await;
    let original_expiry = original
        .get_str("work_lease_expires_at")
        .unwrap()
        .to_owned();
    let revision = original.get_i64("revision").unwrap();
    let mut expiry = original_expiry.clone();
    let mut renewals = 0;
    // Observe actual renewals beyond the first expiry, then stop immediately
    // after a fresh renewal to exercise the longest remaining takeover window.
    timeout(Duration::from_secs(25), async {
        loop {
            let held = kind.row(&survivor).await;
            assert_eq!(held.get_i64("revision").unwrap(), revision);
            assert_eq!(
                held["record"]["work"]["lease"]["holder"].as_str(),
                Some("process-owner")
            );
            let current = held.get_str("work_lease_expires_at").unwrap();
            if current != expiry {
                assert!(current > expiry.as_str());
                expiry = current.to_owned();
                renewals += 1;
                if at(0) > original_expiry {
                    break;
                }
            }
            refused(kind.claim(&competitor, "premature", at(0), at(15)).await);
            assert!(owner.0.try_wait().unwrap().is_none());
            sleep(POLL).await;
        }
    })
    .await
    .expect("healthy owner must renew beyond its initial lease");
    assert!(renewals >= 5, "observed only {renewals} renewals");

    let fault = Instant::now();
    if resume {
        owner.signal("-STOP");
    } else {
        owner.0.kill().unwrap();
        let status = owner.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    let stopped = kind.row(&survivor).await;
    let expires_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(
            layer2_core::seconds_from_rfc3339(stopped.get_str("work_lease_expires_at").unwrap())
                .unwrap(),
        );
    let deadline = expires_at + SLACK;
    let remaining = deadline
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    let mut claim_latencies = Vec::new();
    let (winner, fence, claim_at, claim_latency) = timeout(remaining, async {
        loop {
            let (left, right) = tokio::join!(
                measured_claim(kind, &survivor, "survivor-a"),
                measured_claim(kind, &competitor, "survivor-b"),
            );
            let (left, left_at, left_latency) = left;
            let (right, right_at, right_latency) = right;
            claim_latencies.extend([left_latency, right_latency]);
            let left = accepted(left);
            let right = accepted(right);
            assert!(
                !(left.is_some() && right.is_some()),
                "two successors acquired"
            );
            if let Some(fence) = left {
                break (&survivor, fence, left_at, left_latency);
            }
            if let Some(fence) = right {
                break (&competitor, fence, right_at, right_latency);
            }
            sleep(POLL).await;
        }
    })
    .await
    .expect("real-time takeover must complete within SLACK after persisted lease expiry");
    let acquired_at = SystemTime::now();
    let takeover = fault.elapsed();
    assert!(
        acquired_at <= deadline,
        "takeover exceeded expiry + {SLACK:?}"
    );
    assert!(
        claim_at >= expires_at,
        "successor claimed before persisted expiry"
    );
    claim_latencies.sort_unstable();
    let median = (claim_latencies[(claim_latencies.len() - 1) / 2]
        + claim_latencies[claim_latencies.len() / 2])
        / 2;
    eprintln!(
        "takeover-timing kind={} mode={} after_expiry_ms={:.3} winner_claim_ms={:.3} claim_min_ms={:.3} claim_median_ms={:.3} claim_max_ms={:.3} claims={}",
        kind.id(), if resume { "pause-resume" } else { "sigkill" },
        acquired_at.duration_since(expires_at).unwrap().as_secs_f64() * 1000.0,
        claim_latency.as_secs_f64() * 1000.0,
        claim_latencies[0].as_secs_f64() * 1000.0,
        median.as_secs_f64() * 1000.0,
        claim_latencies.last().unwrap().as_secs_f64() * 1000.0,
        claim_latencies.len(),
    );
    assert_eq!(fence, 2);
    let successor = captured(winner, kind).await;
    let held = kind.row(winner).await;
    assert!(renew(winner, &stale).await.unwrap().is_none());
    let attempted = std::sync::atomic::AtomicBool::new(false);
    let rejected = WorkAuthority::new(stale.clone(), Instant::now())
        .run(
            async {
                attempted.store(true, Ordering::SeqCst);
                complete(winner, &stale).await
            },
            || renew(winner, &stale),
        )
        .await;
    assert_eq!(rejected, Err(SandboxStoreError::Conflict));
    assert!(!attempted.load(Ordering::SeqCst));
    assert_eq!(
        complete(winner, &stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        kind.row(winner).await,
        held,
        "predecessor must not mutate work"
    );

    if resume {
        control
            .update_one(doc! {"_id":"owner"}, doc! {"$set":{"release":true}})
            .await
            .unwrap();
        owner.signal("-CONT");
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = owner.0.try_wait().unwrap() {
                    assert!(status.success(), "resumed owner failed: {status}");
                    break;
                }
                sleep(POLL).await;
            }
        })
        .await
        .expect("expired owner must stop promptly after resume");
        let result = control
            .find_one(doc! {"_id":"owner"})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.get_str("outcome").unwrap(), "conflict");
        assert!(!result.contains_key("publication_attempted"));
        assert_eq!(kind.row(winner).await, held);
    }

    WorkAuthority::new(successor.clone(), Instant::now())
        .run(complete(winner, &successor), || renew(winner, &successor))
        .await
        .unwrap();
    let completed = kind.row(winner).await;
    assert!(!completed.get_bool("work_pending").unwrap());
    assert_eq!(completed.get_i64("revision").unwrap(), revision + 2);
    assert_eq!(
        completed["record"]["name"].as_str(),
        Some("survivor-publication")
    );
    assert!(kind
        .claim(&competitor, "duplicate", at(0), at(15))
        .await
        .unwrap()
        .is_none());
    assert_eq!(kind.row(winner).await, completed);
    println!("process-loss kind={} mode={} renewals={} takeover_ms={} recovery_ms={} fence={} publications=1 stale_publications=0",
        kind.id(), if resume { "pause-resume" } else { "sigkill" }, renewals,
        takeover.as_millis(), fault.elapsed().as_millis(), fence);
    drop(owner);
    client.database(&name).drop().await.unwrap();
}

async fn measured_claim(
    kind: Kind,
    store: &MongoSandboxStore,
    holder: &str,
) -> (Result<Option<u64>, SandboxStoreError>, SystemTime, Duration) {
    let started = Instant::now();
    let claim_at = SystemTime::now();
    let result = kind
        .claim(store, holder, layer2_core::rfc3339_at(claim_at), at(15))
        .await;
    (result, claim_at, started.elapsed())
}

fn accepted(result: Result<Option<u64>, SandboxStoreError>) -> Option<u64> {
    match result {
        Ok(value) => value,
        Err(SandboxStoreError::Conflict) => None,
        Err(error) => panic!("unexpected acquisition failure: {error:?}"),
    }
}

struct OwnerProcess(Child);

impl OwnerProcess {
    fn start(database: &str, kind: Kind) -> Self {
        // Reuse the exact compiled test executable; the child creates its own
        // runtime, client and authority, with no inherited in-memory store.
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!(
                        "{}::owner_subprocess",
                        module_path!().trim_start_matches("cma::")
                    ),
                    "--nocapture",
                ])
                .env(PROCESS_DATABASE, database)
                .env(PROCESS_KIND, kind.id())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn signal(&self, signal: &str) {
        assert!(Command::new("kill")
            .args([signal, &self.0.id().to_string()])
            .status()
            .unwrap()
            .success());
    }
}

impl Drop for OwnerProcess {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(error) => eprintln!("cannot inspect owned child: {error}"),
        }
        if let Err(error) = self.0.kill() {
            eprintln!("cannot kill owned child: {error}");
        }
        if let Err(error) = self.0.wait() {
            eprintln!("cannot reap owned child: {error}");
        }
    }
}

#[tokio::test]
async fn owner_subprocess() {
    let Ok(name) = std::env::var(PROCESS_DATABASE) else {
        return;
    };
    assert!(name.starts_with("cma_804_work_process_loss_"));
    let kind = match std::env::var(PROCESS_KIND).unwrap().as_str() {
        "sbx_server_claim" => Kind::Sandbox,
        "wks_server_claim" => Kind::Workspace,
        other => panic!("unexpected worker kind: {other}"),
    };
    let store = peer(&name).await.with_renewable_work(true);
    let start = Instant::now();
    assert_eq!(
        kind.claim(&store, "process-owner", at(0), at(15))
            .await
            .unwrap(),
        Some(1)
    );
    let claim = captured(&store, kind).await;
    let control = store.client.database(&name).collection::<Document>(CONTROL);
    let result = WorkAuthority::new(claim.clone(), start)
        .run(
            async {
                control
                    .insert_one(
                        doc! {"_id":"owner","release":false,"pid":i64::from(std::process::id())},
                    )
                    .await
                    .unwrap();
                loop {
                    let row = control
                        .find_one(doc! {"_id":"owner"})
                        .await
                        .unwrap()
                        .unwrap();
                    if row.get_bool("release").unwrap() {
                        break;
                    }
                    sleep(POLL).await;
                }
                control
                    .update_one(
                        doc! {"_id":"owner"},
                        doc! {"$set":{"publication_attempted":true}},
                    )
                    .await
                    .unwrap();
                complete(&store, &claim).await
            },
            || renew(&store, &claim),
        )
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    control
        .update_one(doc! {"_id":"owner"}, doc! {"$set":{"outcome":"conflict"}})
        .await
        .unwrap();
}
