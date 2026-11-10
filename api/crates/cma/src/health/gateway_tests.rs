//! Exercise the policy's direct-to-Pod URL with the real driver/pool. Keeping
//! the HTTP listener attached throughout models the independent NEG boundary;
//! this does not simulate the GKE controller or its health-check thresholds.
use super::{
    mongo_fixture::{DriverProbe, MongoPeer},
    *,
};

struct Endpoint {
    client: reqwest::Client,
    base: String,
    path: &'static str,
    server: tokio::task::JoinHandle<()>,
}

impl Endpoint {
    async fn start(peer: &MongoPeer) -> Self {
        let policy = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../infra/api/healthcheckpolicy.yaml"
        ));
        let paths: Vec<_> = policy
            .lines()
            .filter_map(|line| line.trim().strip_prefix("requestPath: "))
            .collect();
        assert_eq!(paths.len(), 1, "one direct backend health path");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(Arc::new(DriverProbe(peer.client.clone())));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_millis(500))
                .pool_max_idle_per_host(0)
                .build()
                .unwrap(),
            base,
            path: paths[0],
            server,
        }
    }

    async fn get(&self, path: &str) -> (StatusCode, String) {
        assert!(!self.server.is_finished(), "HTTP listener must stay alive");
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("cached health must respond without waiting on Mongo");
        (response.status(), response.text().await.unwrap())
    }

    async fn wait_for(&self, status: StatusCode) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (actual, body) = self.get(self.path).await;
                if actual == status {
                    return serde_json::from_str(&body).expect("dependency health JSON");
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("configured edge endpoint did not change dependency health")
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn configured_edge_probe_rejects_dead_mongo_and_recovers_on_same_listener() {
    let peer = MongoPeer::start().await;
    let endpoint = Endpoint::start(&peer).await;
    endpoint.wait_for(StatusCode::OK).await;
    peer.block(true);
    let healthy_pings = peer.ping_count();
    peer.wait_for(|| peer.ping_count() > healthy_pings).await;

    // The last success is still fresh while a new check waits. HTTP probes
    // themselves neither acquire Mongo connections nor wait for this ping.
    let pending_pings = peer.ping_count();
    let checkouts = peer.trace.lock().unwrap().checkout_started;
    tokio::time::timeout(Duration::from_millis(500), async {
        for _ in 0..20 {
            assert_eq!(endpoint.get(endpoint.path).await.0, StatusCode::OK);
        }
    })
    .await
    .unwrap();
    assert_eq!(peer.ping_count(), pending_pings);
    assert_eq!(peer.trace.lock().unwrap().checkout_started, checkouts);

    let failed = endpoint.wait_for(StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(failed["dependency"]["reason"], "deadline");
    assert_eq!(endpoint.get("/livez").await, (StatusCode::OK, "ok".into()));
    // Keep the endpoint attached while the next check waits. The previous
    // failure must stay unhealthy until a successful check completes.
    peer.wait_for(|| peer.ping_count() > pending_pings).await;
    assert_eq!(
        endpoint.get(endpoint.path).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    peer.block(false);
    let recovered = endpoint.wait_for(StatusCode::OK).await;
    assert_eq!(recovered["status"], "ready");
    assert!(recovered["dependency"]["reason"].is_null());
    drop(endpoint);
    peer.client.clone().shutdown().await;
}

#[tokio::test]
async fn configured_edge_probe_tolerates_checkout_recovery_before_deadline() {
    let peer = MongoPeer::start().await;
    let endpoint = Endpoint::start(&peer).await;
    endpoint.wait_for(StatusCode::OK).await;
    peer.wait_for(|| peer.in_use() == 0).await;
    peer.block(true);
    let client = peer.client.clone();
    let holder = tokio::spawn(async move { DriverProbe(client).ping().await });
    peer.wait_for(|| peer.in_use() == 1).await;
    let checkouts = peer.trace.lock().unwrap().checkout_started;
    // The one-slot pool is occupied by unrelated work when the next health
    // check begins; the edge still reads the recent completed success.
    peer.wait_for(|| peer.trace.lock().unwrap().checkout_started > checkouts)
        .await;
    assert_eq!(endpoint.get(endpoint.path).await.0, StatusCode::OK);
    let pings = peer.ping_count();
    peer.block(false);
    holder.await.unwrap().unwrap();
    peer.wait_for(|| peer.ping_count() > pings && peer.in_use() == 0)
        .await;
    assert_eq!(endpoint.get(endpoint.path).await.0, StatusCode::OK);
    assert_eq!(peer.trace.lock().unwrap().dropped, 0);
    drop(endpoint);
    peer.client.clone().shutdown().await;
}
