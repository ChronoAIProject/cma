use super::{
    fake_nyxid::console::barrier::ResponseGate, shared_support::Cluster, support::csrf_for,
};
use cmaeg::console::authority::Denied;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_refresh_waiters_are_bounded_without_blocking_a_fresh_session() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let sid = a.login(&a, &cluster, "alice").await;
    let bob = a.login(&a, &cluster, "bob").await;
    cluster.due(&sid).await;
    let gate = ResponseGate::new(false);
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = Some(gate.clone());
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..17 {
        let http = a.http.clone();
        let url = format!("{}/console", a.base);
        let cookie = format!("__Host-cmaeg-console={sid}");
        requests.spawn(async move {
            http.get(url)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
                .status()
        });
    }
    gate.reached().await;
    let refused = tokio::time::timeout(Duration::from_secs(3), requests.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        refused, 503,
        "the seventeenth refresh waiter is refused immediately"
    );
    assert!(tokio::time::timeout(Duration::from_secs(3), a.page(&bob))
        .await
        .unwrap()
        .contains("Signed in as …bob"));
    gate.release();
    while let Some(result) = requests.join_next().await {
        assert_eq!(result.unwrap(), 200);
    }
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    a.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_refresh_is_single_flight_and_rotates_on_the_next_attempt() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let b = cluster.replica().await;
    let sid = a.login(&b, &cluster, "alice").await;
    for expected in 1..=2 {
        cluster.due(&sid).await;
        let gate = ResponseGate::new(false);
        *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = Some(gate.clone());
        let first = a.get(&sid);
        let second = b.get(&sid);
        let release = async {
            gate.reached().await;
            assert_eq!(cluster.nyxid.oauth.refresh_calls(), expected);
            gate.release();
        };
        let (first, second, ()) = tokio::join!(first, second, release);
        assert_eq!(first.status(), 200);
        assert_eq!(second.status(), 200);
        assert!(first.text().await.unwrap().contains("Signed in as …lice"));
        assert!(second.text().await.unwrap().contains("Signed in as …lice"));
        assert_eq!(cluster.nyxid.oauth.refresh_calls(), expected);
        *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = None;
    }
    let calls = cluster.nyxid.oauth.token_calls.lock().unwrap().clone();
    let tokens: Vec<_> = calls
        .iter()
        .filter_map(|call| call.refresh_token.as_ref())
        .collect();
    assert_ne!(tokens[0], tokens[1]);
    a.close().await;
    b.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_logout_wins_over_a_late_successful_refresh() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let b = cluster.replica().await;
    let sid = a.login(&b, &cluster, "alice").await;
    let csrf = csrf_for(&b.page(&sid).await, "/console/logout");
    cluster.due(&sid).await;
    let gate = ResponseGate::new(false);
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = Some(gate.clone());
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    let console = a.console.clone();
    let task = tokio::spawn(async move { console.authority().authorize(&cell).await });
    gate.reached().await;
    assert_eq!(b.logout(&sid, &csrf).await.status(), 303);
    gate.release();
    assert_eq!(task.await.unwrap(), Err(Denied::SignedOut));
    assert!(cluster.row(&sid).await.is_none());
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = None;
    let c = cluster.replica().await;
    assert!(c.page(&sid).await.contains("Sign in to the console"));
    tokio::time::timeout(Duration::from_secs(3), async {
        while cluster.nyxid.oauth.revoked.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("old and unused successor token cleanup");
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    a.close().await;
    b.close().await;
    c.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_consumed_token_response_loss_never_replays_on_replacement() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let sid = a.login(&a, &cluster, "alice").await;
    cluster.due(&sid).await;
    let gate = ResponseGate::new(true);
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = Some(gate.clone());
    let response = a.get(&sid);
    let release = async {
        gate.reached().await;
        gate.release();
    };
    let (response, ()) = tokio::join!(response, release);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("Sign in to the console"));
    a.close().await;
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = None;
    let c = cluster.replica().await;
    assert!(c.page(&sid).await.contains("Sign in to the console"));
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    c.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_dead_refresh_owner_does_not_reopen_its_consumed_token() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let sid = a.login(&a, &cluster, "alice").await;
    cluster.due(&sid).await;
    let gate = ResponseGate::new(false);
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = Some(gate.clone());
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    let console = a.console.clone();
    let task = tokio::spawn(async move { console.authority().authorize(&cell).await });
    gate.reached().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gate.release();
    a.close().await;
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = None;
    let c = cluster.replica().await;
    assert!(c.page(&sid).await.contains("Sign in to the console"));
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    assert!(cluster.row(&sid).await.is_none());
    c.close().await;
    cluster.close().await;
}
