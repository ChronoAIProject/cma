use super::*;

async fn cancel_terminal_command(command: &'static str) {
    let f = Fixture::new().await;
    f.prepare().await;
    let abort = command == "abortTransaction";
    if abort {
        f.takeover().await;
    }
    f.fail(
        &[command],
        1,
        doc! {"blockConnection":true,"blockTimeMS":1000},
    )
    .await;
    *f.commands.terminal_command.lock().unwrap() = Some(command);
    let store = f.store.clone();
    let pending = tokio::spawn(async move {
        store
            .reconcile_bindings("original", "sandbox", vec![], now() - 900)
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        f.commands.terminal_started.notified(),
    )
    .await
    .expect("transaction must reach its terminal command");

    // Keep the real command pending on the server while cancelling its caller.
    // Async command callbacks observe dispatch but cannot pause driver work.
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        Duration::from_secs(5),
        f.commands.terminal_completed.notified(),
    )
    .await
    .expect("terminal command must complete after caller cancellation");
    f.disable().await;
    assert_eq!(f.commands.count(command), 1);

    f.successor
        .reconcile_bindings(
            if abort { "successor" } else { "original" },
            "sandbox",
            vec![],
            now() - 900,
        )
        .await
        .expect("successor must not conflict with a stranded transaction");
    assert_eq!(f.successor.mapping_revision().await.unwrap(), 2);
    let observer = f
        .successor
        .collection(CONTROL)
        .find_one(doc! {"_id":"observer"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        observer.get_i64("control_write_fence").unwrap(),
        if abort { 2 } else { 3 },
        "cancelled commit applies once; cancelled abort rolls back"
    );
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn cancelled_commit_completes_before_successor_writes() {
    cancel_terminal_command("commitTransaction").await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn cancelled_abort_completes_before_successor_writes() {
    cancel_terminal_command("abortTransaction").await;
}
