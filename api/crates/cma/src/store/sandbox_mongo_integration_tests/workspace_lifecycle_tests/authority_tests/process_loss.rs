//! Real process loss at the shared renewal/store boundary, without clock edits.
//! The composed-worker tests cover adapters; this is not an AM deployment test.

use super::*;
use futures_util::{stream::FuturesUnordered, StreamExt};
use layer2_core::sandbox::store::{
    ClaimedWork, WorkAuthority, WorkLeaseGrant, WORK_LEASE_SECONDS, WORK_RENEW_SECONDS,
};
use std::process::{Child, Command, Stdio};
use tokio::time::{sleep, timeout, timeout_at, Instant};

mod claims;
use claims::{captured, complete, renew};

const PROCESS_DATABASE: &str = "CMA_TEST_WORK_PROCESS_DATABASE";
const PROCESS_KIND: &str = "CMA_TEST_WORK_PROCESS_KIND";
const CONTROL: &str = "test_work_process_control";
const POLL: Duration = Duration::from_millis(50);

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
    let deadline = fault + Duration::from_secs(WORK_LEASE_SECONDS);
    let (winner, fence, takeover) = loop {
        let mut attempts = FuturesUnordered::new();
        for (store, holder) in [(&survivor, "survivor-a"), (&competitor, "survivor-b")] {
            attempts.push(async move { (store, kind.claim(store, holder, at(0), at(15)).await) });
        }
        let acquired = timeout_at(deadline, async {
            while let Some((store, result)) = attempts.next().await {
                if let Some(fence) = accepted(result) {
                    return Some((store, fence, fault.elapsed()));
                }
            }
            None
        })
        .await
        .expect("real-time takeover must complete within 15 seconds");
        if let Some(acquired) = acquired {
            // Time the first confirmed acquisition, not the losing transaction's
            // retries. Still finish every competitor and prove a single winner.
            while let Some((_, result)) = attempts.next().await {
                assert!(accepted(result).is_none(), "two successors acquired");
            }
            break acquired;
        }
        timeout_at(deadline, sleep(POLL))
            .await
            .expect("real-time takeover must complete within 15 seconds");
    };
    assert!(takeover <= Duration::from_secs(WORK_LEASE_SECONDS));
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
