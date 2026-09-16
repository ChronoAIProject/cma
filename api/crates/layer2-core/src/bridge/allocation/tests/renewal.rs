use super::*;
use crate::bridge::*;
use crate::resource_access::*;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

struct Organizations(OrganizationRole);
#[async_trait::async_trait]
impl OrganizationAuthority for Organizations {
    async fn organization_context(
        &self,
        caller: &crate::Caller,
        _: &str,
        org: &crate::SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut context = context(caller.subject.as_str());
        context.memberships = vec![OrganizationMembership::new(
            context.credential.clone(),
            org.clone(),
            self.0,
            SystemTime::now(),
            SystemTime::now() + Duration::from_secs(30),
        )
        .unwrap()];
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &crate::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}
struct Connections;
#[async_trait::async_trait]
impl BridgeConnectionValidator for Connections {
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        unreachable!()
    }
}

pub(super) fn context(subject: &str) -> CallerContext {
    let subject = crate::SubjectId::new(subject);
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: crate::Caller::new(
            subject.clone(),
            crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://nyx.example".into(),
            subject: subject.clone(),
            credential_id: "fresh-sign-in".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: Vec::new(),
    }
}

pub(super) async fn manager(
    bridge: BridgeRecord,
    actor: &str,
    role: OrganizationRole,
) -> Result<BridgeManagementLease, BridgeError> {
    let store = Arc::new(MemoryBridgeStore::default());
    store
        .apply(BridgeWrite {
            expected_revision: 0,
            record: bridge.clone(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
            custody: None,
            enable_grant: None,
            receipt: StoredBridgeReceipt {
                id: "fixture".into(),
                command_digest: "a".repeat(64),
                receipt: BridgeReceipt {
                    bridge_id: bridge.id.clone(),
                    owner: bridge.owner.clone(),
                    actor: bridge.created_by.clone(),
                    revision: bridge.revision,
                    config_revision: bridge.config_revision,
                    command: "create".into(),
                    applied_at: bridge.created_at.clone(),
                },
            },
        })
        .await
        .unwrap();
    BridgeService::new(store, Arc::new(Organizations(role)), Arc::new(Connections))
        .management_lease(
            &context(actor),
            Some("fresh-oauth"),
            &bridge.id,
            bridge.revision,
        )
        .await
}

fn observation(record: &AllocationRecord) -> AllocationCapabilityObservation {
    AllocationCapabilityObservation {
        owner: record.owner.clone(),
        profiles_digest: record.profiles_digest().unwrap().into(),
        capacity_bounds: crate::profiles::ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        owner_live_sandbox_limit: 3,
        baseline_services: Vec::new(),
        one_sandbox_workspace_worktree_agent_available: true,
        runtime_model_services_skills_available: true,
        deadline: tokio::time::Instant::now() + Duration::from_secs(15),
    }
}

fn organization_record() -> (AllocationRecord, BridgeRecord) {
    let (mut record, authority) = reserved();
    let mut bridge = authority.bridge().clone();
    let org = ResourceOwner::Organization {
        subject: crate::SubjectId::new("organization"),
    };
    record.owner = org.clone();
    bridge.owner = org;
    (record, bridge)
}

#[tokio::test]
async fn new_current_admin_renews_without_rewriting_original_template_approval() {
    let (record, bridge) = organization_record();
    let management = manager(bridge, "new-admin", OrganizationRole::Admin)
        .await
        .unwrap();
    let plan = AllocationProfilePlan::renew_for_record(&management, &record, observation(&record))
        .unwrap();
    let AllocationPlacement::NewSandbox { profiles, .. } = &record.placement else {
        unreachable!()
    };
    assert_eq!(plan.profiles(), profiles.as_ref());
    assert_ne!(plan.profiles().approved_by, *management.actor());
    assert!(manager(
        management.record().clone(),
        "new-admin",
        OrganizationRole::Member
    )
    .await
    .is_err());
}

#[tokio::test(start_paused = true)]
async fn expired_manager_and_suspended_original_generation_cannot_renew() {
    let (record, mut bridge) = organization_record();
    let management = manager(bridge.clone(), "new-admin", OrganizationRole::Admin)
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(matches!(
        AllocationProfilePlan::renew_for_record(&management, &record, observation(&record)),
        Err(AllocationError::AuthorityUnavailable)
    ));
    bridge.state = BridgeState::Disabled {
        accepted_work: AcceptedWorkPolicy::SuspendOriginal,
    };
    let management = manager(bridge, "new-admin", OrganizationRole::Admin)
        .await
        .unwrap();
    assert!(matches!(
        AllocationProfilePlan::renew_for_record(&management, &record, observation(&record)),
        Err(AllocationError::AuthorityUnavailable)
    ));
}

#[tokio::test]
async fn renewal_refuses_owner_profile_and_live_capability_substitution() {
    let (record, bridge) = organization_record();
    let management = manager(bridge, "new-admin", OrganizationRole::Admin)
        .await
        .unwrap();
    let mut wrong = observation(&record);
    wrong.owner = ResourceOwner::Person {
        subject: crate::SubjectId::new("new-admin"),
    };
    assert!(matches!(
        AllocationProfilePlan::renew_for_record(&management, &record, wrong),
        Err(AllocationError::InvalidProfiles)
    ));
    let mut wrong = observation(&record);
    wrong.runtime_model_services_skills_available = false;
    assert!(AllocationProfilePlan::renew_for_record(&management, &record, wrong).is_err());
    let mut wrong = record.clone();
    if let AllocationPlacement::NewSandbox { profiles, .. } = &mut wrong.placement {
        profiles.agent.content.system_prompt = "replacement".into();
    }
    assert!(
        AllocationProfilePlan::renew_for_record(&management, &wrong, observation(&record)).is_err()
    );
}
