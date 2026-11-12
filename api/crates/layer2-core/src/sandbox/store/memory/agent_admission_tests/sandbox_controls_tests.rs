use super::*;
mod platform_tests;
mod worker_tests;
use crate::resource_access::*;
use crate::sandbox::agent_io::{
    AgentAuthorizationRevision, AgentCommandAdmissionAuthority, CommandAuthorizationError,
};
use crate::sandbox::control::*;
use crate::sandbox::store::PageRequest;
use crate::{Authorization, Caller, Capability};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes, Capability::DeleteOwnSandbox]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "credential-a".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    }
}

struct Membership(Mutex<Result<OrganizationRole, MembershipError>>);
#[async_trait::async_trait]
impl OrganizationAuthority for Membership {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!("choices do not authorize commands")
    }
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let role = self.0.lock().unwrap().clone()?;
        let mut current = context(caller.subject.as_str());
        current.caller = caller.clone();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            org.clone(),
            role,
            SystemTime::now(),
            SystemTime::now() + Duration::from_secs(3600),
        )?);
        Ok(current)
    }
}
struct Custody;
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Custody {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(AgentAuthorizationRevision {
            authority_id: "fixture-sign-in".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
struct Fixture {
    store: Arc<InMemorySandboxStore>,
    access: Arc<ResourceAccessService>,
    service: Arc<SandboxControlService>,
    membership: Arc<Membership>,
}
impl Fixture {
    async fn new() -> Self {
        let store = Arc::new(ready_store(4).await);
        {
            let mut state = store.lock();
            let root = state.sandboxes.get_mut(&sandbox(4).id).unwrap();
            let live = crate::sandbox::lifecycle_test_support::sandbox();
            root.runtime = live.runtime;
            root.credential = live.credential;
        }
        let membership = Arc::new(Membership(Mutex::new(Ok(OrganizationRole::Admin))));
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            membership.clone(),
        ));
        let service = SandboxControlService::new(store.clone(), access.clone(), Arc::new(Custody));
        Self {
            store,
            access,
            service,
            membership,
        }
    }
    async fn admit(
        &self,
        actor: &str,
        command: SandboxControl,
        key: &str,
        revision: u64,
    ) -> Result<SandboxControlRecord, SandboxControlServiceError> {
        self.service
            .admit(
                &context(actor),
                "fixture",
                SandboxControlSubmission {
                    sandbox_id: sandbox(4).id,
                    command,
                    expected_revision: revision,
                    idempotency_key: key.into(),
                },
            )
            .await
    }
    async fn proof(
        &self,
        actor: &str,
        record: &SandboxControlRecord,
    ) -> Arc<SandboxControlAuthorization> {
        self.access
            .authorize_sandbox_control(&context(actor), "fixture", record)
            .await
            .unwrap()
    }
    async fn share(&self, mode: ShareMode, narrow: Option<AgentId>) -> ShareMutationReceipt {
        self.access
            .change_share(
                &context("owner"),
                Some("fixture"),
                ShareCommand {
                    selector: narrow.map_or(
                        ResourceSelector::Sandbox {
                            sandbox_id: sandbox(4).id,
                        },
                        |agent_id| ResourceSelector::Agent { agent_id },
                    ),
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode,
                    },
                    expected_share_revision: None,
                    idempotency_key: "fixture-share".into(),
                },
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn sandbox_controls_concurrent_acceptance_and_application_preserve_live_tree() {
    let f = Fixture::new().await;
    f.store.admit_agent(admission("keep-agent")).await.unwrap();
    let before = f.store.lock().clone();
    let revision = before.sandboxes[&sandbox(4).id].revision;
    let (left, right) = tokio::join!(
        f.admit("owner", SandboxControl::Pause, "retry", revision),
        f.admit("owner", SandboxControl::Pause, "retry", revision)
    );
    let record = left.unwrap();
    assert_eq!(record, right.unwrap());
    assert!(matches!(
        f.admit("owner", SandboxControl::Revive, "other", revision)
            .await,
        Err(SandboxControlServiceError::Store(SandboxControlError::Busy))
    ));
    let proof = f.proof("owner", &record).await;
    let (left, right) = tokio::join!(
        f.store.apply_sandbox_control(&record, proof.clone()),
        f.store.apply_sandbox_control(&record, proof)
    );
    let applied = left.unwrap();
    assert_eq!(right, Err(SandboxControlError::Conflict));
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied {
            disposition: SandboxControlDisposition::Scheduled,
            ..
        }
    ));
    assert_eq!(
        f.admit("owner", SandboxControl::Pause, "retry", revision)
            .await
            .unwrap(),
        applied
    );
    assert!(matches!(
        f.admit("owner", SandboxControl::Pause, "retry", revision + 1)
            .await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    let after = f.store.lock();
    let root = &after.sandboxes[&sandbox(4).id];
    assert_eq!(root.status, SandboxStatus::Pausing);
    assert_eq!(root.runtime, before.sandboxes[&root.id].runtime);
    assert_eq!(root.profile, before.sandboxes[&root.id].profile);
    assert_eq!(root.credential, before.sandboxes[&root.id].credential);
    assert_eq!(root.owner, before.sandboxes[&root.id].owner);
    assert_eq!(after.agents, before.agents);
    assert_eq!(after.agent_logs, before.agent_logs);
    assert_eq!(after.worktrees, before.worktrees);
    assert_eq!(after.workspaces, before.workspaces);
    assert_eq!(
        after.agent_creation_provenance,
        before.agent_creation_provenance
    );
    assert_eq!(
        after.sandbox_events[&root.id].len(),
        before.sandbox_events.get(&root.id).map_or(0, Vec::len) + 1
    );
    assert_eq!(after.sandbox_controls.len(), 1);
}

#[tokio::test]
async fn sandbox_controls_shared_role_matrix_and_retirement_boundary() {
    for mode in [ShareMode::View, ShareMode::OrgRoles] {
        for role in [
            OrganizationRole::Admin,
            OrganizationRole::Member,
            OrganizationRole::Viewer,
        ] {
            for command in [
                SandboxControl::Pause,
                SandboxControl::Revive,
                SandboxControl::Retire,
            ] {
                let f = Fixture::new().await;
                f.share(mode, None).await;
                *f.membership.0.lock().unwrap() = Ok(role);
                let result = f.admit("recipient", command, "matrix", 1).await;
                let allowed = mode == ShareMode::OrgRoles
                    && role == OrganizationRole::Admin
                    && command != SandboxControl::Retire;
                assert_eq!(result.is_ok(), allowed, "{mode:?} {role:?} {command:?}");
                if let Ok(record) = result {
                    let proof = f.proof("recipient", &record).await;
                    let applied = f.store.apply_sandbox_control(&record, proof).await.unwrap();
                    assert_eq!(applied.envelope.actor, context("recipient").actor);
                    assert_eq!(applied.admission_audit.actor, context("recipient").actor);
                    assert_eq!(
                        applied.execution_audit.unwrap().actor,
                        context("recipient").actor
                    );
                    assert_eq!(
                        f.store.lock().sandboxes[&sandbox(4).id].owner,
                        SubjectId::new("owner")
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn sandbox_controls_org_admin_can_retire_but_members_and_operators_cannot() {
    for role in [
        OrganizationRole::Admin,
        OrganizationRole::Member,
        OrganizationRole::Viewer,
    ] {
        let f = Fixture::new().await;
        let root = f.store.lock().sandboxes[&sandbox(4).id].clone();
        f.store.lock().resource_policies.insert(
            root.id.clone(),
            ResourcePolicyRecord {
                sandbox_id: root.id,
                sandbox_created_at: root.created_at,
                owner: ResourceOwner::Organization {
                    subject: root.owner.clone(),
                },
                organization_evidence: Some(OrganizationOwnershipEvidence {
                    issuer: "https://identity.test".into(),
                    organization: root.owner,
                    admitted_by: context("admin").actor,
                    verified_at: NOW.into(),
                }),
                revision: 1,
            },
        );
        *f.membership.0.lock().unwrap() = Ok(role);
        let result = f
            .admit("admin", SandboxControl::Retire, "org-retire", 1)
            .await;
        assert_eq!(result.is_ok(), role == OrganizationRole::Admin);
        if let Ok(record) = result {
            f.store
                .apply_sandbox_control(&record, f.proof("admin", &record).await)
                .await
                .unwrap();
            assert_eq!(
                f.store.lock().sandboxes[&sandbox(4).id].status,
                SandboxStatus::Retiring
            );
        }
    }
    let f = Fixture::new().await;
    let mut operator = context("operator");
    operator.caller.authorization = Authorization::new([Capability::ControlAnySandbox]);
    for command in [SandboxControl::Pause, SandboxControl::Retire] {
        let result = f
            .service
            .admit(
                &operator,
                "fixture",
                SandboxControlSubmission {
                    sandbox_id: sandbox(4).id,
                    command,
                    expected_revision: 1,
                    idempotency_key: "operator".into(),
                },
            )
            .await;
        assert_eq!(result.is_ok(), command == SandboxControl::Pause);
    }
    let mut owner = context("owner");
    owner.caller.authorization = Authorization::new([Capability::UseOwnSandboxes]);
    assert!(f
        .service
        .admit(
            &owner,
            "fixture",
            SandboxControlSubmission {
                sandbox_id: sandbox(4).id,
                command: SandboxControl::Retire,
                expected_revision: 1,
                idempotency_key: "no-delete-capability".into()
            }
        )
        .await
        .is_err());
}

#[tokio::test]
async fn sandbox_controls_narrow_grants_cannot_control_parent() {
    let f = Fixture::new().await;
    let agent = created(f.store.admit_agent(admission("narrow")).await.unwrap()).agent;
    f.share(ShareMode::View, Some(agent.id)).await;
    for command in [
        SandboxControl::Pause,
        SandboxControl::Revive,
        SandboxControl::Retire,
    ] {
        assert!(f.admit("recipient", command, "narrow", 2).await.is_err());
    }
    assert!(f.store.lock().sandbox_controls.is_empty());
}

#[tokio::test]
async fn sandbox_controls_revocation_fences_held_proof_and_hides_receipt() {
    let f = Fixture::new().await;
    let share = f.share(ShareMode::OrgRoles, None).await;
    let record = f
        .admit("recipient", SandboxControl::Pause, "revoke", 1)
        .await
        .unwrap();
    let proof = f.proof("recipient", &record).await;
    f.access
        .change_share(
            &context("owner"),
            Some("fixture"),
            ShareCommand {
                selector: share.share.selector,
                change: ShareChange::Revoke {
                    share_id: share.share.id,
                },
                expected_share_revision: Some(share.share.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.apply_sandbox_control(&record, proof).await,
        Err(SandboxControlError::Conflict)
    );
    assert!(f
        .admit("recipient", SandboxControl::Pause, "revoke", 1)
        .await
        .is_err());
    assert!(f
        .service
        .read(
            &context("recipient"),
            "fixture",
            &record.envelope.command_id
        )
        .await
        .is_err());
    assert_eq!(
        f.store.lock().sandboxes[&sandbox(4).id].status,
        SandboxStatus::Active
    );
}

#[tokio::test]
async fn sandbox_controls_current_membership_required_for_replay_and_execution() {
    for membership in [
        Err(MembershipError::NotMember),
        Err(MembershipError::Unavailable),
        Ok(OrganizationRole::Member),
    ] {
        let f = Fixture::new().await;
        f.share(ShareMode::OrgRoles, None).await;
        let record = f
            .admit("recipient", SandboxControl::Pause, "membership", 1)
            .await
            .unwrap();
        *f.membership.0.lock().unwrap() = membership;
        assert!(f
            .admit("recipient", SandboxControl::Pause, "membership", 1)
            .await
            .is_err());
        assert!(f
            .access
            .authorize_sandbox_control(&context("recipient"), "fixture", &record)
            .await
            .is_err());
        assert_eq!(
            f.store.lock().sandbox_controls[&record.envelope.command_id],
            record
        );
    }
}

#[tokio::test(start_paused = true)]
async fn sandbox_controls_original_membership_and_queue_deadlines_do_not_renew() {
    let f = Fixture::new().await;
    f.share(ShareMode::OrgRoles, None).await;
    let record = f
        .admit("recipient", SandboxControl::Pause, "expiry", 1)
        .await
        .unwrap();
    let proof = f.proof("recipient", &record).await;
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(
        f.store.apply_sandbox_control(&record, proof).await,
        Err(SandboxControlError::Conflict)
    );
    let mut short = record.clone();
    short.queue_deadline = crate::rfc3339_at(SystemTime::now() + Duration::from_secs(2));
    f.store
        .lock()
        .sandbox_controls
        .insert(short.envelope.command_id.clone(), short.clone());
    let proof = f.proof("recipient", &short).await;
    tokio::time::advance(Duration::from_secs(3)).await;
    assert_eq!(
        f.store.apply_sandbox_control(&short, proof).await,
        Err(SandboxControlError::Conflict)
    );
    assert_eq!(
        f.store.lock().sandboxes[&sandbox(4).id].status,
        SandboxStatus::Active
    );
}

#[tokio::test]
async fn sandbox_controls_stale_target_and_terminal_cas_do_not_schedule_effects() {
    let f = Fixture::new().await;
    assert!(matches!(
        f.admit("owner", SandboxControl::Pause, "stale", 2).await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::Conflict
        ))
    ));
    let record = f
        .admit("owner", SandboxControl::Pause, "change", 1)
        .await
        .unwrap();
    f.store
        .lock()
        .sandboxes
        .get_mut(&sandbox(4).id)
        .unwrap()
        .revision += 1;
    let proof = f.proof("owner", &record).await;
    let rejected = f.store.apply_sandbox_control(&record, proof).await.unwrap();
    assert!(matches!(
        rejected.status,
        SandboxControlStatus::Rejected {
            reason: SandboxControlFailure::TargetChanged,
            ..
        }
    ));
    assert_eq!(
        f.store
            .reject_sandbox_control(&record, SandboxControlFailure::AuthorityRevoked)
            .await,
        Err(SandboxControlError::Conflict)
    );
    assert!(f
        .store
        .queued_sandbox_controls(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        f.store.lock().sandboxes[&sandbox(4).id].status,
        SandboxStatus::Active
    );
}

#[tokio::test]
async fn sandbox_controls_forged_audit_and_corrupt_discovery_are_refused() {
    let f = Fixture::new().await;
    let record = f
        .admit("owner", SandboxControl::Pause, "audit", 1)
        .await
        .unwrap();
    let proof = f.proof("owner", &record).await;
    let mut forged = record.clone();
    forged.admission_audit.policy_revision = 99;
    assert!(forged.validate().is_ok());
    assert_eq!(
        f.store.reserve_sandbox_control(forged, proof, Ok(())).await,
        Err(SandboxControlError::Conflict)
    );
    assert!(f
        .service
        .read(
            &context("unrelated"),
            "fixture",
            &record.envelope.command_id
        )
        .await
        .is_err());
    let mut corrupt = record.clone();
    corrupt.admission_audit.actor = context("fake-owner").actor;
    f.store
        .lock()
        .sandbox_controls
        .insert(corrupt.envelope.command_id.clone(), corrupt);
    assert_eq!(
        f.store.sandbox_control(&record.envelope.command_id).await,
        Err(SandboxControlError::Corrupt)
    );
    assert_eq!(
        f.store
            .queued_sandbox_controls(&PageRequest::default())
            .await,
        Err(SandboxControlError::Corrupt)
    );
}
