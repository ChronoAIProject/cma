use super::*;
use std::sync::Mutex;

#[tokio::test(start_paused = true)]
async fn observations_preserve_results_and_cancellation_without_escaping_scope() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let observer = events.clone();
    assert_eq!(
        measure(Stage::Authorization, async { Ok::<_, ()>(7) }).await,
        Ok(7)
    );
    scope(
        move |stage, outcome, elapsed| observer.lock().unwrap().push((stage, outcome, elapsed)),
        async {
            assert_eq!(
                measure(Stage::Authorization, async { Ok::<_, ()>(42) }).await,
                Ok(42)
            );
            assert_eq!(
                measure(Stage::RuntimeRead, async { Err::<(), _>("private error") }).await,
                Err("private error")
            );
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(10),
                measure(Stage::ReadControl, std::future::pending::<Result<(), ()>>())
            )
            .await
            .is_err());
            // Spawned work does not inherit request diagnostics.
            tokio::spawn(measure(Stage::FinalFence, async { Ok::<_, ()>(()) }))
                .await
                .unwrap()
                .unwrap();
        },
    )
    .await;
    measure(Stage::FinalFence, async { Ok::<_, ()>(()) })
        .await
        .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].0, Stage::Authorization);
    assert_eq!(events[0].1, Outcome::Completed);
    assert_eq!(events[1].1, Outcome::Error);
    assert_eq!(events[2].1, Outcome::Cancelled);
}

#[tokio::test]
async fn concurrent_scopes_keep_observations_separate() {
    let left = Arc::new(Mutex::new(Vec::new()));
    let right = Arc::new(Mutex::new(Vec::new()));
    let a = left.clone();
    let b = right.clone();
    let (first, second) = tokio::join!(
        scope(
            move |stage, _, _| a.lock().unwrap().push(stage),
            measure(Stage::Authorization, async {
                tokio::task::yield_now().await;
                Ok::<_, ()>(1)
            })
        ),
        scope(
            move |stage, _, _| b.lock().unwrap().push(stage),
            measure(Stage::FinalFence, async { Ok::<_, ()>(2) })
        )
    );
    assert_eq!((first, second), (Ok(1), Ok(2)));
    assert_eq!(*left.lock().unwrap(), vec![Stage::Authorization]);
    assert_eq!(*right.lock().unwrap(), vec![Stage::FinalFence]);
}

#[tokio::test]
async fn transport_labels_distinguish_status_from_content_and_ignore_other_operations() {
    use crate::wire::WireOperation;
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    scope(
        move |stage, _, _| observed.lock().unwrap().push(stage),
        async {
            for operation in [
                WireOperation::RuntimeStatus,
                WireOperation::ReadAgentFile,
                WireOperation::ListAgentFiles,
            ] {
                for phase in [TransportPhase::Endpoint, TransportPhase::Control] {
                    assert_eq!(
                        transport(operation, phase, async { Ok::<_, ()>(5) }).await,
                        Ok(5)
                    );
                }
            }
        },
    )
    .await;
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            Stage::StatusEndpoint,
            Stage::StatusControl,
            Stage::ReadEndpoint,
            Stage::ReadControl
        ]
    );
}
