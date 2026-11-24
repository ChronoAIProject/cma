use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

struct Exited(Arc<AtomicBool>);
impl Drop for Exited {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test(start_paused = true)]
async fn http_shutdown_starts_immediately_and_workers_remain_until_http_finishes() {
    let (http_stop, signal) = watch::channel(false);
    let mut shutdown = ShutdownCoordinator::new();
    let worker_signal = shutdown.subscribe();
    let stopped = Arc::new(AtomicBool::new(false));
    let exited = Exited(stopped.clone());
    shutdown.push(tokio::spawn(async move {
        let _exited = exited;
        signalled(worker_signal).await;
    }));
    let started = Instant::now();
    let server = Box::pin(async move {
        signalled(signal).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert!(!stopped.load(Ordering::Acquire));
        tokio::time::sleep(Duration::from_secs(9)).await;
        assert!(!stopped.load(Ordering::Acquire));
        Ok(())
    });
    serve_contained(
        server,
        http_stop,
        async { DrainCause::Shutdown },
        None,
        shutdown,
    )
    .await
    .unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(9));
}

#[tokio::test(start_paused = true)]
async fn http_timeout_then_worker_abort_are_observed_within_one_process_envelope() {
    let (http_stop, _signal) = watch::channel(false);
    let mut shutdown = ShutdownCoordinator::new();
    let stopped = Arc::new(AtomicBool::new(false));
    let exited = Exited(stopped.clone());
    shutdown.push(tokio::spawn(async move {
        let _exited = exited;
        std::future::pending::<()>().await;
    }));
    let started = Instant::now();
    let error = serve_contained(
        Box::pin(std::future::pending()),
        http_stop,
        async { DrainCause::Shutdown },
        None,
        shutdown,
    )
    .await
    .unwrap_err();
    assert_eq!(error, "HTTP server exceeded its shutdown drain deadline");
    assert!(stopped.load(Ordering::Acquire));
    assert_eq!(started.elapsed(), Duration::from_secs(40));
    assert!(started.elapsed() <= PROCESS_LIMIT);
}

#[tokio::test(start_paused = true)]
async fn worker_stage_and_final_observation_cannot_extend_the_process_deadline() {
    let mut shutdown = ShutdownCoordinator::new();
    shutdown.push(tokio::spawn(std::future::pending()));
    let started = Instant::now();
    assert!(shutdown
        .drain_contained_workers(started + Duration::from_secs(2))
        .await
        .is_err());
    assert_eq!(started.elapsed(), Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn fatal_ownership_trigger_remains_an_error_after_complete_cleanup() {
    let (http_stop, signal) = watch::channel(false);
    let server = Box::pin(async move {
        signalled(signal).await;
        Ok(())
    });
    assert_eq!(
        serve_contained(
            server,
            http_stop,
            async { DrainCause::OwnershipFailure },
            None,
            ShutdownCoordinator::new()
        )
        .await,
        Err("Agent Manager ownership maintenance failed".into())
    );
}

#[tokio::test(start_paused = true)]
async fn server_failure_still_signals_and_joins_workers() {
    let (http_stop, signal) = watch::channel(false);
    let mut shutdown = ShutdownCoordinator::new();
    let worker_signal = shutdown.subscribe();
    let stopped = Arc::new(AtomicBool::new(false));
    let exited = Exited(stopped.clone());
    shutdown.push(tokio::spawn(async move {
        let _exited = exited;
        signalled(worker_signal).await;
    }));
    let server = Box::pin(async { Err(std::io::Error::other("synthetic HTTP failure")) });
    assert!(
        serve_contained(server, http_stop, std::future::pending(), None, shutdown)
            .await
            .is_err()
    );
    assert!(*signal.borrow());
    assert!(stopped.load(Ordering::Acquire));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unconfirmed_abort_returns_unclean_without_claiming_worker_termination() {
    let mut shutdown = ShutdownCoordinator::new();
    let stopped = Arc::new(AtomicBool::new(false));
    let exited = Exited(stopped.clone());
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finished, observed) = tokio::sync::oneshot::channel();
    shutdown.push(tokio::spawn(async move {
        let exit_guard = exited;
        started.send(()).unwrap();
        // Model an existing uncooperative worker with a bounded escape hatch.
        // Aborting its handle cannot interrupt this controlled synchronous wait.
        let _ = blocked.recv_timeout(Duration::from_secs(3));
        drop(exit_guard);
        let _ = finished.send(());
    }));
    running.await.unwrap();
    let result = shutdown
        .drain_contained_workers(Instant::now() + Duration::from_millis(100))
        .await;
    let still_executing = !stopped.load(Ordering::Acquire);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), observed)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    assert!(
        still_executing,
        "a timeout must not imply termination or freed backing"
    );
    assert!(stopped.load(Ordering::Acquire));
}
