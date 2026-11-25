struct RevokingRuntime {
    peer: Arc<Peer>,
    at: usize,
    resolutions: AtomicUsize,
    outage: bool,
    sender: layer3_adapter::ConversationInputSender,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for RevokingRuntime {
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        unreachable!()
    }
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        if self.resolutions.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
            if self.outage {
                self.peer.outage.store(true, Ordering::SeqCst);
            } else {
                self.peer.active.store(false, Ordering::SeqCst);
            }
        }
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: self.sender.clone(),
        })
    }
}
fn runtime(f: &Fixture, at: usize, outage: bool) -> (Arc<RevokingRuntime>, Arc<AtomicUsize>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let count = sends.clone();
    let sender = layer3_adapter::ConversationInputSender::spawn(4, move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    (
        Arc::new(RevokingRuntime {
            peer: f.peer.clone(),
            outage,
            at,
            resolutions: AtomicUsize::new(0),
            sender,
        }),
        sends,
    )
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_source_revocation_at_ordered_writer_blocks_runtime_input() {
    let f = Fixture::new("media_writer_revoke", false).await;
    let receipt = f.upload().await.unwrap();
    let admitted = f
        .service
        .supplement(
            f.caller.clone(),
            f.path.clone(),
            f.supplement(&receipt, "late transcript"),
        )
        .await
        .unwrap();
    f.store
        .reject_unclaimed_command(
            &f.parent.receipt.envelope.command_id,
            f.parent.revision,
            CommandFailureCode::AgentUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let claimed = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "media-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.receipt.envelope.command_id,
        admitted.command.receipt.command_id
    );
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &claimed.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    authority.current_snapshot().await.unwrap();
    let (runtime, sends) = runtime(&f, 2, false);
    let result = AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &claimed)
        .await;
    assert!(result.is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let saved = f
        .store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.receipt, claimed.receipt);
    assert!(saved.work.dispatch.is_some());
    let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    assert_eq!(
        agent.conversation.operation.unwrap().state,
        AgentOperationDelivery::PreWriteFailed
    );
    assert_eq!(agent.status, AgentStatus::Ready);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_append_preserves_active_original_turn() {
    let f = Fixture::new("media_active_original", false).await;
    let original = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "original-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &original.receipt.envelope)
        .await
        .unwrap();
    let (runtime, sends) = runtime(&f, usize::MAX, false);
    AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &original)
        .await
        .unwrap();
    let active = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    assert_eq!(active.status, AgentStatus::Working);
    let receipt = f.upload().await.unwrap();
    let accepted = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), f.supplement(&receipt, ""))
        .await
        .unwrap();
    let supplement = f
        .store
        .get_command(&accepted.command.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(supplement.status, AgentResponseStatus::Queued);
    assert!(matches!(
        supplement.receipt.envelope.command,
        AgentCommand::Message {
            order: AgentInputOrder::Append,
            ..
        }
    ));
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap().unwrap(),
        active
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

async fn claimed_continuation_fixture(label: &str) -> (Fixture, AdmittedAgentCommand) {
    let f = Fixture::new(label, false).await;
    let receipt = f.upload().await.unwrap();
    f.service
        .supplement(
            f.caller.clone(),
            f.path.clone(),
            f.supplement(&receipt, "retry transcript"),
        )
        .await
        .unwrap();
    f.store
        .reject_unclaimed_command(
            &f.parent.receipt.envelope.command_id,
            f.parent.revision,
            CommandFailureCode::AgentUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let claimed = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "retry-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    (f, claimed)
}

async fn deferred_fixture(label: &str) -> (Fixture, AdmittedAgentCommand) {
    let (f, claimed) = claimed_continuation_fixture(label).await;
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &claimed.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    let (runtime, sends) = runtime(&f, 2, true);
    assert!(AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &claimed)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    (f, claimed)
}
