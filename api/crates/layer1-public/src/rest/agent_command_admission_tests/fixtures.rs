use super::*;
use crate::rest::instances_tests::MemoryOperationStore;

#[derive(Default)]
pub(super) struct Outer {
    inner: MemoryOperationStore,
    pub fail_complete: AtomicBool,
    pub recovery: AtomicBool,
}
#[async_trait::async_trait]
impl OperationStore for Outer {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        let result = self.inner.reserve(claim).await?;
        Ok(
            if result == OperationReservation::Pending && self.recovery.load(Ordering::SeqCst) {
                OperationReservation::RecoveryRequired
            } else {
                result
            },
        )
    }
    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        if self.fail_complete.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("fixture completion failure".into()));
        }
        self.inner.complete(claim, outcome).await
    }
}

pub(super) struct Authority {
    pub membership: Mutex<Result<OrganizationRole, MembershipError>>,
    pub custody_revoked: AtomicBool,
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "request-credential");
        let role = self.membership.lock().unwrap().clone()?;
        let mut value = context(caller.subject.as_str());
        let now = SystemTime::now();
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            role,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
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
        assert_eq!(credential, "request-credential");
        if self.custody_revoked.load(Ordering::SeqCst) {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: format!("fixture:{}", context.caller.subject),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

pub(super) struct Commands {
    pub inner: Arc<InMemorySandboxStore>,
    pub reserves: AtomicUsize,
    pub unknown: AtomicBool,
}
#[async_trait::async_trait]
impl AgentCommandStore for Commands {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.reserves.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.reserve_command(request).await?;
        if self.unknown.load(Ordering::SeqCst) {
            Err(CommandLedgerError::Unavailable)
        } else {
            Ok(result)
        }
    }
    async fn get_admitted_command(
        &self,
        scope: &str,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_admitted_command(scope, key).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
}

pub(super) struct Fixture {
    pub router: axum::Router,
    pub store: Arc<InMemorySandboxStore>,
    pub service: Arc<AgentCommandService>,
    pub commands: Arc<Commands>,
    pub outer: Arc<Outer>,
    pub authority: Arc<Authority>,
    pub access: Arc<ResourceAccessService>,
    pub grant: ResourceShare,
}
pub(super) async fn fixture() -> Fixture {
    let outer = Arc::new(Outer::default());
    let base = agents_tests::fixture_with_operations(outer.clone()).await;
    agents_tests::insert_agent(
        &base.store,
        agents_tests::seeded_agent("agt_common", AgentStatus::Working, 3, Some(3)),
        vec![AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        }],
    )
    .await;
    let authority = Arc::new(Authority {
        membership: Mutex::new(Ok(OrganizationRole::Admin)),
        custody_revoked: AtomicBool::new(false),
    });
    let access = Arc::new(ResourceAccessService::new(
        base.store.clone(),
        authority.clone(),
    ));
    let grant = access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: ResourceSelector::Sandbox {
                    sandbox_id: SandboxId("sbx_route".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::OrgRoles,
                },
                expected_share_revision: None,
                idempotency_key: "share".into(),
            },
        )
        .await
        .unwrap()
        .share;
    let commands = Arc::new(Commands {
        inner: base.store.clone(),
        reserves: AtomicUsize::new(0),
        unknown: AtomicBool::new(false),
    });
    let service = AgentCommandService::new(commands.clone(), access.clone(), authority.clone());
    Fixture {
        router: agent_router(base.state.with_commands(service.clone())),
        store: base.store,
        service,
        commands,
        outer,
        authority,
        access,
        grant,
    }
}

pub(super) fn request(
    subject: &str,
    route: &str,
    key: &str,
    body: Option<Value>,
) -> axum::http::Request<axum::body::Body> {
    let mut request = agents_tests::request(
        Method::POST,
        &format!("/api/v1/agents/agt_common/{route}"),
        subject,
        Some(key),
        body,
    );
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    request
}
pub(super) async fn records(test: &Fixture) -> Vec<AdmittedAgentCommand> {
    test.store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
}
pub(super) fn outer_claim(key: &str, body: &Value) -> OperationClaim {
    OperationClaim::new(
        SubjectId::new("admin"),
        "agent:input:agt_common",
        key,
        Sha256::digest(body.to_string().as_bytes()).into(),
        SystemTime::now() + Duration::from_secs(120),
        Duration::from_secs(86400),
    )
}
