use super::*;
use layer2_core::resource_access::*;
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use mongodb::Client;
use std::time::{Duration, SystemTime};

pub(super) fn id(value: &str) -> wire::OpaqueId {
    wire::OpaqueId::new(value).unwrap()
}
pub(super) fn digest(value: char) -> wire::Digest {
    wire::Digest::new(&value.to_string().repeat(64)).unwrap()
}
pub(super) fn generation(value: u64) -> wire::Generation {
    wire::Generation::new(value).unwrap()
}

pub(super) fn context(subject: &str) -> CallerContext {
    let subject = SubjectId::new(subject);
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "synthetic".into(),
            subject: subject.clone(),
            credential_id: "synthetic".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: Vec::new(),
    }
}

pub(super) struct Organizations;
#[async_trait::async_trait]
impl OrganizationAuthority for Organizations {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            OrganizationRole::Admin,
            SystemTime::now(),
            SystemTime::now() + Duration::from_secs(60),
        )?);
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(Vec::new())
    }
}

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub app_name: String,
    pub sandbox: MongoSandboxStore,
    pub store: Arc<MongoResourceExecutionStore>,
    pub access: ResourceAccessService,
    pub coordinator: ResourceExecutionCoordinator,
    pub target: ResourceTarget,
    pub selector: ResourceSelector,
    pub share: ResourceShare,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_erasure_keys(false).await
    }

    pub(super) async fn with_erasure_keys(write_erasure_keys: bool) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("Mongo replica-set URI required");
        let name = format!("cma_execution_{}", uuid::Uuid::new_v4().simple());
        let app_name = name.clone();
        let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        let client = Client::with_options(options).unwrap();
        crate::store::migrations::prepare_bindings_for_test(&client.database(&name)).await;
        let (sandbox, target) =
            crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
                &client, &name,
            )
            .await;
        let snapshot = sandbox.resource_access_snapshot(&target).await.unwrap();
        let selector = ResourceSelector::Sandbox {
            sandbox_id: snapshot.lineage.sandbox.id.clone(),
        };
        let request = ShareMutation {
            selector: selector.clone(),
            owner: ResourceOwner::Person {
                subject: SubjectId::new("owner"),
            },
            actor: ResourceActor::Human {
                subject: SubjectId::new("owner"),
            },
            key_sha256: digest('a').as_str().into(),
            change: ShareChange::Create {
                target_org_id: SubjectId::new("synthetic-org"),
                mode: ShareMode::OrgRoles,
            },
            expected_share_revision: None,
            policy_fence: snapshot.fence(),
            proposed_id: "synthetic-share".into(),
            at: "2026-01-01T00:00:00Z".into(),
        };
        let share = sandbox.mutate_share(request).await.unwrap().share;
        sandbox.initialize_resource_execution_once().await.unwrap();
        sandbox
            .activate_resource_execution_after_drain(generation(1), &digest('d'))
            .await
            .unwrap();
        assert_eq!(
            sandbox.resource_execution_activation().await.unwrap(),
            ActivationStateV1::Activated {
                epoch: generation(1)
            }
        );
        let store = Arc::new(MongoResourceExecutionStore {
            sandbox: sandbox.clone(),
            collections: Collections::new(&sandbox),
            write_erasure_keys,
        });
        let policy = ResourceExecutionPolicy::enforce(store.clone(), true)
            .await
            .unwrap();
        let access = ResourceAccessService::new(Arc::new(sandbox.clone()), Arc::new(Organizations))
            .with_execution_policy(policy.clone());
        let coordinator = ResourceExecutionCoordinator::new(access.clone(), policy);
        Self {
            client,
            name,
            app_name,
            sandbox,
            store,
            access,
            coordinator,
            target,
            selector,
            share,
        }
    }

    pub async fn authority(&self, subject: &str) -> ExecutionAuthority {
        self.coordinator
            .authorize_execution(
                &context(subject),
                Some("synthetic"),
                &self.target,
                ResourceCapability::Interact,
                None,
            )
            .await
            .unwrap()
    }

    pub fn command(&self, key: &str) -> ShareCommand {
        ShareCommand {
            selector: self.selector.clone(),
            change: ShareChange::Revoke {
                share_id: self.share.id.clone(),
            },
            expected_share_revision: Some(self.share.revision),
            idempotency_key: key.into(),
        }
    }

    pub async fn freeze(&self) -> RevocationOperationV1 {
        self.coordinator
            .begin_revocation(&context("owner"), None, self.command("freeze"))
            .await
            .unwrap()
    }

    pub fn execution(&self, suffix: &str) -> wire::ExecutionKey {
        let ResourceTarget::Agent { agent_id } = &self.target else {
            panic!()
        };
        wire::ExecutionKey {
            command_id: id(suffix),
            command_fingerprint: digest('a'),
            dispatch_sequence: generation(1),
            prompt_id: id("prompt"),
            turn_id: id("turn"),
            agent_id: id(agent_id.as_str()),
            receiver: wire::ReceiverKey {
                kind: wire::ReceiverKind::Runtime,
                instance: id("runtime"),
                incarnation: digest('b'),
                boot: id("boot"),
            },
            attempt: generation(1),
        }
    }

    pub async fn register(&self, subject: &str, suffix: &str) -> ExecutionRecordV1 {
        self.coordinator
            .register_execution(&self.authority(subject).await, &self.execution(suffix))
            .await
            .unwrap()
    }

    pub async fn complete(&self, operation: &RevocationOperationV1, member: &ExecutionRecordV1) {
        let request = member.barrier_request(operation).unwrap();
        let barrier = wire::BarrierReceiptV1 {
            protocol: 1,
            request,
            barrier_sequence: generation(2),
            durable_watermark: operation.key.watermark,
        };
        self.coordinator
            .observe_barrier(member.member.receiver(), &barrier)
            .await
            .unwrap();
        let completion = match &member.member {
            wire::MemberKey::Execution { .. } => MemberCompletionV1::Execution {
                receipt: wire::StopReceiptV1 {
                    protocol: 1,
                    barrier,
                    stop_sequence: generation(3),
                    evidence: if member
                        .started
                        .as_ref()
                        .is_some_and(|start| start.state == wire::NativeStartState::Entered)
                    {
                        wire::StopEvidenceKind::CooperativeQuiescence
                    } else {
                        wire::StopEvidenceKind::RejectedBeforeStart
                    },
                    quiescence: wire::QuiescenceV1 {
                        queues_purged: true,
                        descendants_absent: true,
                        output_closed: true,
                        submitters_closed: true,
                        pending_local_effects_settled: true,
                        local_changes_digest: digest('c'),
                        effects: wire::EffectJournalV1 {
                            high_water: u64::from(member.effect_count),
                            digest: digest('e'),
                            unresolved_count: member.effect_count,
                        },
                    },
                },
            },
            wire::MemberKey::Release { .. } => MemberCompletionV1::Release {
                receipt: wire::ReleaseReceiptV1 {
                    protocol: 1,
                    barrier,
                    close_sequence: generation(3),
                    producers_closed: true,
                    pending_chunks_discarded: true,
                    transport_drained: true,
                    released_bytes: 0,
                },
            },
        };
        self.coordinator
            .observe_completion(member.member.receiver(), &completion)
            .await
            .unwrap();
    }

    pub async fn started_ticket(&self, suffix: &str) -> wire::ExecutionTicketV1 {
        self.started_ticket_for(&self.authority("recipient").await, suffix)
            .await
    }

    pub async fn started_ticket_for(
        &self,
        authority: &ExecutionAuthority,
        suffix: &str,
    ) -> wire::ExecutionTicketV1 {
        let execution = self.execution(suffix);
        let record = self
            .coordinator
            .register_execution(authority, &execution)
            .await
            .unwrap();
        let ticket = wire::ExecutionTicketV1 {
            protocol: 1,
            execution: execution.clone(),
            basis: authority.basis().clone(),
            operation: wire::ExecutionOperation::Prompt,
            input_digest: digest('a'),
            manager: wire::ManagerKey {
                instance: id("manager"),
                epoch: generation(1),
                agent_id: execution.agent_id.clone(),
            },
            challenge: wire::ReceiverChallengeV1 {
                protocol: 1,
                receiver: execution.receiver.clone(),
                nonce: id("challenge"),
                created_tick_ms: 100,
                deadline_tick_ms: 3_100,
            },
            nonce: id("ticket"),
            registration: record.registration,
        };
        self.coordinator
            .issue_ticket(authority, &ticket)
            .await
            .unwrap();
        self.coordinator
            .observe_start(
                &execution.receiver,
                &wire::StartObservationV1 {
                    protocol: 1,
                    execution: execution.clone(),
                    registration: ticket.registration.clone(),
                    nonce: ticket.nonce.clone(),
                    state: wire::NativeStartState::Entered,
                    sequence: generation(1),
                },
            )
            .await
            .unwrap();
        ticket
    }

    pub async fn ambiguous_commits(&self, enabled: bool) {
        let command = if enabled {
            doc! {
                "configureFailPoint":"failCommand", "mode":{"times":5_i32},
                "data":{"failCommands":["commitTransaction"], "appName":&self.app_name,
                    "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"synthetic ambiguous resource commit"}}
            }
        } else {
            doc! {"configureFailPoint":"failCommand","mode":"off"}
        };
        self.client
            .database("admin")
            .run_command(command)
            .await
            .unwrap();
    }

    pub async fn borrowed_binding(&self) -> layer2_core::sandbox::binding::ProviderBinding {
        use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
        use layer2_core::sandbox::binding::*;
        let value = serde_json::json!({
            "schema_version":1,
            "metadata":{"name":"synthetic-binding","owner":{"kind":"human","subject":"owner"},"provider":"registered"},
            "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,
                "api_type":"supported","model":"test","thinking_effort":"medium"},
            "permission":{"action_mode":"guarded"},"context":{},
            "environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}
        });
        let profile = map_profile(
            value.clone(),
            &format!("sha256:{}", content_digest(&value).unwrap()),
            AgentProvider::default(),
            "supported",
            ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
        )
        .unwrap();
        let mut binding = ProviderBinding::new(
            "synthetic-binding".into(),
            SubjectId::new("manager"),
            profile,
            digest('a').as_str().into(),
            "2026-01-01T00:00:00Z".into(),
        )
        .unwrap();
        let snapshot = self
            .sandbox
            .resource_access_snapshot(&self.target)
            .await
            .unwrap();
        let agent = snapshot.lineage.agent.unwrap();
        binding.sandbox_id = agent.sandbox_id;
        binding.workspace_id = agent.workspace_id;
        binding.existing_agent = Some(ExistingBindingAgent {
            agent_id: agent.id,
            worktree_id: agent.worktree_id,
        });
        binding.level = 1;
        binding.state = BindingState::Ready;
        binding.work = BindingWork::Idle;
        self.sandbox
            .save_binding(None, binding.clone())
            .await
            .unwrap();
        let conversation = BindingConversation::new(
            &binding,
            "existing-agent".into(),
            binding.created_at.clone(),
        )
        .unwrap();
        self.sandbox
            .save_binding_conversation(binding.revision, None, conversation)
            .await
            .unwrap();
        binding
    }

    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
