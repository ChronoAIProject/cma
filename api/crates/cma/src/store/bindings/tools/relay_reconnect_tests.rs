use super::*;

async fn claim(f: &Fixture, fence: u64) {
    let mut agent = f
        .store
        .get_agent(&f.conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.work.step = AgentWorkStep::PublishConnection;
    agent.work.next_fence = fence + 1;
    agent.work.lease = Some(AgentWorkLease {
        holder: "relay-reconnect".into(),
        fence,
        expires_at: layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        ),
    });
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
}

async fn reopen(f: &Fixture, fence: u64) -> HostedProviderToolBinding {
    let mut attachment = f
        .source
        .resolve(&RuntimeHandle("provider-runtime".into()), &f.request, fence)
        .await
        .unwrap()
        .unwrap();
    attachment
        .observe(layer3_adapter::ToolProxyStatus {
            request: attachment.scope().3.clone(),
            boot_id: "b".repeat(32),
        })
        .unwrap();
    f.source.confirm_open(&attachment, NATIVE).await.unwrap();
    attachment
}

async fn release(f: &Fixture, fence: u64) {
    let mut agent = f
        .store
        .get_agent(&f.conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent
        .work
        .advance(
            "relay-reconnect",
            fence,
            AgentWorkStep::None,
            layer2_core::now_rfc3339(),
        )
        .unwrap();
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_relay_reconnect_replays_pending_installation_without_claiming_it() {
    let f = Fixture::new().await;
    let old = MongoProviderToolSource::new(f.store.clone());
    let original = f.open(&old).await;
    old.confirm_open(&original, NATIVE).await.unwrap();
    let attached = f.open(&f.source).await;
    let agent = f.publish().await;
    f.idle_intent(agent).await;
    let prepared = f
        .source
        .prepare_transport(&attached, ORIGIN, NATIVE, 1)
        .await
        .unwrap();
    let capability = f.capability(&prepared);
    claim(&f, 2).await;
    let recovered = reopen(&f, 2).await;
    assert!(recovered.transport_origin().is_none());
    assert!(f
        .source
        .authenticate_relay(&capability, false)
        .await
        .is_err());
    assert!(f
        .source
        .confirm_transport(&prepared, NATIVE, 1)
        .await
        .is_err());
    let replay = f
        .source
        .prepare_transport(&recovered, ORIGIN, NATIVE, 1)
        .await
        .unwrap();
    assert!(replay.same_transport(&prepared));
    f.source
        .confirm_transport(&replay, NATIVE, 1)
        .await
        .unwrap();
    release(&f, 2).await;
    claim(&f, 3).await;
    let installed = reopen(&f, 3).await;
    release(&f, 3).await;
    assert!(installed.same_transport(&prepared));
    assert_eq!(installed.scope().0, original.scope().0);
    let authorized = f
        .source
        .authenticate_relay(&capability, false)
        .await
        .unwrap();
    assert_eq!(authorized.credential.expose(), ISSUER);
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_relay_rereads_credential_authority_after_a_concurrent_change() {
    let f = Fixture::new().await;
    let attachment = f.open(&f.source).await;
    f.source.confirm_open(&attachment, NATIVE).await.unwrap();
    f.publish().await;
    let capability = f.capability(&attachment);
    let mut competing = f.store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut competing)
        .await
        .unwrap();
    f.store
        .bindings
        .records
        .update_one(
            doc! {"_id":&f.binding.binding_id},
            doc! {"$set":{"record.key_sha256":"b".repeat(64)}},
        )
        .session(&mut competing)
        .await
        .unwrap();
    let (authentication, ()) =
        tokio::join!(f.source.authenticate_relay(&capability, false), async {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            competing.commit_transaction().await.unwrap();
        });
    assert!(
        authentication.is_err(),
        "a transaction retry must not release stale custody"
    );
    f.client.database(&f.database).drop().await.unwrap();
}
