use super::*;
use crate::agent_manager::relay::{tests::public_routes, Relay};
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
};
use layer1_public::responses::v2::{router, ResponsesV2State};
use layer2_core::{
    agent_ownership::{candidates, Member, OwnershipCoordinator, PROTOCOL_VERSION},
    resource_access::{
        CallerContext, CredentialContext, CredentialSource, MembershipError, OrganizationAuthority,
        OrganizationChoice, ResourceAccessService, ResourceActor,
    },
};
use serde_json::{json, Value};
use std::num::NonZeroU32;

struct Authority;

#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }

    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}

#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        _: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        assert_eq!(credential, "fixture-caller");
        assert!(context.validate_principal());
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "synthetic-command-authority".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for Authority {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("queued cancellation must not read runtime resources")
    }
}

struct Replica {
    coordinator: Arc<OwnershipCoordinator>,
    address: String,
    _updates: Arc<DurableAgentUpdateRecorder>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Replica {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Replica {
    async fn start(a: &Accepted, count: usize) -> Self {
        let id = format!("cancel-replica-{count}");
        let store = Arc::new(
            (*a.f.resources)
                .clone()
                .enable_agent_ownership(id.clone())
                .await
                .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            store.agent_ownership.clone().unwrap(),
            Member {
                instance_id: id,
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
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-cancel-relay-fixture"),
        )
        .unwrap();
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(Authority),
        ));
        let reads = AgentReadService::new(store.clone(), Arc::new(Authority))
            .with_resource_access(access.clone());
        let commands = AgentCommandService::new(store, access, Arc::new(Authority))
            .with_ownership(Some(coordinator.clone()));
        let n = |value| NonZeroU32::new(value).unwrap();
        let database = a.f.seed.client.database(&a.f.seed.database);
        let limits = crate::rate_limit::RateLimiters::from_config(
            crate::config::RateLimitConfig {
                admission_per_minute: n(100),
                health_per_minute: n(100),
                default_per_minute: n(100),
                create_per_minute: n(100),
                input_per_minute: n(100),
                control_per_minute: n(3),
                control_tier_enabled: true,
                response_input_tier_enabled: true,
                default_lease_enabled: false,
                anonymous_public: crate::config::AnonymousPublicConfig {
                    enabled: false,
                    per_minute: n(100),
                    concurrency: n(16),
                },
            },
            crate::config::TrustedProxyConfig::sample(),
            Arc::new(
                crate::store::request_limits::MongoRequestLimits::open(&database)
                    .await
                    .unwrap(),
            ),
        );
        let responses = router(ResponsesV2State::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://agents.example").unwrap(),
            commands,
            reads,
        ))
        .layer(middleware::from_fn_with_state(
            limits,
            crate::rate_limit::enforce,
        ));
        let owner = a.agent.owner.clone();
        let app =
            public_routes(&relay, responses).layer(middleware::from_fn(
                move |mut request: Request, next: Next| {
                    let subject = owner.clone();
                    async move {
                        if request
                            .headers()
                            .get("authorization")
                            .is_some_and(|value| value == "Bearer fixture-caller")
                        {
                            let authorization = Authorization::new([Capability::UseOwnSandboxes]);
                            request
                                .extensions_mut()
                                .insert(layer1_public::AuthenticatedSubject(subject.clone()));
                            request.extensions_mut().insert(authorization.clone());
                            request.extensions_mut().insert(
                                layer1_public::DelegatedCredential::new("fixture-caller".into()),
                            );
                            request.extensions_mut().insert(CallerContext {
                                caller: Caller::new(subject.clone(), authorization),
                                actor: ResourceActor::Human {
                                    subject: subject.clone(),
                                },
                                credential_source: CredentialSource::Retained,
                                credential: CredentialContext {
                                    issuer: "synthetic".into(),
                                    subject,
                                    credential_id: "cancellation-test".into(),
                                },
                                memberships: Vec::new(),
                            });
                        }
                        next.run(request).await
                    }
                },
            ));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            coordinator,
            address,
            _updates: updates,
            server,
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn queued_trigger_cancellation_reaches_owner_and_replays_across_replicas() {
    for segmented in [false, true] {
        let a = if segmented {
            super::segmented::accepted().await
        } else {
            Accepted::new().await
        };
        let replicas = [Replica::start(&a, 0).await, Replica::start(&a, 1).await];
        let membership = replicas[0].coordinator.store.membership().await.unwrap();
        let preferred = candidates(&a.agent.id, &membership, None).unwrap();
        let owner = replicas
            .iter()
            .position(|replica| replica.coordinator.instance_id == preferred[0].instance_id)
            .unwrap();
        let peer = 1 - owner;
        assert!(replicas[owner]
            .coordinator
            .acquire(&a.agent.id)
            .await
            .unwrap()
            .is_some());
        assert!(replicas[peer]
            .coordinator
            .acquire(&a.agent.id)
            .await
            .unwrap()
            .is_none());

        let path = format!(
            "/api/v2/agents/{}/responses/{}/queued-cancellation",
            a.agent.id,
            a.command.receipt.response_id.as_ref().unwrap()
        );
        let body = json!({
            "source_command_id": a.command.receipt.envelope.command_id,
            "expected_command_revision": a.command.revision,
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let submit = |replica: usize| {
            client
                .post(format!("{}{path}", replicas[replica].address))
                .header("idempotency-key", "cancel-trigger-once")
                .json(&body)
        };
        assert_eq!(
            submit(peer).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let first = submit(peer)
            .bearer_auth("fixture-caller")
            .send()
            .await
            .unwrap();
        let status = first.status();
        let result: Value = first.json().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{result:?}");
        assert_eq!(result["response"]["status"], "cancelled");
        for replica in [owner, peer] {
            let replay = submit(replica)
                .bearer_auth("fixture-caller")
                .send()
                .await
                .unwrap();
            assert_eq!(replay.status(), StatusCode::OK);
            assert_eq!(replay.json::<Value>().await.unwrap(), result);
        }
        assert_eq!(
            submit(peer)
                .bearer_auth("fixture-caller")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );

        let agent = a.current_agent().await;
        assert_eq!(
            agent.conversation.last_seq,
            a.agent.conversation.last_seq + 1
        );
        assert!(agent.conversation.operation.is_none());
        let rows = a.f.resources.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(
                    row.kind,
                    AgentLogKind::FirstMessageChanged {
                        state: FirstMessageDelivery::Failed,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(!rows.iter().any(|row| matches!(
            row.kind,
            AgentLogKind::OwnerMessage { .. } | AgentLogKind::TurnStarted { .. }
        )));
        assert_eq!(
            a.f.resources
                .agent_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            a.current_command().await.status,
            AgentResponseStatus::Cancelled
        );
        drop(replicas);
        a.f.close().await;
    }
}
