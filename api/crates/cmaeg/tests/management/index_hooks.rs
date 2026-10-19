//! Every record management writes or deletes, and every channel id the
//! resolver restores, is reported to the channel index observer.
use super::*;
use cmaeg::channels::observer::ChannelIndexObserver;
use cmaeg_core::ids::{ChannelId, RecordId};
use std::sync::Mutex;

#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

impl ChannelIndexObserver for Recording {
    fn changed(&self, record: &RecordId, owner: &str, version: u64) {
        self.0
            .lock()
            .unwrap()
            .push(format!("changed:{record}:{owner}:{version}"));
    }

    fn removed(&self, record: &RecordId) {
        self.0.lock().unwrap().push(format!("removed:{record}"));
    }

    fn observed(&self, record: &RecordId, id: &ChannelId) {
        self.0
            .lock()
            .unwrap()
            .push(format!("observed:{record}:{}", id.as_str()));
    }
}

#[tokio::test]
async fn create_update_rekey_delete_and_restores_are_reported() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let plain = management_state(&provider, &cma, &nyxid, key_ring(1));
    let recording = Arc::new(Recording::default());
    let state = ManagementState::observed(
        plain.providers.clone(),
        plain.records.clone(),
        plain.creator_auth.clone(),
        cmaeg::ManagementConfig {
            master_keys: plain.master_keys.clone(),
            public_url: plain.public_url.clone(),
            public_api_url: plain.public_url.clone(),
            cma_base_url: plain.cma_base_url.clone(),
            tools_nyxid_slug: plain.tools_nyxid_slug.clone(),
            cma_service_token: plain.cma_service_token.clone(),
            nyxid_base_url: nyxid.base_url.clone(),
        },
        cmaeg::ChannelCacheConfig::default(),
        recording.clone(),
    );
    let resolver = state.resolver.clone();
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    let id = initial["channel_id"].as_str().unwrap();
    let record = initial["record_id"].as_str().unwrap();
    let url = format!("{base}/v1/channels/{id}");
    resolver.resolve(id).await.unwrap();
    let mut update = create_body();
    update["expected_version"] = 1.into();
    update["name"] = "new-name".into();
    let response = client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = client
        .post(format!("{url}/rekey"))
        .bearer_auth("cma-service")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = client
        .delete(format!("{url}?expected_version=3"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(
        *recording.0.lock().unwrap(),
        [
            format!("changed:{record}:alice:1"),
            format!("observed:{record}:{id}"),
            format!("changed:{record}:alice:2"),
            format!("changed:{record}:alice:3"),
            format!("removed:{record}"),
        ]
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
