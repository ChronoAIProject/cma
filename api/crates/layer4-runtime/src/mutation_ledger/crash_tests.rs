use std::io::Write;
use std::process::Command;

use super::tests::{close, Fixture};
use super::*;
use crate::mutation_journal::Status;
use layer3_adapter::{wire::AgentCloseReceipt, AgentSlot};

fn receipt() -> AgentCloseReceipt {
    AgentCloseReceipt {
        operation_id: close().operation_id,
        request_sha256: close().request_sha256,
        agent_id: AgentSlot::new("agent"),
    }
}
fn child(fixture: &Fixture, stage: &str) -> i32 {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mutation_ledger::crash_tests::crash_child",
            "--ignored",
            "--nocapture",
        ])
        .env("CMA_LEDGER_TEST_ROOT", fixture.database().parent().unwrap())
        .env("CMA_LEDGER_TEST_STAGE", stage)
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    assert!(
        code == 86 || code == 87,
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    code
}

#[test]
#[ignore = "invoked only by the bounded ledger crash parent"]
fn crash_child() {
    let root =
        std::path::PathBuf::from(std::env::var_os("CMA_LEDGER_TEST_ROOT").expect("test root"));
    let stage = std::env::var("CMA_LEDGER_TEST_STAGE").expect("test stage");
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let config=RuntimeConfig::for_test(root.join("home"));
        let result=Ledger::open(root.join("ledger"),config,false,32_768).await;
        if stage=="lock" {
            assert_eq!(result.err().unwrap().code,"mutation_ledger_writer_held");
            std::process::exit(87);
        }
        let ledger=result.unwrap();
        if stage=="hot-journal" {
            ledger.storage.call::<()>(|db| {
                db.pragma_update(None,"cache_size",1).unwrap();
                db.execute_batch("BEGIN IMMEDIATE; UPDATE accounting SET revision=revision+1000; CREATE TABLE uncommitted (bytes BLOB); INSERT INTO uncommitted VALUES (zeroblob(262144));").unwrap();
                // Exit without unwinding transaction, connection or writer lock.
                std::process::exit(86);
            }).await.unwrap();
        }
        let entry=ledger.begin(close()).await.unwrap();
        if stage=="started" {std::process::exit(86);}
        let mut marker=std::fs::OpenOptions::new().write(true).create_new(true).open(root.join("effect")).unwrap();
        marker.write_all(b"one effect").unwrap();
        marker.sync_all().unwrap();
        if stage=="effect" {std::process::exit(86);}
        ledger.finish(entry.record.completed(&receipt()).unwrap()).await.unwrap();
        std::process::exit(86);
    });
}

#[tokio::test]
async fn a_second_process_is_excluded_and_dead_writer_hot_journal_recovers() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger.admit(1, vec![close()]).await.unwrap();
    let before = ledger.snapshot().await.unwrap();
    assert_eq!(child(&fixture, "lock"), 87);
    ledger.close().await.unwrap();
    assert_eq!(child(&fixture, "hot-journal"), 86);
    let journal = fixture.database().join("ledger.sqlite3-journal");
    assert!(journal.metadata().unwrap().len() > 0);
    let ledger = fixture.open(false).await;
    let after = ledger.snapshot().await.unwrap();
    assert_eq!(after, before); // Neither accounting nor uncommitted table survived.
    assert!(!ledger.admitted(close()).await.unwrap());
    ledger.begin(close()).await.unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn abrupt_exit_at_started_effect_and_terminal_boundaries_preserves_exact_state() {
    for stage in ["started", "effect", "terminal"] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        ledger.admit(1, vec![close()]).await.unwrap();
        ledger.close().await.unwrap();
        assert_eq!(child(&fixture, stage), 86);
        let marker = fixture.database().parent().unwrap().join("effect");
        assert_eq!(marker.exists(), stage != "started");
        let ledger = fixture.open(false).await;
        let entry = ledger.begin(close()).await.unwrap();
        assert!(!entry.new);
        if stage == "terminal" {
            assert_eq!(entry.record.status(), Status::Completed);
            assert_eq!(
                entry.record.receipt::<AgentCloseReceipt>().unwrap(),
                receipt()
            );
        } else {
            assert_eq!(entry.record.status(), Status::Started);
            assert_eq!(
                ledger.started_page(0, String::new()).await.unwrap().len(),
                1
            );
        }
        if marker.exists() {
            assert_eq!(std::fs::read(marker).unwrap(), b"one effect");
        }
        ledger.close().await.unwrap();
    }
}
