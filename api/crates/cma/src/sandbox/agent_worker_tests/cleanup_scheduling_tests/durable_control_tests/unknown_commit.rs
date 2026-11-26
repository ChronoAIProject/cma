use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn unknown_commit_with_unavailable_readback_never_replays_control_writes() {
    for deleting in [false, true] {
        // Calibrate the read prefix from the actual public composition so the
        // fault begins exactly at commit, leaving authorization/admission reads
        // healthy. The injected uncertainty remains active for readback.
        let baseline = Fixture::new(deleting).await;
        let response = baseline.raw(baseline.owner, "calibration", "{}").await;
        assert_eq!(response.status(), 202);
        let reads = baseline
            .commands
            .lock()
            .unwrap()
            .iter()
            .take_while(|name| name.as_str() != "commitTransaction")
            .filter(|name| name.as_str() == "find")
            .count();
        assert!(reads >= 4);
        baseline.complete().await;
        baseline.close().await;

        let f = Fixture::new(deleting).await;
        f.fail_commands(
            doc! {"failCommands":["commitTransaction","find"],
            "errorCode":50,"errorLabels":["UnknownTransactionCommitResult"]},
            doc! {"skip":reads as i32}.into(),
        )
        .await;
        let response = f.raw(1 - f.owner, "unknown-commit", "{}").await;
        f.disable_failpoint().await;
        assert_eq!(response.status(), 503);
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "instance_service_unavailable"
        );
        let commands = f.commands.lock().unwrap().clone();
        assert_eq!(
            commands
                .iter()
                .filter(|name| name.as_str() == "commitTransaction")
                .count(),
            5
        );
        let first_commit = commands
            .iter()
            .position(|name| name == "commitTransaction")
            .unwrap();
        assert!(commands[first_commit..].iter().any(|name| name == "find"));
        assert!(
            !commands[first_commit..].iter().any(|name| name == "update"),
            "uncertain commit must not start a second write transaction"
        );
        assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision);
        assert!(f.control_receipt().await.is_none());
        // The same HTTP key replays the original refusal, never fresh execution.
        let again = f.raw(f.owner, "unknown-commit", "{}").await;
        assert_eq!(again.status(), 503);
        assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision);
        f.close().await;
    }
}
