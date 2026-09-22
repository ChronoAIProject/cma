use super::*;
use layer2_core::bridge::{BridgeConnectionValidator, BridgeValidation, RetainedChannelConnection};
use std::sync::{atomic::Ordering, Mutex};

async fn fixture() -> (Fixture, Arc<MongoBridgeInvokeStore>, BridgeInvokeGrantView) {
    let mut f = Fixture::new().await;
    f.record = f.ready().await;
    f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
    let (store, service) = invokes(&f).await;
    let grant = service
        .issue_channel(
            f.lease.clone(),
            &f.record.id,
            &f.record.pin,
            f.authority.clone(),
        )
        .await
        .unwrap();
    (f, store, grant)
}

fn service(
    f: &Fixture,
    store: Arc<MongoBridgeInvokeStore>,
    connections: Arc<dyn BridgeConnectionValidator>,
) -> BridgeInvokeService {
    BridgeInvokeService::new(
        BridgeService::new(f.bridges.clone(), Arc::new(Authority), connections),
        Arc::new(ResourceAccessService::new(
            f.resources.clone(),
            Arc::new(Authority),
        )),
        f.resources.clone(),
        store,
    )
}

fn request(grant: &BridgeInvokeGrantView, effect: BridgeInvokeEffect) -> BridgeInvokeRequest {
    BridgeInvokeRequest {
        grant_id: grant.id.clone(),
        pin: grant.pin.clone(),
        target: grant.target.ids.clone(),
        effect,
    }
}

struct Replay(BridgeValidation);
#[async_trait::async_trait]
impl BridgeConnectionValidator for Replay {
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_channel_validation_skips_provisioning_but_cannot_escape_to_setup_or_allocation() {
    let (f, store, grant) = fixture().await;
    let (connections, checks) = crate::bridge::retained_validation_test_support::validator(
        f.resources.clone(),
        f.record.pin.bridge_id.clone(),
    );
    assert!(
        connections
            .validate_connection(&f.record.owner, &f.record.config)
            .await
            .is_err(),
        "new Sandbox still needs provisioning dependencies"
    );
    let invokes = service(&f, store, connections);
    let effect = BridgeInvokeEffect::AdmitMessage {
        inbox_id: "fresh-retained-input".into(),
    };
    let proof = invokes.authorize(request(&grant, effect)).await.unwrap();
    proof.require_current().unwrap();
    assert_eq!(checks.full.load(Ordering::SeqCst), 1);
    assert_eq!(checks.retained.load(Ordering::SeqCst), 1);
    assert_eq!(checks.resolutions.load(Ordering::SeqCst), 2);
    assert_eq!(
        checks.revalidations.load(Ordering::SeqCst),
        1,
        "current custody remains checked"
    );
    let retained = checks.proof.lock().unwrap().clone().unwrap();
    let generic = BridgeService::new(
        f.bridges.clone(),
        Arc::new(Authority),
        Arc::new(Replay(retained.clone())),
    );
    assert!(
        generic
            .validate(&caller(), None, &f.record.pin.bridge_id)
            .await
            .is_err(),
        "retained proof cannot validate setup"
    );
    let management = f
        .svc
        .management_lease(
            &caller(),
            None,
            &f.record.pin.bridge_id,
            f.authority.bridge().revision,
        )
        .await
        .unwrap();
    let profiles = f.record.placement.profiles().unwrap();
    let plan = AllocationProfilePlan::renew_for_record(
        &management,
        &f.record,
        AllocationCapabilityObservation {
            owner: f.record.owner.clone(),
            profiles_digest: profiles.trust_digest.clone(),
            capacity_bounds: layer2_core::profiles::ProfileCapacityBounds {
                max_cpu_millis: 64000,
                max_memory_mib: 262144,
                min_cpu_millis: 100,
                min_memory_mib: 128,
            },
            owner_live_sandbox_limit: 3,
            baseline_services: vec![],
            one_sandbox_workspace_worktree_agent_available: true,
            runtime_model_services_skills_available: true,
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
        },
    )
    .unwrap();
    assert!(
        AllocationProvisioningProof::new(
            management.record().clone(),
            &f.record.identity,
            plan,
            retained.clone()
        )
        .is_err(),
        "retained proof cannot create or renew a Sandbox allocation"
    );
    let replay = service(
        &f,
        Arc::new(
            MongoBridgeInvokeStore::open(f.bridges.clone(), f.resources.clone(), &f.database)
                .await
                .unwrap(),
        ),
        Arc::new(Replay(retained)),
    );
    assert!(
        replay
            .authorize(request(
                &grant,
                BridgeInvokeEffect::ExecuteCommand {
                    command_id: layer2_core::ids::AgentCommandId::generate()
                }
            ))
            .await
            .is_err(),
        "proof for one admitted input cannot authorize a different command"
    );
    let before = checks.resolutions.load(Ordering::SeqCst);
    assert!(invokes
        .authorize(request(
            &grant,
            BridgeInvokeEffect::BindExistingTarget {
                allocation_id: f.record.id.clone()
            }
        ))
        .await
        .is_err());
    assert_eq!(checks.resolutions.load(Ordering::SeqCst), before);
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_channel_validation_requires_current_custody_grant_target_and_deadline() {
    let (f, store, grant) = fixture().await;
    let (connections, checks) = crate::bridge::retained_validation_test_support::validator(
        f.resources.clone(),
        f.record.pin.bridge_id.clone(),
    );
    let invokes = service(&f, store, connections);
    let next = || {
        request(
            &grant,
            BridgeInvokeEffect::ExecuteCommand {
                command_id: layer2_core::ids::AgentCommandId::generate(),
            },
        )
    };
    checks.refuse_custody.store(true, Ordering::SeqCst);
    assert!(invokes.authorize(next()).await.is_err());
    checks.refuse_custody.store(false, Ordering::SeqCst);
    let proof = invokes.authorize(next()).await.unwrap();
    let mut resources = proof.resources().clone();
    resources.lineage.agent = None;
    assert!(proof
        .require_snapshot(proof.bridge(), proof.grant(), &resources)
        .is_err());
    let mut wrong = next();
    wrong.target.agent_id = layer2_core::AgentId::generate();
    let calls = checks.resolutions.load(Ordering::SeqCst);
    assert!(invokes.authorize(wrong).await.is_err());
    assert_eq!(checks.resolutions.load(Ordering::SeqCst), calls);
    invokes
        .revoke(
            &caller(),
            None,
            RevokeBridgeInvokeGrant {
                grant_id: grant.id.clone(),
                expected_bridge_revision: proof.bridge().revision,
                expected_revision: grant.revision,
                idempotency_key: "retained-current-revoke".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        invokes.authorize(next()).await,
        Err(BridgeInvokeError::Revoked)
    ));
    assert_eq!(checks.resolutions.load(Ordering::SeqCst), calls);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(proof.require_current().is_err());
    tokio::time::resume();
    f.client.database(&f.database).drop().await.unwrap();
}

struct ChangingTarget {
    inner: Arc<dyn BridgeConnectionValidator>,
    resources: Arc<crate::store::MongoSandboxStore>,
    target: layer2_core::AgentId,
    mutation: Mutex<Option<bool>>,
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for ChangingTarget {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.inner.validate_connection(owner, config).await
    }
    async fn validate_retained_channel(
        &self,
        request: &RetainedChannelConnection,
    ) -> Result<BridgeValidation, BridgeError> {
        let proof = self.inner.validate_retained_channel(request).await?;
        let mutation = self.mutation.lock().unwrap().take();
        if let Some(remove) = mutation {
            if remove {
                self.resources
                    .agents
                    .delete_one(doc! {"_id":self.target.as_str()})
                    .await
                    .unwrap();
            } else {
                self.resources
                    .agents
                    .update_one(
                        doc! {"_id":self.target.as_str()},
                        doc! {"$set":{"record.created_at":"2001-01-01T00:00:00Z"}},
                    )
                    .await
                    .unwrap();
            }
        }
        Ok(proof)
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_channel_validation_refuses_target_change_during_fresh_connection_io() {
    for remove in [false, true] {
        let (f, store, grant) = fixture().await;
        let (connections, checks) = crate::bridge::retained_validation_test_support::validator(
            f.resources.clone(),
            f.record.pin.bridge_id.clone(),
        );
        let invokes = service(
            &f,
            store,
            Arc::new(ChangingTarget {
                inner: connections,
                resources: f.resources.clone(),
                target: grant.target.ids.agent_id.clone(),
                mutation: Mutex::new(Some(remove)),
            }),
        );
        assert!(invokes
            .authorize(request(
                &grant,
                BridgeInvokeEffect::AdmitMessage {
                    inbox_id: "target-race-input".into()
                }
            ))
            .await
            .is_err());
        assert_eq!(checks.retained.load(Ordering::SeqCst), 1);
        f.client.database(&f.database).drop().await.unwrap();
    }
}
