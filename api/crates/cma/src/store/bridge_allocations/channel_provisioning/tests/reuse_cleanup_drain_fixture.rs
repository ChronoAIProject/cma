use super::*;

impl CleanupFixture {
    pub(super) async fn close_admission(
        &self,
    ) -> Result<ChannelReuseCleanupAdmission, AllocationError> {
        self.f
            .f
            .original
            .allocations
            .close_channel_reuse_cleanup_admission(&self.permit().await, &self.request())
            .await
    }
    pub(super) async fn observe(&self) -> Result<ChannelReuseCleanupObservation, AllocationError> {
        self.f
            .f
            .original
            .allocations
            .observe_channel_reuse_cleanup_quiescence(&self.permit().await, &self.request())
            .await
    }
    pub(super) async fn ordinary_request(
        &self,
        id: &layer2_core::AgentId,
        key: &str,
        command: AgentCommand,
    ) -> CommandReservation {
        let agent = self
            .f
            .f
            .original
            .resources
            .get_agent(id)
            .await
            .unwrap()
            .unwrap();
        let parent = self.f.sandbox().await;
        let policy_revision = self
            .db()
            .collection::<Document>("resource_policies")
            .find_one(doc! {"_id":parent.id.as_str()})
            .await
            .unwrap()
            .as_ref()
            .map(crate::store::resource_access::read_inventory_policy)
            .transpose()
            .unwrap()
            .map_or(0, |policy| policy.revision);
        let now = SystemTime::now();
        CommandReservation {
            authority_deadline: None,
            admission_audit: None,
            response_id: matches!(command, AgentCommand::Message { .. })
                .then(layer2_core::ids::AgentResponseId::generate),
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: layer2_core::ids::AgentCommandId::generate(),
                agent_id: agent.id.clone(),
                owner: self.f.record.owner.clone(),
                actor: caller().actor,
                source: AgentCommandSource {
                    kind: AgentSourceKind::UserInterface,
                    reference: "cleanup-drain-fixture".into(),
                },
                correlation_id: key.into(),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    authority_id: "personal-owner".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                command,
            },
            key_sha256: command_key_digest(key).unwrap(),
            admitted_at: layer2_core::rfc3339_at(now),
            queue_deadline: layer2_core::rfc3339_at(now + Duration::from_secs(120)),
            queue_capacity: 8,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&parent),
            policy: layer2_core::resource_access::ResourcePolicyFence {
                sandbox_id: parent.id,
                sandbox_created_at: parent.created_at,
                policy_revision,
            },
        }
    }
    pub(super) fn operations(&self) -> Arc<AgentOperationService> {
        AgentOperationService::new(self.f.f.original.resources.clone(), self.f.registry.clone())
    }
}
