use super::*;

#[tokio::test]
async fn replacement_fails_at_the_short_startup_deadline() {
    let root = TestDirectory::new();
    let mut child = serving_process(&root.0.join("held"), None, false);
    let (_stop, shutdown) = watch::channel(false);
    let (state, _) = watch::channel(State::Restarting);
    let mut recovery = Recovery {
        state,
        shutdown,
        budget: RestartBudget::new(),
        startup_limit: SHORT_STARTUP_LIMIT,
    };

    tokio::time::pause();
    let started = Instant::now();
    let (end, ready_since) = recovery.drive(&mut child, 1, None).await;
    let elapsed = started.elapsed();
    tokio::time::resume();

    child.finish(false).await.unwrap();
    assert!(matches!(end, End::StartupTimeout));
    assert_eq!(ready_since, None);
    assert!(
        (SHORT_STARTUP_LIMIT..=SHORT_STARTUP_LIMIT + Duration::from_millis(1)).contains(&elapsed)
    );
}

#[tokio::test]
async fn replacement_authenticates_after_thirty_seconds_with_extended_startup() {
    let root = TestDirectory::new();
    let release = root.0.join("release");
    let mut child = serving_process(&release, None, false);
    let (stop, shutdown) = watch::channel(false);
    let (state, mut current) = watch::channel(State::Restarting);
    let startup_limit = Duration::from_secs(90);
    let mut recovery = Recovery {
        state,
        shutdown,
        budget: RestartBudget::new(),
        startup_limit,
    };

    tokio::time::pause();
    let started = Instant::now();
    let observe = async {
        tokio::time::sleep(Duration::from_secs(45)).await;
        assert!(matches!(*current.borrow(), State::Restarting));
        // The fixture child uses the real clock to serve the authenticated display.
        tokio::time::resume();
        std::fs::write(release, b"").unwrap();
        let generation = ready_generation(&mut current).await;
        let mut display = connect(&generation, DISPLAY_URL).await.unwrap();
        assert_eq!(
            display.next().await.unwrap().unwrap(),
            Message::Binary(b"RFB 003.008\n".to_vec()),
        );
        stop.send(true).unwrap();
    };
    let ((end, ready_since), ()) = tokio::join!(recovery.drive(&mut child, 1, None), observe);
    child.finish(false).await.unwrap();
    assert!(matches!(end, End::Shutdown));
    let elapsed = ready_since.unwrap().duration_since(started);
    assert!((Duration::from_secs(45)..startup_limit).contains(&elapsed));
}
