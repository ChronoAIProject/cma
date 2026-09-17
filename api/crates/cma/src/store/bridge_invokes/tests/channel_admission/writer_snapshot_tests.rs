use super::*;

#[derive(Clone, Copy, Debug)]
enum AdmissionMutation {
    SessionRevision,
    PolicyRevision,
    TargetIdentity,
}

struct MovingConnection {
    inner: Arc<crate::store::bridge_invokes::tests::fixtures::Authority>,
    resources: Arc<crate::store::MongoSandboxStore>,
    agent_id: layer2_core::AgentId,
    sandbox_id: layer2_core::SandboxId,
    replace_generation: bool,
    admission_mutation: Option<AdmissionMutation>,
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
        if let Some(mutation) = self.admission_mutation {
            // Mutate between the two resource reads used for admission, before
            // any command or runtime dispatch fence exists.
            let updated = match mutation {
                AdmissionMutation::SessionRevision => self
                    .resources
                    .agents
                    .update_one(
                        doc! {"_id":self.agent_id.as_str()},
                        doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
                    )
                    .await
                    .unwrap(),
                AdmissionMutation::PolicyRevision => self
                    .resources
                    .resource_policies
                    .update_one(
                        doc! {"_id":self.sandbox_id.as_str()},
                        doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
                    )
                    .await
                    .unwrap(),
                AdmissionMutation::TargetIdentity => self
                    .resources
                    .agents
                    .update_one(
                        doc! {"_id":self.agent_id.as_str()},
                        doc! {"$set":{"record.created_at":"2001-01-01T00:00:00Z"}},
                    )
                    .await
                    .unwrap(),
            };
            self.updates
                .fetch_add(updated.modified_count as usize, Ordering::SeqCst);
            return Ok(result);
        }
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
    install_moving_connection(ChannelFixture::new().await, replace_generation, None)
}

fn install_moving_connection(
    mut c: ChannelFixture,
    replace_generation: bool,
    admission_mutation: Option<AdmissionMutation>,
) -> (ChannelFixture, Arc<MovingConnection>) {
    let moving = Arc::new(MovingConnection {
        inner: c.f.authority.clone(),
        resources: c.f.resources.clone(),
        agent_id: c.f.target.agent_id.clone(),
        sandbox_id: c.f.target.sandbox_id.clone(),
        replace_generation,
        admission_mutation,
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
async fn channel_admission_refreshes_session_revision_but_keeps_policy_identity_and_binding_fences()
{
    for (mutation, binding) in [
        (AdmissionMutation::SessionRevision, false),
        (AdmissionMutation::PolicyRevision, false),
        (AdmissionMutation::TargetIdentity, false),
        (AdmissionMutation::SessionRevision, true),
    ] {
        let c = ChannelFixture::from_fixture(Fixture::organization().await).await;
        let (c, moving) = install_moving_connection(c, false, Some(mutation));
        let before =
            c.f.resources
                .get_agent(&c.f.target.agent_id)
                .await
                .unwrap()
                .unwrap();
        let effect = if binding {
            BridgeInvokeEffect::BindExistingTarget {
                allocation_id: c.allocation.id.clone(),
            }
        } else {
            BridgeInvokeEffect::AdmitMessage {
                inbox_id: c.inbox.id.clone(),
            }
        };
        let result = c.f.service.authorize(c.f.request(&c.grant, effect)).await;
        assert_eq!(moving.updates.load(Ordering::SeqCst), 1, "{mutation:?}");
        if matches!(mutation, AdmissionMutation::SessionRevision) && !binding {
            let proof = result.expect("session updates must not suspend message admission");
            assert_eq!(
                proof.resources().lineage.agent.as_ref().unwrap().revision,
                before.revision + 1
            );
            let reservation =
                ChannelCommandReservation::prepare(proof, c.inbox.clone(), c.allocation.clone())
                    .unwrap();
            let admitted = c
                .commands
                .reserve_channel_command(reservation)
                .await
                .unwrap();
            assert_eq!(admitted.status, AgentResponseStatus::WaitingAction);
        } else {
            assert!(
                matches!(result, Err(BridgeInvokeError::TargetMismatch)),
                "{mutation:?}, binding={binding}"
            );
        }
        c.close().await;
    }
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
