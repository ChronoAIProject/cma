use super::super::process_fixture::*;
use super::*;
use mongodb::bson::doc;
use std::{process::Stdio, time::Duration};
use tokio::process::{Child, Command};

pub(super) fn spawn(fixture: &Fixture, pause: bool) -> Child {
    spawn_binary(fixture, pause, std::env::current_exe().unwrap())
}

pub(super) fn spawn_binary(
    fixture: &Fixture,
    pause: bool,
    binary: impl AsRef<std::ffi::OsStr>,
) -> Child {
    Command::new(binary)
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env("CMA_REVOCATION_CHILD_DATABASE", fixture.database().name())
        .env(
            "CMA_REVOCATION_CHILD_MODE",
            if pause { "pause" } else { "recover" },
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

// Real process death and worker persistence; synthetic receiver evidence.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn worker_process_death_recovers_original_revocation() {
    for cut in [Cut::BeforeStop, Cut::AfterReceipt] {
        let fixture = Fixture::new().await;
        let row = fixture.register("process-recovery").await;
        let operation = fixture.freeze().await;
        let scripts = fixture.database().collection::<Script>(COLLECTION);
        scripts
            .insert_one(Script::new(&row, &operation, cut))
            .await
            .unwrap();
        let mut child = spawn(&fixture, true);
        let first_pid = child.id().unwrap();
        assert_ne!(first_pid, std::process::id());
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "child exited before its cut"
                );
                if scripts
                    .find_one(doc! {"_id": "participant", "reached": true})
                    .await
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        child.kill().await.unwrap();
        let killed = child.wait_with_output().await.unwrap();
        assert!(!killed.status.success());
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(killed.status.signal(), Some(9));
        }
        assert!(!String::from_utf8(killed.stdout)
            .unwrap()
            .contains(COMPLETED));
        let pending = fixture
            .store
            .revocation(&operation.key.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.key, operation.key);
        assert_eq!(pending.deadline_unix_ms, operation.deadline_unix_ms);
        assert!(matches!(pending.phase, RevocationPhaseV1::Pending));
        let stopped = fixture
            .store
            .record(&row.registration)
            .await
            .unwrap()
            .unwrap();
        assert!(stopped.barrier.is_some());
        assert!(stopped.completion.is_none());
        let before = scripts
            .find_one(doc! {"_id": "participant"})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stopped.barrier, Some(before.receipt.barrier.clone()));
        assert_eq!(before.retained, matches!(cut, Cut::AfterReceipt));
        assert_eq!(before.stop_effects, u32::from(before.retained));

        let replacement = spawn(&fixture, false);
        let replacement_pid = replacement.id().unwrap();
        assert_ne!(replacement_pid, std::process::id());
        let output = tokio::time::timeout(Duration::from_secs(15), replacement.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains(COMPLETED) && stdout.contains("1 passed"),
            "{stdout}"
        );
        let final_state = final_operation(&fixture, &operation).await;
        assert!(fixture.freeze().await == final_state);
        assert!(fixture
            .store
            .pending_revocations(None, 32)
            .await
            .unwrap()
            .is_empty());
        let after = scripts
            .find_one(doc! {"_id": "participant"})
            .await
            .unwrap()
            .unwrap();
        assert!(after.retained);
        assert_eq!(after.stop_effects, 1);
        let retained = fixture
            .store
            .record(&row.registration)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.member, row.member);
        assert_eq!(retained.basis, row.basis);
        assert_eq!(retained.barrier, Some(after.receipt.barrier.clone()));
        assert_eq!(
            retained.completion,
            Some(MemberCompletionV1::Execution {
                receipt: after.receipt
            })
        );
        let expected = match cut {
            Cut::BeforeStop => vec!["query", "barrier", "stop", "query", "barrier", "stop"],
            Cut::AfterReceipt => vec!["query", "barrier", "stop", "query"],
        };
        assert_eq!(after.calls, expected);
        println!("{cut:?}: killed worker {first_pid}; replacement {replacement_pid} recovered original operation");
        fixture.cleanup().await;
    }
}
