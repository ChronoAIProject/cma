use super::{
    shared_support::{id, Cluster, ORIGIN},
    support::{csrf_for, csrf_token},
};
use cmaeg::console::{authority::Denied, session::OwnerSelection};
use mongodb::bson::{doc, DateTime, Document};
use std::sync::atomic::Ordering;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_login_cross_replica_owner_csrf_logout_and_instance_loss() {
    let cluster = Cluster::new().await;
    cluster
        .nyxid
        .oauth
        .grant("org-a", "Org A", "alice", "admin");
    cluster
        .nyxid
        .oauth
        .grant("org-b", "Org B", "alice", "member");
    let a = cluster.replica().await;
    let b = cluster.replica().await;
    let sid = a.login(&b, &cluster, "alice").await;
    let page = a.page(&sid).await;
    assert!(page.contains("Signed in as …lice"));
    assert!(b.page(&sid).await.contains("Signed in as …lice"));
    let csrf = csrf_for(&page, "/console/owner");
    let before = cluster
        .row(&sid)
        .await
        .unwrap()
        .get_datetime("last_seen")
        .unwrap()
        .to_owned();
    assert_eq!(
        b.post(
            &sid,
            "/console/owner",
            "https://foreign.example",
            &[("csrf", &csrf), ("owner", "org-a")]
        )
        .await
        .status(),
        403
    );
    assert_eq!(
        *cluster
            .row(&sid)
            .await
            .unwrap()
            .get_datetime("last_seen")
            .unwrap(),
        before
    );
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    let key = cell.meta().csrf_key;
    for invalid in [
        csrf_token(&key, "/console/logout", ""),
        csrf_token(&key, "/console/owner", "other-record"),
    ] {
        assert_eq!(
            b.post(
                &sid,
                "/console/owner",
                ORIGIN,
                &[("csrf", &invalid), ("owner", "org-a")]
            )
            .await
            .status(),
            403
        );
    }
    assert_eq!(
        b.post(
            &sid,
            "/console/owner",
            ORIGIN,
            &[("csrf", &csrf), ("owner", "org-a")]
        )
        .await
        .status(),
        303
    );
    assert!(a
        .page(&sid)
        .await
        .contains("Channels of organization Org A."));
    assert_eq!(
        b.post(
            &sid,
            "/console/owner",
            ORIGIN,
            &[("csrf", &csrf), ("owner", "org-b")]
        )
        .await
        .status(),
        303
    );
    assert!(a
        .page(&sid)
        .await
        .contains("Channels of organization Org A."));
    let bob = a.login(&b, &cluster, "bob").await;
    assert_eq!(
        b.post(
            &bob,
            "/console/owner",
            ORIGIN,
            &[("csrf", &csrf), ("owner", "org-a")]
        )
        .await
        .status(),
        403
    );
    assert!(b
        .page(&bob)
        .await
        .contains("Channels of your personal account."));
    assert_eq!(
        b.console
            .sessions()
            .find_session(&bob)
            .await
            .unwrap()
            .unwrap()
            .meta()
            .selected,
        OwnerSelection::Personal
    );
    b.close().await;
    let c = cluster.replica().await;
    let page = c.page(&sid).await;
    assert!(page.contains("Channels of organization Org A."));
    let logout = csrf_for(&page, "/console/logout");
    cluster.nyxid.oauth.down.store(true, Ordering::SeqCst);
    let response = c.logout(&sid, &logout).await;
    assert_eq!(response.status(), 303);
    assert!(response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    assert!(cluster.row(&sid).await.is_none());
    assert_eq!(
        a.console.authority().authorize(&cell).await,
        Err(Denied::SignedOut)
    );
    assert!(a.page(&sid).await.contains("Sign in to the console"));
    assert!(c.page(&sid).await.contains("Sign in to the console"));
    a.close().await;
    c.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_logout_fences_an_identity_check_already_in_flight() {
    use super::fake_nyxid::console::barrier::ResponseGate;
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let b = cluster.replica().await;
    let sid = b.login(&b, &cluster, "alice").await;
    let csrf = csrf_for(&b.page(&sid).await, "/console/logout");
    let cell = a
        .console
        .sessions()
        .find_session(&sid)
        .await
        .unwrap()
        .unwrap();
    let gate = ResponseGate::new(false);
    *cluster.nyxid.oauth.identity_gate.lock().unwrap() = Some(gate.clone());
    let console = a.console.clone();
    let request = tokio::spawn(async move { console.authority().authorize(&cell).await });
    gate.reached().await;
    assert_eq!(b.logout(&sid, &csrf).await.status(), 303);
    gate.release();
    assert_eq!(request.await.unwrap(), Err(Denied::SignedOut));
    *cluster.nyxid.oauth.identity_gate.lock().unwrap() = None;
    let c = cluster.replica().await;
    assert!(c.page(&sid).await.contains("Sign in to the console"));
    a.close().await;
    b.close().await;
    c.close().await;
    cluster.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn shared_expiry_and_ciphertext_binding_fail_closed_without_ttl_cleanup() {
    let cluster = Cluster::new().await;
    let a = cluster.replica().await;
    let b = cluster.replica().await;
    let first = a.login(&a, &cluster, "alice").await;
    let second = b.login(&b, &cluster, "bob").await;
    let row = cluster.row(&first).await.unwrap();
    let payload = row.get_binary_generic("payload").unwrap();
    for token in cluster.nyxid.oauth.issued_tokens() {
        assert!(!payload
            .windows(token.len())
            .any(|window| window == token.as_bytes()));
    }
    assert!(!row.contains_key("subject"));
    let rows = cluster
        .db
        .collection::<Document>(cmaeg::console::session_backend::schema::COLLECTION);
    rows.update_one(
        doc! {"_id":id(&second)},
        doc! {"$set":{"payload":row.get("payload").unwrap().clone()}},
    )
    .await
    .unwrap();
    assert_eq!(
        b.get(&second).await.status(),
        503,
        "another session's ciphertext cannot be transplanted"
    );
    rows.update_one(
        doc! {"_id":id(&first)},
        doc! {"$set":{"absolute_at":DateTime::from_millis(1)}},
    )
    .await
    .unwrap();
    assert!(
        cluster.row(&first).await.is_some(),
        "physical row retained before expiry read"
    );
    assert!(b.page(&first).await.contains("Sign in to the console"));
    assert!(a
        .console
        .sessions()
        .find_session(&first)
        .await
        .unwrap()
        .is_none());
    a.close().await;
    b.close().await;
    cluster.close().await;
}
