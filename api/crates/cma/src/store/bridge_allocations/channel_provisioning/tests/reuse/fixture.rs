use super::*;
use crate::store::sandbox_mongo::record_bson;
use std::time::{Duration, SystemTime};

pub(crate) struct ReuseFixture {
    pub original: Fixture,
    pub parent: Sandbox,
    pub sibling: Agent,
    pub workspace: Workspace,
    pub profiles: AllocationProfiles,
}
impl ReuseFixture {
    pub async fn new(shared_workspace: bool) -> Self {
        Self::with_original(shared_workspace, Fixture::new().await).await
    }
    pub async fn with_original(shared_workspace: bool, mut original: Fixture) -> Self {
        original.ready().await;
        let target = original.record.placement.target().clone();
        let mut parent = original
            .resources
            .get_sandbox(&target.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let sibling = original
            .resources
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap();
        let workspace = original
            .resources
            .get_workspace(&target.workspace_id)
            .await
            .unwrap()
            .unwrap();
        let mut profiles = original.record.placement.profiles().unwrap().clone();
        // Construct the disposable shared owner fixture before exercising reuse.
        // Subsequent assertions require exact equality to this parent and sibling.
        parent.profile.content.max_workspaces = 4;
        parent.profile.content.max_agents_per_workspace = 4;
        parent.profile.digest =
            layer2_core::profiles::content_digest(&parent.profile.content).unwrap();
        parent.limits = SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        };
        parent.origin = SandboxOrigin::Manual;
        parent.work.step = SandboxWorkStep::WaitForCredentialRotation;
        parent.validate().unwrap();
        profiles.sandbox = parent.profile.clone();
        profiles.trust_digest =
            AllocationProfiles::digest(&profiles.sandbox, &profiles.workspace, &profiles.agent)
                .unwrap();
        let database = original.client.database(&original.database);
        let rows = database.collection::<Document>("sandboxes");
        rows.update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap(),
            "$unset":{"channel_creation":"","channel_provisioning":"",
                "channel_renewal":"","channel_lifecycle_credential":"",
                "channel_mint_reconciliation":"","channel_retirement":"","channel_cleanup":""}},
        )
        .await
        .unwrap();
        let row = database
            .collection::<Document>("agent_creation_provenance")
            .find_one(doc! {"_id":sibling.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut origin = crate::store::agent_provenance::read(&row).unwrap();
        origin.creation_origin = AgentCreationOrigin::Manual;
        database
            .collection::<Document>("agent_creation_provenance")
            .replace_one(
                doc! {"_id":sibling.id.as_str()},
                crate::store::agent_provenance::document(&origin).unwrap(),
            )
            .await
            .unwrap();
        database
            .collection::<Document>("bridge_allocations")
            .delete_many(doc! {})
            .await
            .unwrap();
        database
            .collection::<Document>("bridge_allocation_heads")
            .delete_many(doc! {})
            .await
            .unwrap();
        let mut bridge = original
            .bridges
            .bridge(&original.record.identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let config = &mut bridge.generations.last_mut().unwrap().config;
        let shared_trust = BridgeSharedTrust::AllParticipantsShareFilesystemProcessesAndCredentials;
        let capacity = BridgeSharedCapacity {
            keep_other_workspaces: 1,
            keep_other_agents_per_workspace: 1,
        };
        let agent_profile = layer2_core::profiles::AgentProfileRef::new(
            profiles.agent.id.clone(),
            profiles.agent.revision,
        );
        config.placement = if shared_workspace {
            BridgePlacementPolicy::WorktreeAgentPerPartition {
                sandbox_id: parent.id.clone(),
                workspace_id: workspace.id.clone(),
                agent_profile,
                shared_trust,
                capacity,
            }
        } else {
            BridgePlacementPolicy::WorkspacePerPartition {
                sandbox_id: parent.id.clone(),
                workspace_profile: layer2_core::profiles::WorkspaceProfileRef::new(
                    profiles.workspace.id.clone(),
                    profiles.workspace.revision,
                ),
                agent_profile,
                shared_trust,
                capacity,
            }
        };
        config.limits.max_partitions = 10;
        bridge.validate().unwrap();
        original
            .bridges
            .definitions
            .update_one(
                doc! {"_id":bridge.id.as_str()},
                doc! {"$set":{"record":record_bson(&bridge).unwrap()}},
            )
            .await
            .unwrap();
        let invokes = Arc::new(
            MongoBridgeInvokeStore::open(
                original.bridges.clone(),
                original.resources.clone(),
                &original.database,
            )
            .await
            .unwrap(),
        );
        original.allocations = Arc::new(
            MongoBridgeAllocationStore::open(
                original.bridges.clone(),
                Some(invokes),
                &original.database,
            )
            .await
            .unwrap(),
        );
        Self {
            original,
            parent,
            sibling,
            workspace,
            profiles,
        }
    }
    pub async fn proposal(&self, sender: &str) -> (AllocationAdmission, AllocationAuthority) {
        let f = &self.original;
        let bridge = f
            .bridges
            .bridge(&f.record.identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let config = &bridge.current().unwrap().config;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let inbox = StoredInbox {
            id: layer2_core::profiles::content_digest(&sender).unwrap(),
            owner: bridge.owner.clone(),
            pin: bridge.admission_pin().unwrap(),
            bot_id: config.connector.bot_id.clone(),
            key_id: config.connector.key_id.clone(),
            message_id: format!("message-{sender}"),
            correlation_id: format!("correlation-{sender}"),
            envelope: InboxEnvelope {
                platform: "telegram".into(),
                route_id: config.connector.route_ids[0].clone(),
                chat_id: "-100123".into(),
                conversation_type: "group".into(),
                sender_id: sender.into(),
                sender_display_name: None,
                content_type: "text".into(),
                text: Some("fixture".into()),
                attachments: vec![],
                reply_to_message_id: None,
                reply_to_platform_message_id: None,
                protected_thread_context_present: false,
                timestamp: layer2_core::now_rfc3339(),
            },
            attestation: CallbackAttestation {
                issuer: config.connector.issuer.clone(),
                signing_key_id: "signing".into(),
                token_id: format!("token-{sender}"),
                issued_at: now - 1,
                expires_at: now + 300,
                body_sha256: layer2_core::profiles::content_digest(&(sender, "fixture")).unwrap(),
            },
            relay: RelayCorroboration::AbsentInboxOnly,
            admission: InboxAdmission::Accepted,
            capabilities: None,
            execution_hold: None,
            accepted_at: now,
            expires_at: now + 3600,
            reply_expires_at: None,
        };
        let mut observation = PlatformCapabilityObservation::unknown(&inbox).unwrap();
        observation.sender_kind = ChannelSenderKind::Human;
        observation.event_kind = ChannelEventKind::UserMessage;
        observation.loop_evidence = LoopEvidence::NotLoop;
        let source =
            NormalizedPartitionSource::from_observation(&inbox, &bridge, observation).unwrap();
        let admission = AllocationAdmission::new(bridge, &source).unwrap();
        let authority = self.proof(admission.identity(), None).await;
        (admission, authority)
    }
    pub async fn authority(&self, record: &AllocationRecord) -> AllocationAuthority {
        self.proof(&record.identity, Some(record)).await
    }
    pub(super) async fn proof(
        &self,
        identity: &AllocationIdentity,
        retained: Option<&AllocationRecord>,
    ) -> AllocationAuthority {
        let f = &self.original;
        let bridge = f
            .bridges
            .bridge(&identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let pin = retained.map_or_else(|| bridge.admission_pin().unwrap(), |r| r.pin.clone());
        let config = &bridge.pinned(&pin).unwrap().config;
        let management = f
            .svc
            .management_lease(&caller(), None, &bridge.id, bridge.revision)
            .await
            .unwrap();
        let access = ResourceAccessService::new(f.resources.clone(), Arc::new(Authority));
        let authorized = access
            .authorize(
                &caller(),
                None,
                &config.placement.reused_parent().unwrap(),
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await
            .unwrap();
        let connection = BridgeValidation::from_observation(
            &bridge.owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: bridge.owner.clone(),
                key_owner: bridge.owner.clone(),
                routes: config
                    .connector
                    .route_ids
                    .iter()
                    .map(|route_id| BridgeRouteObservation {
                        route_id: route_id.clone(),
                        owner: bridge.owner.clone(),
                        bot_id: config.connector.bot_id.clone(),
                        key_id: config.connector.key_id.clone(),
                        enabled: true,
                    })
                    .collect(),
                custody_owner: bridge.owner.clone(),
                custody_revision: f.lease.custody_revision(),
                key_state_version: f.authority.custody_pin().key_state_version(),
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: true,
                exact_profiles_available: true,
                existing_target: Some(authorized.snapshot.clone()),
                deadline: tokio::time::Instant::now() + Duration::from_secs(15),
            },
        )
        .unwrap();
        let observation = AllocationReuseObservation {
            ordinary_parent_lifecycle_available: true,
            profiles_digest: self.profiles.trust_digest.clone(),
            runtime_model_services_skills_available: true,
            workspace_writer_coordination_available: true,
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
        };
        let proof = if let Some(record) = retained {
            AllocationReuseProof::renew_for_record(
                &management,
                record,
                authorized,
                connection,
                observation,
            )
        } else {
            AllocationReuseProof::prepare(
                &management,
                identity,
                pin,
                self.profiles.clone(),
                authorized,
                connection,
                observation,
            )
        }
        .unwrap();
        AllocationAuthority::Reuse(Box::new(proof))
    }
    pub fn driver(&self) -> ChannelReuseDriver {
        let f = &self.original;
        ChannelReuseDriver {
            allocations: f.allocations.clone(),
            credentials: Arc::new(super::super::fixture::VerifiedHuman),
            adapter: f.adapter.clone(),
            settings: f.settings.clone(),
            holder: "shared-child-worker".into(),
            startup: ChannelReuseStartupDriver {
                allocations: f.allocations.clone(),
                credentials: Arc::new(super::super::fixture::VerifiedHuman),
                adapter: f.adapter.clone(),
                resources: f.resources.clone(),
                skills: OrnnAgentSkillResolver::anonymous("https://nyx.example").unwrap(),
            },
        }
    }
}
