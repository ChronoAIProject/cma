use super::*;
use std::future::pending;
use tokio::time::sleep;

fn agents(count: usize) -> Vec<AgentId> {
    (0..count)
        .map(|index| AgentId(format!("agt_{index}")))
        .collect()
}

struct MarkCancelled<'a>(&'a mut bool);

impl Drop for MarkCancelled<'_> {
    fn drop(&mut self) {
        *self.0 = true;
    }
}

#[tokio::test(start_paused = true)]
async fn prepares_all_sixteen_agents_serially() {
    let ids = agents(16);
    let started = Instant::now();
    let prepared = prepare_cohort(Vec::new(), &ids, Instant::now, async |prepared, id| {
        sleep(Duration::from_secs(3)).await;
        prepared.push(id.clone());
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(prepared, ids);
    assert_eq!(started.elapsed(), Duration::from_secs(48));
}

#[tokio::test(start_paused = true)]
async fn preparation_error_prevents_returning_a_prepared_cohort() {
    let ids = agents(3);
    let mut calls = Vec::new();
    let result = prepare_cohort(Vec::new(), &ids, Instant::now, async |prepared, id| {
        calls.push(id.clone());
        if id == &ids[1] {
            return Err("preparation semantic checkpoint unavailable".into());
        }
        prepared.push(id.clone());
        Ok(())
    })
    .await;
    assert_eq!(
        result.unwrap_err(),
        "preparation semantic checkpoint unavailable"
    );
    assert_eq!(calls, ids[..2]);
}

#[tokio::test(start_paused = true)]
async fn per_agent_deadline_cancels_current_work_and_stops_the_cohort() {
    let ids = agents(2);
    let started = Instant::now();
    let mut calls = Vec::new();
    let mut cancelled = false;
    let result = prepare_cohort((), &ids, Instant::now, async |_, id| {
        calls.push(id.clone());
        let _guard = MarkCancelled(&mut cancelled);
        pending().await
    })
    .await;
    assert_eq!(result.unwrap_err(), "Agent log preparation timed out");
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    assert_eq!(calls, ids[..1]);
    assert!(cancelled);
}

#[tokio::test(start_paused = true)]
async fn cohort_deadline_cancels_current_work_before_its_own_deadline() {
    let ids = agents(16);
    let started = Instant::now();
    let mut calls = Vec::new();
    let mut completed = Vec::new();
    let mut cancelled = false;
    let result = prepare_cohort((), &ids, Instant::now, async |_, id| {
        calls.push(id.clone());
        if id == &ids[6] {
            let _guard = MarkCancelled(&mut cancelled);
            return pending().await;
        }
        sleep(Duration::from_secs(9)).await;
        completed.push(id.clone());
        Ok(())
    })
    .await;
    assert_eq!(
        result.unwrap_err(),
        "Agent log cohort preparation exceeded 60 seconds"
    );
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert_eq!(calls, ids[..7]);
    assert_eq!(completed, ids[..6]);
    assert!(cancelled);
}

#[tokio::test(start_paused = true)]
async fn refuses_a_cohort_larger_than_sixteen_before_starting_work() {
    let result = prepare_cohort(
        (),
        &agents(17),
        || panic!("oversize cohort must not start the clock"),
        async |_, _| panic!("oversize cohort must not start preparation"),
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        "Agent log admission cohort exceeds preparation budget"
    );
}
