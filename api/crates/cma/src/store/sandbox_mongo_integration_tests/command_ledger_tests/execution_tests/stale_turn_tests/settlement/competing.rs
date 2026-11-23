use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipStore};
use layer3_adapter::{PromptResponse, TurnStopReason};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Race {
    Canonical,
    Interrupt,
    Successor,
    Oversize,
    Predecessor,
}

pub(super) fn history_rows(race: Option<Race>) -> u64 {
    if race == Some(Race::Predecessor) {
        10_000
    } else {
        6
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_seal_refuses_predecessor_before_fetching_its_payload() {
    Box::pin(exercise_settlement(true, None, Some(Race::Predecessor))).await;
}

#[derive(Clone, Default)]
pub(super) struct SealProbe(Arc<Mutex<SealRead>>);

#[derive(Default)]
struct SealRead {
    enabled: bool,
    request: Option<i32>,
    charged_bytes: Option<i32>,
    payload_present: bool,
}

impl SealProbe {
    pub(super) async fn client(&self) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let capture = self.0.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let mut trace = capture.lock().unwrap();
            if !trace.enabled {
                return;
            }
            match event {
                CommandEvent::Started(event)
                    if event.command.get_str("find").ok() == Some(segments::SEGMENTS)
                        && event.command.get_document("filter").is_ok_and(|filter| {
                            filter.get_i32("ordinal").is_ok() || filter.get_i64("ordinal").is_ok()
                        }) =>
                {
                    assert!(trace.request.replace(event.request_id).is_none());
                }
                CommandEvent::Succeeded(event) if trace.request == Some(event.request_id) => {
                    let batch = event
                        .reply
                        .get_document("cursor")
                        .unwrap()
                        .get_array("firstBatch")
                        .unwrap();
                    let row = batch[0].as_document().unwrap();
                    trace.charged_bytes = row.get_i32("proof_bytes").ok();
                    trace.payload_present =
                        row.contains_key("evidence") || row.contains_key("leaf_digests");
                }
                _ => {}
            }
        }));
        Client::with_options(options).unwrap()
    }
}

pub(super) struct Competition<'a> {
    pub client: &'a Client,
    pub peer: &'a Arc<MongoSandboxStore>,
    pub name: &'a str,
    pub scan: &'a StaleTurnStore,
    pub owner: &'a Arc<Authority>,
    pub proof: &'a StaleTurnSettlement,
    pub source: RuntimeOutputSource,
    pub budget_before: &'a mongodb::bson::Document,
    pub seal_probe: Option<SealProbe>,
}

impl Competition<'_> {
    pub(super) async fn run(self, race: Race, budget: &mut ProofBudget) {
        let agent = &self.proof.snapshot().agent;
        let command = self.proof.command();
        let id = &command.receipt.envelope.command_id;
        let (store, owner) = match race {
            Race::Successor => {
                let store = Arc::new(
                    MongoSandboxStore::open(self.client, self.name)
                        .await
                        .unwrap()
                        .enable_agent_ownership("test-successor".into())
                        .await
                        .unwrap(),
                );
                let ownership = store.agent_ownership.as_ref().unwrap();
                let manager = OwnershipCoordinator::register(
                    ownership.clone(),
                    Member {
                        instance_id: "test-successor".into(),
                        protocol: PROTOCOL_VERSION,
                        relay_address: "http://127.0.0.1:1".into(),
                        expires_at_ms: 0,
                        draining: false,
                        owned_agents: 0,
                        max_owned_agents: 1,
                    },
                )
                .await
                .unwrap();
                ownership
                    .heartbeat(&self.owner.token.instance_id, true)
                    .await
                    .unwrap();
                ownership.release(&self.owner.token).await.unwrap();
                let successor = manager.acquire(&agent.id).await.unwrap().unwrap();
                assert!(successor.token.epoch > self.owner.token.epoch);
                assert!(self.owner.is_current());
                (store, successor)
            }
            _ => (self.peer.clone(), self.owner.clone()),
        };
        if race == Race::Canonical {
            self.complete(&store, &owner, TurnStopReason::EndTurn).await;
        }
        if race == Race::Interrupt {
            let parent = self.proof.parent();
            let mut command_request = live_request(agent, parent, "stale-turn-interrupt");
            command_request.envelope.command = AgentCommand::Interrupt;
            command_request.envelope.request_fingerprint =
                AgentCommand::Interrupt.fingerprint().unwrap();
            command_request.response_id = None;
            command_request.queue_capacity = 8;
            let mut audit = super::super::super::super::audit_tests::personal_audit(agent, parent);
            audit.capability = layer2_core::resource_access::ResourceCapability::OperateAgent;
            audit.observations[0].capabilities = vec![audit.capability];
            command_request.admission_audit = Some(audit);
            let admitted = self
                .peer
                .reserve_targeted_control(TargetedControlReservation {
                    command: command_request,
                    target: TargetedResponse {
                        response_id: command.receipt.response_id.clone().unwrap(),
                        turn_sequence: agent.conversation.active_turn.unwrap(),
                    },
                    observed_agent_revision: agent.revision,
                    agent_created_at: agent.created_at.clone(),
                    retry_deadline: None,
                })
                .await
                .unwrap();
            assert_eq!(admitted.record.status, AgentResponseStatus::Queued);
        }
        const PREDECESSOR_ALLOWANCE: i64 = 96 * 1024;
        if let Some(probe) = &self.seal_probe {
            let head = self
                .client
                .database(self.name)
                .collection::<Document>(segments::HEADS)
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(head.get_i64("active_ordinal").unwrap(), 1);
            assert!(head.get_i64("active_bytes").unwrap() < 16 * 1024);
            budget
                .charge(budget.remaining() - PREDECESSOR_ALLOWANCE)
                .unwrap();
            probe.0.lock().unwrap().enabled = true;
        }
        let result = self
            .owner
            .run(self.scan.settle(self.proof, budget))
            .await
            .unwrap();
        if let Some(probe) = &self.seal_probe {
            let trace = probe.0.lock().unwrap();
            assert!(
                trace
                    .charged_bytes
                    .is_some_and(|bytes| i64::from(bytes) > PREDECESSOR_ALLOWANCE),
                "{result:?}"
            );
            assert!(!trace.payload_present);
        }
        let over_budget = matches!(race, Race::Oversize | Race::Predecessor);
        if over_budget {
            assert!(
                matches!(result, Err(SandboxStoreError::Unavailable(detail)) if detail.contains("proof byte budget"))
            );
            assert_eq!(budget.remaining(), 0);
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
        }
        let row = self
            .peer
            .agent_commands
            .find_one(doc! {"_id": id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("stale_turn_publication"));
        let after_refusal = self
            .peer
            .agent_log(&agent.id, agent.conversation.last_seq, 100)
            .await
            .unwrap();
        if over_budget {
            assert!(after_refusal.is_empty());
            assert_eq!(
                self.peer.get_command(id).await.unwrap().unwrap().status,
                AgentResponseStatus::InProgress
            );
            assert_eq!(&budget_turn(self.peer, agent).await, self.budget_before);
            return;
        }
        let (expected, stop_reason) = match race {
            Race::Canonical => (AgentResponseStatus::Completed, None),
            Race::Interrupt => (
                AgentResponseStatus::Cancelled,
                Some(TurnStopReason::Cancelled),
            ),
            Race::Successor => (
                AgentResponseStatus::Completed,
                Some(TurnStopReason::EndTurn),
            ),
            Race::Oversize | Race::Predecessor => unreachable!(),
        };
        if let Some(stop_reason) = stop_reason {
            assert!(after_refusal.is_empty());
            assert_eq!(&budget_turn(self.peer, agent).await, self.budget_before);
            self.complete(&store, &owner, stop_reason).await;
        }
        owner
            .run(async {
                assert_eq!(store.reconcile_command(id).await.unwrap().status, expected);
            })
            .await
            .unwrap();
        let events = self
            .peer
            .agent_log(&agent.id, agent.conversation.last_seq, 100)
            .await
            .unwrap();
        let terminals: Vec<_> = events
            .iter()
            .filter_map(|entry| match entry.kind {
                AgentLogKind::TurnCompleted { .. } => Some(AgentResponseStatus::Completed),
                AgentLogKind::TurnInterrupted { .. } => Some(AgentResponseStatus::Cancelled),
                AgentLogKind::TurnFailed { .. } => Some(AgentResponseStatus::Failed),
                _ => None,
            })
            .collect();
        assert_eq!(terminals, [expected]);
        let mut settled = self.budget_before.clone();
        settled
            .get_document_mut("reservation")
            .unwrap()
            .insert("settled", true);
        assert_eq!(budget_turn(self.peer, agent).await, settled);
    }

    async fn complete(
        &self,
        store: &Arc<MongoSandboxStore>,
        owner: &Arc<Authority>,
        stop: TurnStopReason,
    ) {
        let agent = &self.proof.snapshot().agent;
        let (acks, _unused) = tokio::sync::mpsc::channel(1);
        owner
            .run(async {
                let recorder = DurableAgentUpdateRecorder::new(store.clone());
                let update = ConversationUpdate::RuntimeOutput {
                    receipt: RuntimeOutputReceipt::new(self.source.clone(), acks),
                    update: Some(
                        Box::new(ConversationUpdate::TurnEnded(PromptResponse::new(stop))).into(),
                    ),
                };
                assert!(
                    recorder
                        .record_update(
                            &agent.id,
                            1,
                            &mut AgentMessageAssembly::default(),
                            update,
                            layer2_core::now_rfc3339(),
                        )
                        .await
                        .unwrap()
                        .turn_closed
                );
            })
            .await
            .unwrap();
    }
}
