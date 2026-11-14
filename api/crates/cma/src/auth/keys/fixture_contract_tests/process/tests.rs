use super::cleanup::{self, ChildControl, Failure};
use super::{directory::PrivateDirectory, receipt};
use serde_json::{json, Value};
use std::{collections::VecDeque, io, panic::AssertUnwindSafe, time::Duration};
use tokio::{io::BufReader, time::Instant};

enum Wait {
    Exit(bool),
    Error,
    Pending,
}

struct ControlledChild {
    waits: VecDeque<Wait>,
    kill_error: bool,
    kills: usize,
}

impl ControlledChild {
    fn new(waits: impl IntoIterator<Item = Wait>, kill_error: bool) -> Self {
        Self {
            waits: waits.into_iter().collect(),
            kill_error,
            kills: 0,
        }
    }
}

impl ChildControl for ControlledChild {
    async fn wait(&mut self) -> io::Result<bool> {
        match self.waits.pop_front().expect("declared wait outcome") {
            Wait::Exit(success) => Ok(success),
            Wait::Error => Err(io::Error::other("synthetic wait error")),
            Wait::Pending => std::future::pending().await,
        }
    }
    fn start_kill(&mut self) -> io::Result<()> {
        self.kills += 1;
        if self.kill_error {
            Err(io::Error::other("synthetic kill error"))
        } else {
            Ok(())
        }
    }
}

fn wire() -> Value {
    json!({"type":"closed", "reason":"stdin_closed", "receipt":{
        "schema":"ordinary-work-provider-ledger-v1", "attempted_sends":null,
        "received_mint_posts":1, "accepted_calls":1, "distinct_effects":1,
        "physical_commits":1, "replays":0, "refusals":0, "failed":false,
        "pending_at_close":[{"request":1,"phase":"response_held"}]
    }})
}

async fn controlled_close(child: &mut ControlledChild, row: Value) -> (Result<(), Failure>, Value) {
    let mut directory = PrivateDirectory::new();
    directory.spawned();
    let path = directory.path().to_owned();
    std::fs::write(path.join("provider.jsonl"), "synthetic journal\n").unwrap();
    let bytes = format!("{row}\n");
    let result = cleanup::close(child, &mut BufReader::new(bytes.as_bytes()), &mut directory).await;
    drop(directory);
    let retained = if path.exists() {
        assert_eq!(
            std::fs::read_to_string(path.join("provider.jsonl")).unwrap(),
            "synthetic journal\n"
        );
        let receipt =
            serde_json::from_slice(&std::fs::read(path.join("cleanup-receipt.json")).unwrap())
                .unwrap();
        // Only the controlled stub used this directory; no OS child exists.
        std::fs::remove_dir_all(path).unwrap();
        receipt
    } else {
        Value::Null
    };
    (result, retained)
}

#[tokio::test(start_paused = true)]
async fn clean_exit_with_closed_receipt_removes_state_without_kill() {
    let mut child = ControlledChild::new([Wait::Exit(true)], false);
    let (result, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(result, Ok(()));
    assert!(retained.is_null());
    assert_eq!(child.kills, 0);
}

#[tokio::test(start_paused = true)]
async fn wait_error_and_join_error_retain_unconfirmed_journal() {
    let mut child = ControlledChild::new([Wait::Error, Wait::Error], false);
    let (result, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(result, Err(Failure::WaitError));
    assert_eq!(retained["exit_confirmed"], false);
    assert_eq!(retained["directory_removed"], false);
    assert_eq!(retained["failures"], json!(["wait_error", "join_error"]));
    assert_eq!(retained["closed_receipt_summary"]["accepted_calls"], 1);
    assert_eq!(child.kills, 1);
}

#[tokio::test(start_paused = true)]
async fn kill_error_does_not_substitute_for_confirmed_exit() {
    let mut child = ControlledChild::new([Wait::Error, Wait::Error], true);
    let (_, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(retained["exit_confirmed"], false);
    assert_eq!(
        retained["failures"],
        json!(["wait_error", "kill_error", "join_error"])
    );
}

#[tokio::test(start_paused = true)]
async fn kill_error_still_joins_an_already_exited_child_but_retains_failure() {
    let mut child = ControlledChild::new([Wait::Error, Wait::Exit(true)], true);
    let (result, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(result, Err(Failure::WaitError));
    assert_eq!(retained["exit_confirmed"], true);
    assert_eq!(retained["directory_removed"], false);
    assert_eq!(retained["failures"], json!(["wait_error", "kill_error"]));
}

#[tokio::test(start_paused = true)]
async fn hanging_wait_and_join_share_one_six_second_deadline() {
    let start = Instant::now();
    let mut child = ControlledChild::new([Wait::Pending, Wait::Pending], false);
    let (result, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(Instant::now() - start, Duration::from_secs(6));
    assert_eq!(result, Err(Failure::GraceTimeout));
    assert_eq!(retained["exit_confirmed"], false);
    assert_eq!(
        retained["failures"],
        json!(["grace_timeout", "join_timeout"])
    );
}

#[tokio::test(start_paused = true)]
async fn forced_confirmed_exit_keeps_timeout_evidence() {
    let start = Instant::now();
    let mut child = ControlledChild::new([Wait::Pending, Wait::Exit(false)], false);
    let (result, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(Instant::now() - start, Duration::from_secs(5));
    assert_eq!(result, Err(Failure::GraceTimeout));
    assert_eq!(retained["exit_confirmed"], true);
    assert_eq!(retained["kill_requested"], true);
}

#[tokio::test(start_paused = true)]
async fn unsuccessful_exit_retains_state_without_additional_signal() {
    let mut child = ControlledChild::new([Wait::Exit(false)], false);
    let (result, retained) = controlled_close(&mut child, wire()).await;
    assert_eq!(result, Err(Failure::ExitUnsuccessful));
    assert_eq!(retained["exit_confirmed"], true);
    assert_eq!(child.kills, 0);
}

#[test]
fn pre_spawn_failure_removes_unused_state() {
    let directory = PrivateDirectory::new();
    let path = directory.path().to_owned();
    drop(directory);
    assert!(!path.exists());
}

#[tokio::test(start_paused = true)]
async fn cancelled_close_keeps_journal_and_fixed_incomplete_receipt() {
    let mut directory = PrivateDirectory::new();
    directory.spawned();
    let path = directory.path().to_owned();
    let mut child = ControlledChild::new([Wait::Pending], false);
    let bytes = format!("{}\n", wire());
    let mut output = BufReader::new(bytes.as_bytes());
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        cleanup::close(&mut child, &mut output, &mut directory)
    )
    .await
    .is_err());
    drop(directory);
    let retained: Value =
        serde_json::from_slice(&std::fs::read(path.join("cleanup-receipt.json")).unwrap()).unwrap();
    assert_eq!(retained["exit_confirmed"], false);
    assert_eq!(retained["child_spawned"], true);
    assert_eq!(retained["failures"], json!(["interrupted"]));
    assert_eq!(child.kills, 0);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn original_panic_survives_cleanup_failure() {
    let panic = std::panic::catch_unwind(AssertUnwindSafe(|| {
        cleanup::finish_test(
            Err(Box::new("original test failure")),
            Ok(Err(Failure::JoinError)),
        );
    }))
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"original test failure"));
}

#[test]
fn cleanup_failure_fails_an_otherwise_successful_test() {
    assert!(
        std::panic::catch_unwind(|| cleanup::finish_test(Ok(()), Ok(Err(Failure::JoinError))))
            .is_err()
    );
    cleanup::finish_test(Ok(()), Ok(Ok(())));
}

#[test]
fn original_panic_survives_an_unexpected_cleanup_panic() {
    let panic = std::panic::catch_unwind(AssertUnwindSafe(|| {
        cleanup::finish_test(
            Err(Box::new("original test failure")),
            Err(Box::new("cleanup failure")),
        );
    }))
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"original test failure"));
}

#[test]
fn unexpected_cleanup_panic_fails_an_otherwise_successful_test() {
    let panic = std::panic::catch_unwind(AssertUnwindSafe(|| {
        cleanup::finish_test(Ok(()), Err(Box::new("cleanup failure")));
    }))
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"cleanup failure"));
}

#[tokio::test(start_paused = true)]
async fn invalid_closed_receipt_never_deletes_even_a_confirmed_childs_state() {
    let mut child = ControlledChild::new([Wait::Exit(true)], false);
    let (result, retained) = controlled_close(&mut child, json!({"type":"error"})).await;
    assert_eq!(result, Err(Failure::ReceiptInvalid));
    assert_eq!(retained["exit_confirmed"], true);
    assert!(retained["closed_receipt_summary"].is_null());
}

#[tokio::test(start_paused = true)]
async fn missing_closed_receipt_cannot_extend_the_cleanup_deadline() {
    let (reader, _held_writer) = tokio::io::duplex(64);
    let mut directory = PrivateDirectory::new();
    directory.spawned();
    let path = directory.path().to_owned();
    let mut child = ControlledChild::new([Wait::Exit(true)], false);
    let start = Instant::now();
    let result = cleanup::close(&mut child, &mut BufReader::new(reader), &mut directory).await;
    assert_eq!(result, Err(Failure::ReceiptTimeout));
    assert_eq!(Instant::now() - start, Duration::from_secs(6));
    drop(directory);
    assert!(path.join("cleanup-receipt.json").exists());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn remaining_lock_refuses_deletion_after_confirmed_exit() {
    let mut directory = PrivateDirectory::new();
    directory.spawned();
    let path = directory.path().to_owned();
    std::fs::write(path.join("provider.lock"), "").unwrap();
    let result = directory.finish(
        cleanup::ExitObservation {
            exit_confirmed: true,
            ..Default::default()
        },
        Some(json!({})),
    );
    assert_eq!(result, Err(Failure::LockRemains));
    drop(directory);
    assert!(path.join("provider.lock").exists());
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn close_summary_preserves_unknown_sends_and_pending_without_arbitrary_output() {
    let mut row = wire();
    row["receipt"]["unexpected_private_payload"] = json!("must not be copied");
    let bytes = format!("{row}\n");
    let summary = receipt::read_closed(
        &mut BufReader::new(bytes.as_bytes()),
        Instant::now() + Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(summary["attempted_sends"], Value::Null);
    assert_eq!(
        summary["pending_at_close"],
        row["receipt"]["pending_at_close"]
    );
    assert_eq!(summary["wire_sha256"].as_str().unwrap().len(), 64);
    assert!(!summary.to_string().contains("must not be copied"));
}

#[tokio::test]
async fn close_receipt_rejects_invalid_counts_pending_failure_and_framing() {
    let mut bad_count = wire();
    bad_count["receipt"]["accepted_calls"] = json!(-1);
    let mut failed = wire();
    failed["receipt"]["failed"] = json!(true);
    let mut missing = wire();
    missing["receipt"]
        .as_object_mut()
        .unwrap()
        .remove("attempted_sends");
    let mut bad_phase = wire();
    bad_phase["receipt"]["pending_at_close"][0]["phase"] = json!("arbitrary");
    let mut bad_request = wire();
    bad_request["receipt"]["pending_at_close"][0]["request"] = json!(0);
    for bytes in [
        format!("{bad_count}\n"),
        format!("{failed}\n"),
        format!("{missing}\n"),
        format!("{bad_phase}\n"),
        format!("{bad_request}\n"),
        wire().to_string(),
        "not JSON\n".into(),
    ] {
        assert_eq!(
            receipt::read_closed(
                &mut BufReader::new(bytes.as_bytes()),
                Instant::now() + Duration::from_secs(1)
            )
            .await,
            Err(Failure::ReceiptInvalid)
        );
    }
}
