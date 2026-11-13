use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::control::*;
use std::time::{Duration, SystemTime};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_control_wake_waits_for_applied_durable_commit() {
    use crate::store::admission_wake_test_support::CommitGate;
    use futures_util::FutureExt;
    let gate = Arc::new(CommitGate::default());
    let client = gate.client().await;
    let name = format!("cma_control_wake_{}", uuid::Uuid::new_v4().simple());
    let f = Fixture::new(&client, &name).await;
    let wake = f.store.sandbox_admitted.clone();
    let record = f.admit("wake").await.unwrap();
    assert!(
        wake.notified().now_or_never().is_none(),
        "queued ledger alone cannot wake lifecycle execution"
    );
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    let store = f.store.clone();
    gate.arm();
    let task = tokio::spawn(async move { store.apply_sandbox_control(&record, proof).await });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let premature = wake.notified().now_or_never().is_some();
    let before = f.store.get_sandbox(&f.parent.id).await.unwrap().unwrap();
    gate.release();
    let applied = task.await.unwrap().unwrap();
    assert!(!premature);
    assert_eq!(before.status, SandboxStatus::Active);
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied { .. }
    ));
    assert!(wake.notified().now_or_never().is_some());
    assert!(wake.notified().now_or_never().is_none());
    assert_eq!(
        f.store
            .get_sandbox(&f.parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
    client.database(&name).drop().await.unwrap();
    client.shutdown().await;
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

async fn failpoint_client() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_sandbox_controls_{suffix}");
    let app = format!("cma-controls-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    (admin, Client::with_options(options).unwrap(), name, app)
}
async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_controls_expiry_during_event_insert_rolls_back_all_effects() {
    let (admin, client, name, app) = failpoint_client().await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("expire").await.unwrap();
    let proof = f.proof(&record, Duration::from_millis(500)).await;
    let before = f.documents().await;
    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1_i32 }, "data": { "failCommands": ["insert"], "appName": &app, "blockConnection": true, "blockTimeMS": 1000_i32 } }).await.unwrap();
    let result = f.store.apply_sandbox_control(&record, proof).await;
    disable(&admin).await;
    assert_eq!(result, Err(SandboxControlError::Conflict));
    assert_eq!(f.documents().await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_controls_policy_revocation_during_transaction_cannot_commit() {
    let (admin, client, name, app) = failpoint_client().await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("policy").await.unwrap();
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    let replica = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let configured = admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1_i32 }, "data": { "failCommands": ["update"], "appName": &app, "blockConnection": true, "blockTimeMS": 2000_i32 } }).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let store = f.store.clone();
    let work = record.clone();
    let pending = tokio::spawn(async move { store.apply_sandbox_control(&work, proof).await });
    let reached = admin.database("admin").run_command(doc! { "waitForFailPoint": "failCommand", "timesEntered": count + 1, "maxTimeMS": 5000_i32 }).await;
    let snapshot = replica
        .resource_access_snapshot(&record.envelope.target())
        .await
        .unwrap();
    let share = snapshot.shares[0].clone();
    let access =
        ResourceAccessService::new(Arc::new(replica), Arc::new(Membership(MAX_MEMBERSHIP_AGE)));
    let revoked = access
        .change_share(
            &context("owner"),
            Some("fixture"),
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-held".into(),
            },
        )
        .await;
    let result = pending.await.unwrap();
    disable(&admin).await;
    reached.unwrap();
    revoked.unwrap();
    assert_eq!(result, Err(SandboxControlError::Conflict));
    assert_eq!(
        f.store.get_sandbox(&f.parent.id).await.unwrap(),
        Some(f.parent)
    );
    assert_eq!(
        f.store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap(),
        Some(record)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_controls_ambiguous_commit_recovers_receipt_without_repeated_transition() {
    let (admin, client, name, app) = failpoint_client().await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("ambiguous").await.unwrap();
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 5_i32 }, "data": { "failCommands": ["commitTransaction"], "appName": &app, "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous control commit" } } }).await.unwrap();
    let result = f.store.apply_sandbox_control(&record, proof).await;
    disable(&admin).await;
    assert_eq!(result, Err(SandboxControlError::Unavailable));
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let stored = reopened
        .sandbox_control(&record.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        stored.status,
        SandboxControlStatus::Applied { .. }
    ));
    let before = f.documents().await;
    assert_eq!(f.admit("ambiguous").await.unwrap(), stored);
    // The additive admission guard may advance; retained aggregate, command,
    // event and Agent history remain identical across retry.
    let after = f.documents().await;
    assert_eq!(before.1, after.1);
    assert_eq!(before.2, after.2);
    assert_eq!(before.3, after.3);
    let mut old = before.0;
    let mut new = after.0;
    old.remove("resource_share_guard");
    new.remove("resource_share_guard");
    assert_eq!(old, new);
    client.database(&name).drop().await.unwrap();
}
