//! Actual public media HTTP routing through two AMs and the Mongo owner fence.
use super::*;
use crate::agent_manager::relay::{PublicAccess, Relay};
use layer2_core::agent_ownership::{
    execution_authority, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};

struct OwnedFiles {
    files: Arc<Files>,
    instance: String,
    observed: Arc<Mutex<Vec<String>>>,
}
#[async_trait::async_trait]
impl AttachmentWriter for OwnedFiles {
    async fn write(
        &self,
        sandbox: &Sandbox,
        attachment: &AgentAttachment,
        bytes: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        let owner = execution_authority().expect("native media requires captured owner");
        assert!(owner.is_current());
        assert_eq!(owner.token.instance_id, self.instance);
        assert_eq!(owner.token.agent_id, attachment.agent_id);
        self.observed.lock().unwrap().push(self.instance.clone());
        self.files.write(sandbox, attachment, bytes).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn nonowner_public_media_upload_and_continuation_commit_on_exact_owner_once() {
    let f = Fixture::new("media_public_peers", false).await;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut peers = Vec::new();
    let mut servers = Vec::new();
    let mut owner = None;
    for index in 0..2 {
        let instance = format!("media-peer-{index}");
        let store = Arc::new(
            f.store
                .as_ref()
                .clone()
                .enable_agent_ownership(instance.clone())
                .await
                .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            store.agent_ownership.as_ref().unwrap().clone(),
            Member {
                instance_id: instance.clone(),
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
        )
        .await
        .unwrap();
        if index == 0 {
            owner = Some(coordinator.acquire(&f.agent.id).await.unwrap().unwrap());
        }
        let media = MediaContinuations::new(
            AgentCommandService::new(store.clone(), f.access.clone(), f.auth.clone()),
            store.clone(),
            store.clone(),
            f.access.clone(),
            f.auth.clone(),
            AgentAttachmentService::with_writer(
                f.access.clone(),
                Arc::new(OwnedFiles {
                    files: f.files.clone(),
                    instance,
                    observed: observed.clone(),
                }),
            ),
            f.service.gateway.as_str(),
        )
        .unwrap();
        media.install_ownership(coordinator.clone()).unwrap();
        let updates = layer2_core::sandbox::DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"media-peer-test-key"),
        )
        .unwrap();
        let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
        relay
            .install_public_access(
                PublicAccess::new(
                    f.auth.clone(),
                    AgentReadService::new(store, Arc::new(Resources))
                        .with_resource_access(f.access.clone()),
                    f.access.clone(),
                    Arc::new(adapter),
                )
                .with_media(Some(media.clone())),
            )
            .unwrap();
        let app = relay.authenticated_routes(binding_media_router(media), f.auth.clone());
        servers.push(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        peers.push((address, coordinator, updates, relay));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let token = f.caller.credential.as_ref().unwrap().value();
    let upload_url = format!("{}{}{}", peers[1].0, f.base(), "/attachment");
    let mut receipts = Vec::new();
    for _ in 0..2 {
        let response = client
            .post(&upload_url)
            .bearer_auth(token)
            .query(&[
                ("name", f.artifact.name.clone()),
                ("sha256", f.artifact.source_sha256.clone()),
                ("event_expires_at", f.expiry.to_string()),
            ])
            .header("content-type", "application/octet-stream")
            .body(f.bytes.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        receipts.push(response.json::<AgentAttachment>().await.unwrap());
    }
    assert_eq!(receipts[0], receipts[1]);
    let body = f.supplement(&receipts[0], "through nonowner media peer");
    let payload = json!({"gateway_binding_generation":body.gateway_binding_generation,
        "event_expires_at":body.event_expires_at,"artifact":body.artifact,"derived_text":body.derived_text});
    let mut commands = Vec::new();
    for index in [1, 0] {
        let response = client
            .post(format!("{}{}/supplement", peers[index].0, f.base()))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        commands.push(response.json::<Value>().await.unwrap());
    }
    f.peer.active.store(false, Ordering::SeqCst);
    let revoked = client
        .post(format!("{}{}/supplement", peers[1].0, f.base()))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert!(matches!(
        revoked.status(),
        StatusCode::FORBIDDEN | StatusCode::CONFLICT
    ));
    let count = f
        .store
        .agent_commands
        .count_documents(doc! {})
        .await
        .unwrap();
    let writes = f.files.writes.load(Ordering::SeqCst);
    let instances = observed.lock().unwrap().clone();
    let still_current = owner.unwrap().is_current();
    for server in servers {
        server.abort();
        let _ = server.await;
    }
    drop(peers);
    f.close().await;
    assert!(still_current);
    assert_eq!(commands[0]["command_id"], commands[1]["command_id"]);
    assert_eq!(commands[0]["response_id"], commands[1]["response_id"]);
    assert_eq!(count, 2);
    assert_eq!(writes, 1);
    assert_eq!(instances, vec!["media-peer-0"]);
}
