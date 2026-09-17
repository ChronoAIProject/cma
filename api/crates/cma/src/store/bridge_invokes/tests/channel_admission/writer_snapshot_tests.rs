use super::*;

struct MovingConnection {
    inner: Arc<crate::store::bridge_invokes::tests::fixtures::Authority>,
    resources: Arc<crate::store::MongoSandboxStore>,
    agent_id: layer2_core::AgentId,
    replace_generation: bool,
    updates: AtomicUsize,
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for MovingConnection {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        let result = self.inner.validate_connection(owner, config).await?;
        // Inject a mutation exactly during the writer's authority refresh, after
        // the message and its dispatch fence have been committed together.
        let mut increment = doc! {"revision":1_i64,"record.revision":1_i64};
        if self.replace_generation {
            increment.insert("record.conversation.runtime_generation", 1_i64);
        }
        let updated = self.resources.agents.update_one(
            doc! {"_id":self.agent_id.as_str(),"record.conversation.operation.state":"dispatch_fenced"},
            doc! {"$inc":increment},
        ).await.unwrap();
        self.updates
            .fetch_add(updated.modified_count as usize, Ordering::SeqCst);
        Ok(result)
    }
}

async fn moving_fixture(replace_generation: bool) -> (ChannelFixture, Arc<MovingConnection>) {
    let mut c = ChannelFixture::new().await;
    let moving = Arc::new(MovingConnection {
        inner: c.f.authority.clone(),
        resources: c.f.resources.clone(),
        agent_id: c.f.target.agent_id.clone(),
        replace_generation,
        updates: AtomicUsize::new(0),
    });
    c.f.service = BridgeInvokeService::new(
        BridgeService::new(c.f.bridges.clone(), c.f.authority.clone(), moving.clone()),
        Arc::new(ResourceAccessService::new(
            c.f.resources.clone(),
            c.f.authority.clone(),
        )),
        c.f.resources.clone(),
        c.f.store.clone(),
    );
    (c, moving)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_writer_accepts_current_revision_after_concurrent_session_update() {
    let (c, moving) = moving_fixture(false).await;
    let record = held(&c).await;
    let (execution, sends) = execution(&c, false);
    let worker =
        AgentCommandWorker::new(c.f.resources.clone(), execution, "moving-revision".into())
            .unwrap();
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(moving.updates.load(Ordering::SeqCst), 1);
    assert_eq!(
        sends.load(Ordering::SeqCst),
        1,
        "ordinary session updates must not discard an accepted input"
    );
    let current =
        c.f.resources
            .get_command(&record.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.receipt, record.receipt);
    assert_eq!(current.status, AgentResponseStatus::InProgress);
    assert_eq!(
        current.work.observation.unwrap().delivery,
        AgentOperationDelivery::WriteConfirmed
    );
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(
        sends.load(Ordering::SeqCst),
        1,
        "reconciliation cannot write twice"
    );
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_writer_still_refuses_runtime_replacement_during_connection_check() {
    let (c, moving) = moving_fixture(true).await;
    let record = held(&c).await;
    let (execution, sends) = execution(&c, false);
    AgentCommandWorker::new(c.f.resources.clone(), execution, "moving-runtime".into())
        .unwrap()
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(moving.updates.load(Ordering::SeqCst), 1);
    assert_eq!(
        sends.load(Ordering::SeqCst),
        0,
        "an old dispatch must never write to a replacement runtime"
    );
    c.close().await;
}
