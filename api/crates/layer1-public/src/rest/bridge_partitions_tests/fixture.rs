//! Inert source evidence with real management/resource policy and HTTP routes.
use super::super::{agents_tests, bridge_cleanup_tests, bridges_tests::fixture::Ports};
pub(in crate::rest) use bridge_cleanup_tests::fixture::context;
use layer2_core::{
    bridge::{allocation::*, ingress::*, partition::*, status::*, *},
    resource_access::*,
    sandbox::{store::*, AgentStatus},
    SubjectId,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

pub(in crate::rest) struct Memory {
    pub evidence: Mutex<PartitionEvidence>,
    pub calls: AtomicUsize,
    pub revoke: AtomicBool,
    pub ports: Arc<Ports>,
    pub activity: Mutex<PartitionActivityEvidence>,
}
#[async_trait::async_trait]
impl BridgePartitionStore for Memory {
    async fn list(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        _: u32,
    ) -> Result<Vec<PartitionEvidence>, PartitionStatusError> {
        manager.require_current()?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let value = self.evidence.lock().unwrap().clone();
        Ok(
            if after.is_some_and(|id| id >= value.allocation.id.as_str()) {
                vec![]
            } else {
                vec![value]
            },
        )
    }
    async fn detail(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<PartitionEvidence>, PartitionStatusError> {
        manager.require_current()?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let value = self.evidence.lock().unwrap().clone();
        Ok((value.allocation.id == id).then_some(value))
    }
    async fn activity(
        &self,
        manager: &BridgeManagementLease,
        _: &AllocationRecord,
    ) -> Result<PartitionActivityEvidence, PartitionStatusError> {
        manager.require_current()?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.revoke.load(Ordering::SeqCst) {
            self.ports.role.store(1, Ordering::SeqCst);
        }
        Ok(self.activity.lock().unwrap().clone())
    }
}
pub(in crate::rest) struct Fixture {
    pub router: axum::Router,
    pub bridge: BridgeId,
    pub allocation: String,
    pub memory: Arc<Memory>,
    pub resources: Arc<InMemorySandboxStore>,
}

pub(in crate::rest) async fn fixture(organization: bool) -> Fixture {
    fixture_for(organization, false).await
}

pub(in crate::rest) async fn fixture_for(organization: bool, foreign_target: bool) -> Fixture {
    let base = bridge_cleanup_tests::fixture::fixture().await;
    let ports = Arc::new(Ports {
        role: AtomicUsize::new(2),
        calls: AtomicUsize::new(0),
        validation: AtomicUsize::new(1),
    });
    let stores = Arc::new(MemoryBridgeStore::default());
    let definitions = BridgeService::new(stores.clone(), ports.clone(), ports.clone());
    let owner = if organization {
        ResourceOwner::Organization {
            subject: SubjectId::new("org-partition"),
        }
    } else {
        ResourceOwner::Person {
            subject: SubjectId::new("owner-a"),
        }
    };
    let mut record = base.memory.records[0].clone();
    record.owner = owner.clone();
    definitions
        .mutate(
            &context("owner-a"),
            Some("request-credential"),
            BridgeMutation {
                id: base.bridge.clone(),
                expected_revision: 0,
                idempotency_key: "cleanup-http-fixture".into(),
                change: BridgeChange::Create {
                    owner: owner.clone(),
                    config: Box::new(record.config.clone()),
                },
            },
        )
        .await
        .unwrap();
    let bridge = stores.bridge(&base.bridge).await.unwrap().unwrap();
    let inbox = source(&record);
    let normalized = NormalizedPartitionSource::from_stored(&inbox, &bridge).unwrap();
    let PartitionAdmissionDecision::Admitted { key, .. } =
        normalized.admit(&record.config).unwrap()
    else {
        panic!("accepted fixture source")
    };
    record.identity.key = key;
    record.id = record.identity.id().unwrap();
    record.operation_id = format!("{}:provision:v1", record.id);
    record.first_source = PartitionSourcePin::for_inbox(&inbox).unwrap();
    let suffix = |kind| {
        layer2_core::profiles::content_digest(&("cma.bridge.resource.v1", &record.id, kind))
            .unwrap()[..32]
            .to_owned()
    };
    let AllocationPlacement::ReuseParent { target, .. } = &mut record.placement else {
        panic!("reused fixture")
    };
    target.worktree_id = layer2_core::WorktreeId(format!("wkt_{}", suffix("worktree")));
    target.agent_id = layer2_core::AgentId(format!("agt_{}", suffix("agent")));
    let ids = target.clone();
    let receipt = record.receipt.as_mut().unwrap();
    receipt.operation_id = record.operation_id.clone();
    receipt.target = ids.clone();
    record.validate().unwrap();
    let resources = Arc::new(InMemorySandboxStore::default());
    let subject = match &owner {
        ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
    };
    let resource_subject = if foreign_target {
        SubjectId::new("another-owner")
    } else {
        subject.clone()
    };
    let sandbox = agents_tests::sandbox(ids.sandbox_id.as_str(), resource_subject.as_str());
    resources.create_sandbox(sandbox.clone()).await.unwrap();
    resources
        .create_workspace(agents_tests::workspace(
            ids.workspace_id.as_str(),
            ids.sandbox_id.as_str(),
            resource_subject.as_str(),
        ))
        .await
        .unwrap();
    let mut agent = agents_tests::seeded_agent(ids.agent_id.as_str(), AgentStatus::Ready, 0, None);
    agent.sandbox_id = ids.sandbox_id;
    agent.workspace_id = ids.workspace_id;
    agent.worktree_id = ids.worktree_id;
    agent.owner = resource_subject;
    resources
        .create_worktree(agents_tests::seeded_worktree(&agent))
        .await
        .unwrap();
    resources.create_agent(agent).await.unwrap();
    if organization {
        resources.set_resource_policy_for_test(ResourcePolicyRecord {
            sandbox_id: sandbox.id,
            sandbox_created_at: sandbox.created_at.clone(),
            owner: owner.clone(),
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "https://identity.test".into(),
                organization: subject.clone(),
                admitted_by: context("owner-a").actor,
                verified_at: sandbox.created_at,
            }),
            revision: 1,
        });
    }
    let memory = Arc::new(Memory {
        evidence: Mutex::new(PartitionEvidence {
            allocation: record.clone(),
            current: true,
            present: vec![
                PartitionResource::Sandbox,
                PartitionResource::Workspace,
                PartitionResource::Worktree,
                PartitionResource::Agent,
            ],
            first_source: Some(inbox),
            observed_at_ms: 1000,
        }),
        calls: AtomicUsize::new(0),
        revoke: AtomicBool::new(false),
        ports: ports.clone(),
        activity: Mutex::new(PartitionActivityEvidence {
            pending: vec![],
            latest: None,
            observed_at_ms: 1000,
        }),
    });
    let service = BridgePartitionService::new(
        definitions,
        Arc::new(ResourceAccessService::new(resources.clone(), ports)),
        memory.clone(),
    );
    Fixture {
        router: super::super::bridge_partitions_router(Some(Arc::new(service))),
        bridge: base.bridge,
        allocation: record.id,
        memory,
        resources,
    }
}

fn source(record: &AllocationRecord) -> StoredInbox {
    let mut inbox: StoredInbox = serde_json::from_value(json!({
        "id":"2".repeat(64),"owner":record.owner,"pin":record.pin,"bot_id":"bot","key_id":"key","message_id":"message-fixture","correlation_id":"correlation-fixture",
        "envelope":{"platform":"telegram","route_id":"route","chat_id":"-100123","conversation_type":"group","sender_id":"12345","sender_display_name":"  Sender\nOne  ","content_type":"text","text":"private fixture input","attachments":[],"reply_to_message_id":null,"reply_to_platform_message_id":null,"protected_thread_context_present":false,"timestamp":"2026-09-15T06:00:00Z"},
        "attestation":{"issuer":"https://identity.test","signing_key_id":"signing-fixture","token_id":"private-token-marker","issued_at":100,"expires_at":200,"body_sha256":"3".repeat(64)},
        "relay":{"kind":"absent_inbox_only"},"admission":"accepted","capabilities":null,"execution_hold":null,"accepted_at":101,"expires_at":3601,"reply_expires_at":null
    })).unwrap();
    inbox.capabilities = Some(serde_json::from_value(json!({"version":1,"contract":"nyxid_platform_v1","source":record.first_source,"message_id":inbox.message_id,"provider_message_id":"42","platform":"telegram","chat_id":"-100123","sender_id":"12345","conversation":"group","sender_kind":"human","event_kind":"user_message","loop_evidence":"not_loop","mentioned_bot":"unknown","replied_to_bot":"unknown","thread":{"kind":"missing"}})).unwrap());
    inbox
}
