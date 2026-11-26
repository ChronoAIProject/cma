use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::control::*;
use std::time::{Duration, SystemTime};

#[path = "sandbox_control_deadline_tests.rs"]
mod deadline;
#[path = "sandbox_control_operator_tests.rs"]
mod operator;
#[path = "sandbox_control_platform_tests.rs"]
mod platform;

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes, Capability::DeleteOwnSandbox]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    }
}
struct Membership(Duration);
#[async_trait::async_trait]
impl OrganizationAuthority for Membership {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
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
            SystemTime::now() + self.0,
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
            replay_principal: None,
            authority_id: "control-sign-in".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
struct Fixture {
    store: Arc<MongoSandboxStore>,
    service: Arc<SandboxControlService>,
    access: Arc<ResourceAccessService>,
    share: ShareMutationReceipt,
    parent: Sandbox,
    agent: Agent,
}
impl Fixture {
    async fn new(client: &Client, name: &str) -> Self {
        let (store, agent, parent) = fixture(name, client).await;
        let store = Arc::new(store);
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(Membership(MAX_MEMBERSHIP_AGE)),
        ));
        let share = access
            .change_share(
                &context("owner"),
                Some("fixture"),
                ShareCommand {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: parent.id.clone(),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::OrgRoles,
                    },
                    expected_share_revision: None,
                    idempotency_key: "share-control".into(),
                },
            )
            .await
            .unwrap();
        let service = SandboxControlService::new(store.clone(), access.clone(), Arc::new(Custody));
        Self {
            store,
            service,
            access,
            share,
            parent,
            agent,
        }
    }
    async fn admit(&self, key: &str) -> Result<SandboxControlRecord, SandboxControlServiceError> {
        self.service
            .admit(
                &context("recipient"),
                "fixture",
                SandboxControlSubmission {
                    sandbox_id: self.parent.id.clone(),
                    command: SandboxControl::Pause,
                    expected_revision: self.parent.revision,
                    idempotency_key: key.into(),
                },
            )
            .await
    }
    async fn proof(
        &self,
        record: &SandboxControlRecord,
        lifetime: Duration,
    ) -> Arc<SandboxControlAuthorization> {
        ResourceAccessService::new(self.store.clone(), Arc::new(Membership(lifetime)))
            .authorize_sandbox_control(&context("recipient"), "fixture", record)
            .await
            .unwrap()
    }
    async fn documents(&self) -> (Document, Vec<Document>, Vec<Document>, Vec<Document>) {
        (
            self.store
                .sandboxes
                .find_one(doc! { "_id": self.parent.id.as_str() })
                .await
                .unwrap()
                .unwrap(),
            self.store
                .sandbox_controls
                .find(doc! {})
                .sort(doc! { "_id": 1 })
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
            self.store
                .sandbox_events
                .find(doc! {})
                .sort(doc! { "_id": 1 })
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
            self.store
                .agent_log
                .find(doc! {})
                .sort(doc! { "_id": 1 })
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        )
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_controls_reopen_and_concurrent_apply_keep_one_audited_effect() {
    let (client, name) = database("sandbox_controls_concurrent").await;
    let f = Fixture::new(&client, &name).await;
    let (left, right) = tokio::join!(f.admit("retry"), f.admit("retry"));
    let record = left.unwrap();
    assert_eq!(record, right.unwrap());
    let before = f.documents().await;
    assert!(matches!(
        f.admit("busy").await,
        Err(SandboxControlServiceError::Store(SandboxControlError::Busy))
    ));
    let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    let (left, right) = tokio::join!(
        f.store.apply_sandbox_control(&record, proof.clone()),
        replica.apply_sandbox_control(&record, proof)
    );
    let results = [left, right];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(SandboxControlError::Conflict))
            .count(),
        1
    );
    let applied = replica
        .sandbox_control(&record.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied {
            disposition: SandboxControlDisposition::Scheduled,
            ..
        }
    ));
    assert_eq!(
        applied.execution_audit.as_ref().unwrap().actor,
        context("recipient").actor
    );
    assert_eq!(f.admit("retry").await.unwrap(), applied);
    let root = replica.get_sandbox(&f.parent.id).await.unwrap().unwrap();
    assert_eq!(root.status, SandboxStatus::Pausing);
    assert_eq!(root.profile, f.parent.profile);
    assert_eq!(root.runtime, f.parent.runtime);
    assert_eq!(root.credential, f.parent.credential);
    assert_eq!(root.owner, f.parent.owner);
    assert_eq!(
        replica.get_agent(&f.agent.id).await.unwrap(),
        Some(f.agent.clone())
    );
    let after = f.documents().await;
    assert_eq!(after.1.len(), 1);
    assert_eq!(after.2.len(), before.2.len() + 1);
    assert_eq!(after.3, before.3);
    assert!(replica
        .queued_sandbox_controls(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_controls_transaction_fences_revision_policy_audit_and_receipt() {
    for change in ["revision", "policy", "audit", "receipt"] {
        let (client, name) = database("sandbox_controls_fence").await;
        let f = Fixture::new(&client, &name).await;
        let mut record = f.admit("fence").await.unwrap();
        let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
        match change {
            "revision" => {
                let mut next = f.parent.clone();
                next.revision += 1;
                f.store.save_sandbox(f.parent.revision, next).await.unwrap();
            }
            "policy" => {
                f.store
                    .resource_policies
                    .update_one(
                        doc! { "_id": f.parent.id.as_str() },
                        doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } },
                    )
                    .await
                    .unwrap();
            }
            "audit" => {
                record.admission_audit.policy_revision += 1;
            }
            "receipt" => {
                f.store
                    .reject_sandbox_control(&record, SandboxControlFailure::AuthorityRevoked)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = f.documents().await;
        let result = f.store.apply_sandbox_control(&record, proof).await;
        if change == "revision" {
            assert!(matches!(
                result.unwrap().status,
                SandboxControlStatus::Rejected {
                    reason: SandboxControlFailure::TargetChanged,
                    ..
                }
            ));
            let after = f.documents().await;
            assert_eq!(after.2, before.2);
            assert_eq!(after.3, before.3);
        } else {
            assert_eq!(result, Err(SandboxControlError::Conflict), "{change}");
            assert_eq!(f.documents().await, before, "{change}");
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_controls_index_record_disagreement_and_forged_reservation_fail_closed() {
    let (client, name) = database("sandbox_controls_corrupt").await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("corrupt").await.unwrap();
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    let mut forged = record.clone();
    forged.admission_audit.policy_revision += 1;
    assert_eq!(
        f.store.reserve_sandbox_control(forged, proof, Ok(())).await,
        Err(SandboxControlError::Conflict)
    );
    f.store
        .sandbox_controls
        .update_one(
            doc! { "_id": record.envelope.command_id.as_str() },
            doc! { "$set": { "scope": "wrong" } },
        )
        .await
        .unwrap();
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
    client.database(&name).drop().await.unwrap();
}

include!("sandbox_controls_tests/transaction_faults.rs");

#[path = "sandbox_control_wake_tests.rs"]
mod wake;

struct PrincipalCustody;
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for PrincipalCustody {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        credential: &str,
        _: &ResourceOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(AgentAuthorizationRevision {
            authority_id: credential.into(),
            policy_revision: 1,
            grant_revision: None,
            replay_principal: Some("principal-p".into()),
        })
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn principal_scoped_pause_replay_survives_authority_change_and_reopen() {
    let (client, name) = database("sandbox_principal_replay").await;
    let f = Fixture::new(&client, &name).await;
    let service = SandboxControlService::new(
        f.store.clone(),
        f.access.clone(),
        Arc::new(PrincipalCustody),
    );
    let submission = SandboxControlSubmission {
        sandbox_id: f.parent.id.clone(),
        command: SandboxControl::Pause,
        expected_revision: f.parent.revision,
        idempotency_key: "principal-pause".into(),
    };
    let original = service
        .admit(&context("recipient"), "authority-a", submission.clone())
        .await
        .unwrap();
    drop(service);
    drop(f);
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let stored = reopened
        .sandbox_control(&original.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, original);
    assert_eq!(
        stored.envelope.authorization.replay_principal.as_deref(),
        Some("principal-p")
    );
    let access = Arc::new(ResourceAccessService::new(
        reopened.clone(),
        Arc::new(Membership(MAX_MEMBERSHIP_AGE)),
    ));
    let service = SandboxControlService::new(reopened.clone(), access, Arc::new(PrincipalCustody));
    let replayed = service
        .admit(&context("recipient"), "authority-b", submission)
        .await
        .unwrap();
    assert_eq!(replayed, original);
    assert_eq!(replayed.envelope.authorization.authority_id, "authority-a");
    assert_eq!(
        reopened
            .sandbox_controls
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
