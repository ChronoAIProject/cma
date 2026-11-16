use super::{
    fake_nyxid::console::barrier::ResponseGate,
    shared_faults::Faults,
    shared_support::{id, Cluster},
    support::csrf_for,
};
use cmaeg::console::{authority::Denied, session::OwnerSelection};
use mongodb::bson::{doc, DateTime, Document};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_lost_rotation_ack_and_unavailable_readback_preserves_published_token() {
    let cluster = Cluster::new().await;
    let faults = Faults::new(cluster.backend().await);
    let a = cluster
        .replica_with(faults.clone(), cluster.keys.clone())
        .await;
    let sid = a.login(&a, &cluster, "alice").await;
    cluster.due(&sid).await;
    faults.lose_completion.store(true, Ordering::SeqCst);
    faults.fail_readback.store(true, Ordering::SeqCst);
    assert_eq!(a.get(&sid).await.status(), 503);
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    assert!(cluster.nyxid.oauth.revoked.lock().unwrap().is_empty());
    let c = cluster.replica().await;
    assert!(c.page(&sid).await.contains("Signed in as …lice"));
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    a.close().await;
    c.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_lost_rotation_ack_reconciles_lineage_after_owner_metadata_changes() {
    let cluster = Cluster::new().await;
    cluster
        .nyxid
        .oauth
        .grant("org-a", "Org A", "alice", "admin");
    let faults = Faults::new(cluster.backend().await);
    let a = cluster
        .replica_with(faults.clone(), cluster.keys.clone())
        .await;
    let b = cluster.replica().await;
    let sid = a.login(&a, &cluster, "alice").await;
    cluster.due(&sid).await;
    let gate = ResponseGate::new(false);
    *faults.completion_gate.lock().unwrap() = Some(gate.clone());
    faults.lose_completion.store(true, Ordering::SeqCst);
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    let console = a.console.clone();
    let request = tokio::spawn(async move { console.authority().authorize(&cell).await });
    gate.reached().await;
    let peer = b
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    b.console
        .authority()
        .select(&peer, OwnerSelection::Organization("org-a".into()))
        .await
        .unwrap();
    gate.release();
    assert_eq!(request.await.unwrap().unwrap().owner(), "org-a");
    assert!(cluster.nyxid.oauth.revoked.lock().unwrap().is_empty());
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 1);
    a.close().await;
    b.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_uncertain_claim_never_sends_and_store_outage_never_reports_logout() {
    let cluster = Cluster::new().await;
    let faults = Faults::new(cluster.backend().await);
    let a = cluster
        .replica_with(faults.clone(), cluster.keys.clone())
        .await;
    let sid = a.login(&a, &cluster, "alice").await;
    let page = a.page(&sid).await;
    let csrf = csrf_for(&page, "/console/logout");
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    faults.unavailable.store(true, Ordering::SeqCst);
    let response = a.logout(&sid, &csrf).await;
    assert_eq!(response.status(), 503);
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(
        a.console.authority().authorize(&cell).await,
        Err(Denied::Unavailable)
    );
    faults.unavailable.store(false, Ordering::SeqCst);
    cluster.due(&sid).await;
    faults.lose_claim.store(true, Ordering::SeqCst);
    assert_eq!(a.get(&sid).await.status(), 503);
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 0);
    let c = cluster.replica().await;
    assert!(c.page(&sid).await.contains("Sign in to the console"));
    assert_eq!(cluster.nyxid.oauth.refresh_calls(), 0);
    a.close().await;
    c.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_refresh_deadline_is_anchored_before_delayed_provider_completion() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let sid = a.login(&a, &cluster, "alice").await;
    cluster.due(&sid).await;
    let gate = ResponseGate::new(false);
    *cluster.nyxid.oauth.refresh_gate.lock().unwrap() = Some(gate.clone());
    let request = a.get(&sid);
    let release = async {
        gate.reached().await;
        let claim = *cluster
            .row(&sid)
            .await
            .unwrap()
            .get_datetime("observed_at")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        gate.release();
        claim
    };
    let (response, claim) = tokio::join!(request, release);
    assert_eq!(response.status(), 200);
    let expires = *cluster
        .row(&sid)
        .await
        .unwrap()
        .get_datetime("access_expires_at")
        .unwrap();
    assert_eq!(
        expires.timestamp_millis(),
        claim.timestamp_millis() + 900_000
    );
    a.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_observation_delay_and_awaited_identity_cannot_extend_access_authority() {
    let cluster = Cluster::new().await;
    let faults = Faults::new(cluster.backend().await);
    let a = cluster
        .replica_with(faults.clone(), cluster.keys.clone())
        .await;
    let b = cluster.replica().await;
    let sid = b.login(&b, &cluster, "alice").await;
    let rows = cluster
        .db
        .collection::<Document>(cmaeg::console::session_backend::schema::COLLECTION);
    rows.update_one(doc!{"_id":id(&sid)},doc!{"$set":{"access_expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()+30_100)}}).await.unwrap();
    faults.observation_delay_ms.store(200, Ordering::SeqCst);
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !cell.lock_auth().await.access_usable(Instant::now()),
        "response delay must not reset local lifetime"
    );
    faults.observation_delay_ms.store(0, Ordering::SeqCst);
    rows.update_one(doc!{"_id":id(&sid)},doc!{"$set":{"access_expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()+90_000)}}).await.unwrap();
    let gate = ResponseGate::new(false);
    *cluster.nyxid.oauth.identity_gate.lock().unwrap() = Some(gate.clone());
    let console = a.console.clone();
    let request = tokio::spawn(async move { console.authority().authorize(&cell).await });
    gate.reached().await;
    rows.update_one(doc!{"_id":id(&sid)},doc!{"$set":{"access_expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()+31_000)}}).await.unwrap();
    gate.release();
    assert_eq!(request.await.unwrap(), Err(Denied::Unavailable));
    a.close().await;
    b.close().await;
    cluster.close().await;
}
