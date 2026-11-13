use super::*;
use std::cell::Cell;
use tokio::time::{sleep, Instant};

fn cohort(count: usize) -> Vec<AgentId> {
    (0..count)
        .map(|i| AgentId(format!("agt_startup_{i}")))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn successful_startup_waits_for_each_serial_preparation() {
    let ids = cohort(3);
    let completed = Cell::new(0);
    let started = Instant::now();
    let ready = Cell::new(false);
    let startup = async {
        let prepared = prepare_cohort(&ids, |_| async {
            sleep(Duration::from_secs(2)).await;
            completed.set(completed.get() + 1);
            Ok(())
        })
        .await
        .unwrap();
        ready.set(true);
        prepared
    };
    let observe = async {
        sleep(Duration::from_secs(3)).await;
        assert_eq!(completed.get(), 1);
        assert!(!ready.get());
    };
    let (prepared, ()) = tokio::join!(startup, observe);
    assert_eq!(prepared, ids);
    assert!(ready.get());
    assert_eq!(started.elapsed(), Duration::from_secs(6));
}

#[tokio::test]
async fn invalid_unavailable_and_unconfirmed_preparation_prevent_startup() {
    for reason in [
        "preparation Agent invalid",
        "preparation lookup unavailable",
        "preparation commit unconfirmed; cohort remains disabled",
    ] {
        let calls = Cell::new(0);
        let ids = cohort(3);
        let result = prepare_cohort(&ids, |_| {
            calls.set(calls.get() + 1);
            let result = if calls.get() == 2 {
                Err(reason.to_owned())
            } else {
                Ok(())
            };
            std::future::ready(result)
        })
        .await;
        assert_eq!(result, Err(reason.to_owned()));
        assert_eq!(calls.get(), 2, "failure must not begin the next Agent");
    }
}

#[tokio::test(start_paused = true)]
async fn per_agent_and_total_timeouts_prevent_startup() {
    for (count, seconds, elapsed, message) in [
        (1, 11, 10, "Agent log preparation timed out"),
        (7, 9, 60, "Agent log cohort preparation exceeded 60 seconds"),
    ] {
        let ids = cohort(count);
        let started = Instant::now();
        let result = prepare_cohort(&ids, |_| async {
            sleep(Duration::from_secs(seconds)).await;
            Ok(())
        })
        .await;
        assert_eq!(result, Err(message.to_owned()));
        assert_eq!(started.elapsed(), Duration::from_secs(elapsed));
    }
}

#[tokio::test]
async fn empty_and_oversized_cohorts_do_not_begin_preparation() {
    for count in [0, 17] {
        let ids = cohort(count);
        let result = prepare_cohort(&ids, |_| async { panic!("unexpected preparation") }).await;
        assert_eq!(result.is_ok(), count == 0);
    }
}
