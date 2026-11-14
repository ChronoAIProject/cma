//! Force an item miss before a peer commits the item/head, using driver events.
use super::super::{
    mongo_fixture::Fixture,
    partition_model::{item_id, Item},
    store::Write,
    Error,
};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

fn emit(mode: &str, phase: &str, outcome: &str, started: Instant) {
    eprintln!(
        "split_read_control mode={mode} phase={phase} outcome={outcome} elapsed_ms={}",
        started.elapsed().as_millis()
    );
}

struct Barrier {
    mode: &'static str,
    started: Instant,
    request: AtomicI32,
    armed: AtomicBool,
    reached: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    item: String,
}
impl Barrier {
    fn observe(&self, event: CommandEvent) {
        match event {
            CommandEvent::Started(event)
                if self.armed.load(Ordering::SeqCst)
                    && event.command_name == "find"
                    && event
                        .command
                        .get_document("filter")
                        .ok()
                        .and_then(|filter| filter.get_str("_id").ok())
                        == Some(self.item.as_str()) =>
            {
                self.request.store(event.request_id, Ordering::SeqCst);
            }
            CommandEvent::Succeeded(event)
                if event.request_id == self.request.load(Ordering::SeqCst)
                    && self.armed.swap(false, Ordering::SeqCst) =>
            {
                let empty = event
                    .reply
                    .get_document("cursor")
                    .unwrap()
                    .get_array("firstBatch")
                    .unwrap()
                    .is_empty();
                emit(
                    self.mode,
                    "item_miss",
                    if empty { "observed" } else { "unexpected_row" },
                    self.started,
                );
                assert!(empty);
                self.reached
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(())
                    .unwrap();
                // Synchronous driver callbacks run before the find result is returned.
                // Four runtime workers leave the peer's independent client runnable.
                let release = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
                let outcome = match release {
                    Ok(()) => "released",
                    Err(mpsc::RecvTimeoutError::Timeout) => "timeout",
                    Err(_) => "closed",
                };
                emit(self.mode, "callback_wait", outcome, self.started);
                assert_eq!(outcome, "released", "bounded split-read barrier");
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn item_miss_before_peer_commit_verifies_exact_retained_custody() {
    for corruption in ["none", "missing", "mismatch"] {
        let started = Instant::now();
        emit(corruption, "setup", "start", started);
        let db = Fixture::new().await;
        let peer = db.node(2).await;
        let plan = super::plan().await;
        let key = item_id(&plan.targets[0].id);
        let (reached, observed) = tokio::sync::oneshot::channel();
        let (release, receiver) = mpsc::channel();
        let barrier = Arc::new(Barrier {
            mode: corruption,
            started,
            request: AtomicI32::new(-1),
            armed: AtomicBool::new(true),
            reached: Mutex::new(Some(reached)),
            release: Mutex::new(receiver),
            item: key.clone(),
        });
        let callback = barrier.clone();
        let caller = db
            .node_with_handler(
                1,
                Some(EventHandler::callback(move |event| callback.observe(event))),
            )
            .await;
        let partitions = caller.partitions.clone();
        let replay = plan.clone();
        let waiting = tokio::spawn(async move { partitions.enqueue(&replay).await });
        let reached = tokio::time::timeout(Duration::from_secs(3), observed).await;
        let outcome = match &reached {
            Ok(Ok(())) => "observed",
            Ok(Err(_)) => "closed",
            Err(_) => "timeout",
        };
        emit(corruption, "barrier_wait", outcome, started);
        reached.unwrap().unwrap();
        let committed = peer.partitions.enqueue(&plan).await;
        emit(
            corruption,
            "peer_commit",
            if committed.is_ok() { "ok" } else { "failed" },
            started,
        );
        committed.unwrap();
        if corruption == "missing" {
            peer.partitions
                .store
                .records
                .database()
                .unwrap()
                .collection::<mongodb::bson::Document>(crate::records::mongo::INTERACTIONS)
                .delete_one(mongodb::bson::doc! {"_id":&key})
                .await
                .unwrap();
        } else if corruption == "mismatch" {
            let (mut item, version) = peer
                .partitions
                .store
                .read::<Item>(&key, true)
                .await
                .unwrap()
                .unwrap();
            item.request_hash = "f".repeat(64);
            peer.partitions
                .store
                .write(
                    &key,
                    &plan.identity.owner,
                    &item,
                    Write {
                        expected: Some(version),
                        retained_until: Some(plan.retained_until),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        emit(corruption, "retained_item", "prepared", started);
        let released = release.send(());
        emit(
            corruption,
            "barrier_release",
            if released.is_ok() { "sent" } else { "closed" },
            started,
        );
        released.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), waiting).await;
        let outcome = match &result {
            Ok(Ok(Ok(()))) => "ok",
            Ok(Ok(Err(Error::Corrupt))) => "corrupt",
            Ok(Ok(Err(Error::Unavailable))) => "unavailable",
            Ok(Ok(Err(_))) => "other",
            Ok(Err(_)) => "task_failure",
            Err(_) => "timeout",
        };
        emit(corruption, "result", outcome, started);
        let result = result.unwrap().unwrap();
        assert!(!barrier.armed.load(Ordering::SeqCst));
        drop((caller, peer));
        emit(corruption, "cleanup", "start", started);
        db.cleanup().await;
        emit(corruption, "cleanup", "ok", started);
        if corruption == "none" {
            assert_eq!(result, Ok(()), "peer committed exact custody between reads");
        } else {
            assert_eq!(
                result,
                Err(Error::Corrupt),
                "unverifiable custody must still refuse"
            );
        }
        emit(corruption, "assertion", "matched", started);
    }
}
