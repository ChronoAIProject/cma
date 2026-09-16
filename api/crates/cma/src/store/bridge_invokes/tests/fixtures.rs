use super::*;
use crate::auth::persistence::SecretValue;
use crate::bridge::custody::{
    BridgeCredentialStore, BridgeCustodyMetadata, StoredBridgeCredential,
};
use crate::store::AuthEncryptionKey;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::resource_access::{
    CredentialContext, MembershipError, OrganizationAuthority, OrganizationChoice,
};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use std::time::Duration;

pub(in crate::store::bridge_invokes) struct Authority {
    pub role: std::sync::Mutex<Option<OrganizationRole>>,
    pub snapshot: std::sync::Mutex<Option<ResourceAccessSnapshot>>,
}
impl Default for Authority {
    fn default() -> Self {
        Self {
            role: std::sync::Mutex::new(None),
            snapshot: std::sync::Mutex::new(None),
        }
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        principal: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let role = (*self.role.lock().unwrap()).ok_or(MembershipError::NotMember)?;
        let mut context = caller();
        context.caller = principal.clone();
        context.credential.subject = principal.subject.clone();
        context.actor = ResourceActor::Human {
            subject: principal.subject.clone(),
        };
        context.memberships.push(
            OrganizationMembership::new(
                context.credential.clone(),
                organization.clone(),
                role,
                std::time::SystemTime::now(),
                std::time::SystemTime::now() + Duration::from_secs(30),
            )
            .unwrap(),
        );
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Authority {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: vec![BridgeRouteObservation {
                    route_id: "route".into(),
                    owner: owner.clone(),
                    bot_id: config.connector.bot_id.clone(),
                    key_id: config.connector.key_id.clone(),
                    enabled: true,
                }],
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 3,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: false,
                exact_profiles_available: false,
                existing_target: self.snapshot.lock().unwrap().clone(),
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
    }
}
pub(super) fn caller() -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://nyx.example".into(),
            subject: SubjectId::new("owner"),
            credential_id: "fixture-login".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        memberships: vec![],
    }
}
pub(in crate::store::bridge_invokes) struct Fixture {
    pub client: mongodb::Client,
    pub database: String,
    pub bridges: Arc<MongoBridgeStore>,
    pub resources: Arc<MongoSandboxStore>,
    pub store: Arc<MongoBridgeInvokeStore>,
    pub definitions: BridgeService,
    pub service: BridgeInvokeService,
    pub bridge: BridgeRecord,
    pub target: BridgeAgentTarget,
    pub context: CallerContext,
    pub authority: Arc<Authority>,
}
impl Fixture {
    pub async fn new() -> Self {
        Self::build(false, BridgePlatform::Telegram).await
    }
    pub async fn organization() -> Self {
        Self::build(true, BridgePlatform::Telegram).await
    }
    pub async fn discord_text() -> Self {
        Self::build(false, BridgePlatform::Discord).await
    }
    async fn build(organization: bool, platform: BridgePlatform) -> Self {
        let mut context = caller();
        if organization {
            context.caller.subject = SubjectId::new("admin");
            context.credential.subject = SubjectId::new("admin");
            context.actor = ResourceActor::Human {
                subject: SubjectId::new("admin"),
            };
        }
        let owner = if organization {
            ResourceOwner::Organization {
                subject: SubjectId::new("owner"),
            }
        } else {
            ResourceOwner::Person {
                subject: SubjectId::new("owner"),
            }
        };
        let credential = organization.then_some("fixture-credential");
        let authority = Arc::new(Authority {
            role: std::sync::Mutex::new(organization.then_some(OrganizationRole::Admin)),
            snapshot: std::sync::Mutex::new(None),
        });
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("disposable Mongo contract replica required");
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let database = format!("cma_bridge_invoke_{}", uuid::Uuid::new_v4().simple());
        let (resources, resource) =
            crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
                &client, &database,
            )
            .await;
        let resources = Arc::new(resources);
        let snapshot = resources.resource_access_snapshot(&resource).await.unwrap();
        let target = BridgeInvokeTarget::capture(&snapshot).unwrap().ids;
        if organization {
            let policy = ResourcePolicyRecord {
                sandbox_id: snapshot.lineage.sandbox.id.clone(),
                sandbox_created_at: snapshot.lineage.sandbox.created_at.clone(),
                owner: owner.clone(),
                revision: 1,
                organization_evidence: Some(OrganizationOwnershipEvidence {
                    issuer: context.credential.issuer.clone(),
                    organization: SubjectId::new("owner"),
                    admitted_by: context.actor.clone(),
                    verified_at: layer2_core::now_rfc3339(),
                }),
            };
            resources.resource_policies.insert_one(doc! {"_id":policy.sandbox_id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
                "owner_kind":"organization","owner_subject":"owner","revision":1_i64,"record":record_bson(&policy).unwrap()}).await.unwrap();
        }
        *authority.snapshot.lock().unwrap() =
            Some(resources.resource_access_snapshot(&resource).await.unwrap());
        let key = AuthEncryptionKey::from_base64(&STANDARD.encode([7_u8; 32])).unwrap();
        let bridges = Arc::new(
            MongoBridgeStore::open(&client, &database, "fixture-deployment", &key)
                .await
                .unwrap(),
        );
        let definitions = BridgeService::new(bridges.clone(), authority.clone(), authority.clone());
        let id = BridgeService::creation_id(&context, "create").unwrap();
        let config = BridgeConfig {
            display_name: "Retained context fixture".into(),
            connector: BridgeConnector {
                issuer: "https://nyx.example".into(),
                base_url: "https://nyx.example".into(),
                bot_id: "bot".into(),
                platform,
                route_ids: vec!["route".into()],
                key_id: "key".into(),
                custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into())
                    .unwrap(),
                callback_url: "https://cma.example/bridge".into(),
            },
            partition: BridgePartitionPolicy::FixedSharedAgent,
            placement: BridgePlacementPolicy::ExistingAgent {
                target: target.clone(),
            },
            admission: BridgeAdmissionPolicy::All,
            reply: BridgeReplyPolicy::FinalResponse,
            limits: BridgeLimits {
                max_partitions: 2,
                max_sandboxes: 2,
                max_queued_messages: 5,
                max_messages_per_minute: 30,
                max_payload_bytes: 4096,
                inbox_retention_seconds: 3600,
                reply_secret_retention_seconds: 600,
            },
        };
        definitions
            .mutate(
                &context,
                credential,
                BridgeMutation {
                    id: id.clone(),
                    expected_revision: 0,
                    idempotency_key: "create".into(),
                    change: BridgeChange::Create {
                        owner,
                        config: Box::new(config.clone()),
                    },
                },
            )
            .await
            .unwrap();
        let management = definitions
            .management_lease(&context, credential, &id, 1)
            .await
            .unwrap();
        bridges
            .credentials
            .admit(
                &management,
                StoredBridgeCredential {
                    metadata: BridgeCustodyMetadata {
                        bridge_id: id.clone(),
                        owner: management.record().owner.clone(),
                        connector: config.connector,
                        revision: 1,
                        key_state_version: 3,
                        scopes: "proxy read write".into(),
                        allowed_service_ids: vec![],
                        allow_all_services: false,
                        allowed_node_ids: vec![],
                        allow_all_nodes: false,
                        created_at: layer2_core::now_rfc3339(),
                        created_by: context.actor.clone(),
                        revoked_at: None,
                        revoked_by: None,
                        revocation_key_digest: None,
                    },
                    value: SecretValue::new("fixture-invoke-secret".into()),
                    control_plane: None,
                },
            )
            .await
            .unwrap();
        definitions
            .mutate(
                &context,
                credential,
                BridgeMutation {
                    id: id.clone(),
                    expected_revision: 1,
                    idempotency_key: "allow-original-work".into(),
                    change: BridgeChange::Disable {
                        accepted_work: AcceptedWorkPolicy::DrainOriginal,
                    },
                },
            )
            .await
            .unwrap();
        let bridge = bridges.bridge(&id).await.unwrap().unwrap();
        let store = Arc::new(
            MongoBridgeInvokeStore::open(bridges.clone(), resources.clone(), &database)
                .await
                .unwrap(),
        );
        let access = Arc::new(ResourceAccessService::new(
            resources.clone(),
            authority.clone(),
        ));
        let service = BridgeInvokeService::new(
            definitions.clone(),
            access,
            resources.clone(),
            store.clone(),
        );
        Self {
            client,
            database,
            bridges,
            resources,
            store,
            definitions,
            service,
            bridge,
            target,
            context,
            authority,
        }
    }
    pub async fn enable(&mut self, key: &str) -> BridgeReceipt {
        let management = self
            .definitions
            .management_lease(
                &self.context,
                Some("fixture-credential"),
                &self.bridge.id,
                self.bridge.revision,
            )
            .await
            .unwrap();
        let proof = self.service.prepare_enable(&management).await.unwrap();
        let receipt = self
            .definitions
            .mutate_with_enable_proof(
                &self.context,
                Some("fixture-credential"),
                BridgeMutation {
                    id: self.bridge.id.clone(),
                    expected_revision: self.bridge.revision,
                    idempotency_key: key.into(),
                    change: BridgeChange::Enable,
                },
                proof,
            )
            .await
            .unwrap();
        self.bridge = self.bridges.bridge(&self.bridge.id).await.unwrap().unwrap();
        receipt
    }
    pub fn issue(&self, key: &str) -> IssueBridgeInvokeGrant {
        IssueBridgeInvokeGrant {
            bridge_id: self.bridge.id.clone(),
            expected_bridge_revision: self.bridge.revision,
            target: self.target.clone(),
            shared_context: BridgeSharedContextConsent::UseRetainedAgentContext,
            idempotency_key: key.into(),
        }
    }
    pub fn request(
        &self,
        grant: &BridgeInvokeGrantView,
        effect: BridgeInvokeEffect,
    ) -> BridgeInvokeRequest {
        BridgeInvokeRequest {
            grant_id: grant.id.clone(),
            pin: grant.pin.clone(),
            target: grant.target.ids.clone(),
            effect,
        }
    }
    pub fn revoke(&self, grant: &BridgeInvokeGrantView, key: &str) -> RevokeBridgeInvokeGrant {
        RevokeBridgeInvokeGrant {
            grant_id: grant.id.clone(),
            expected_bridge_revision: self.bridge.revision,
            expected_revision: grant.revision,
            idempotency_key: key.into(),
        }
    }
    pub async fn effect(&self, proof: &BridgeInvokeProof) -> Result<(), BridgeInvokeError> {
        let mut session = self.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        if let Err(error) = self.store.fence(proof, &mut session).await {
            session.abort_transaction().await.unwrap();
            return Err(error);
        }
        session.commit_transaction().await.unwrap();
        Ok(())
    }
    pub async fn close(self) {
        self.client.database(&self.database).drop().await.unwrap();
    }
}
