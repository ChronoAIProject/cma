use super::*;

#[tokio::test]
async fn custody_probe_proves_selected_key_possession_and_active_owner_without_reading_message_text(
) {
    for probe_status in [StatusCode::OK, StatusCode::FORBIDDEN] {
        let mut rows = replies("person-one", json!({"type":"personal"}));
        rows.insert(
            "/api/v1/users/me/authorization".into(),
            (StatusCode::OK, json!({"id":"person-one","is_active":true})),
        );
        rows.insert(
            "/api/v1/channel-relay/messages/route-one".into(),
            (
                probe_status,
                json!({"messages":[],"total":0,"page":1,"per_page":1}),
            ),
        );
        let f = fixture(rows).await;
        let result = f
            .provider
            .verify_secret(
                &personal(),
                &SecretValue::new(format!("nyxid_ag_{}", "a".repeat(64))),
                &f.connector,
            )
            .await;
        assert_eq!(result.is_ok(), probe_status == StatusCode::OK);
        assert!(
            f.state
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|(_, uri, _)| uri
                    == "/api/v1/channel-relay/messages/route-one?page=1&per_page=1")
        );
    }
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.insert(
        "/api/v1/users/me/authorization".into(),
        (StatusCode::OK, json!({"id":"person-one","is_active":false})),
    );
    let f = fixture(rows).await;
    assert!(f
        .provider
        .verify_secret(
            &personal(),
            &SecretValue::new(format!("nyxid_ag_{}", "a".repeat(64))),
            &f.connector
        )
        .await
        .is_err());
    assert_eq!(f.state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn relay_reply_and_human_tokens_cannot_be_retained_as_bridge_keys() {
    let f = fixture(replies("person-one", json!({"type":"personal"}))).await;
    for token in [
        "header.relay.signature",
        "header.reply.signature",
        "opaque-human-oauth-token",
        "nyxid_ag_short",
    ] {
        assert!(f
            .provider
            .verify_secret(&personal(), &SecretValue::new(token.into()), &f.connector)
            .await
            .is_err());
    }
    assert!(f.state.requests.lock().unwrap().is_empty());
}

#[derive(Default)]
struct CredentialMemory(tokio::sync::Mutex<Option<crate::bridge::custody::StoredBridgeCredential>>);
#[async_trait::async_trait]
impl crate::bridge::custody::BridgeCredentialStore for CredentialMemory {
    async fn get(
        &self,
        reference: &BridgeSecretRef,
    ) -> Result<Option<crate::bridge::custody::StoredBridgeCredential>, BridgeError> {
        Ok(self
            .0
            .lock()
            .await
            .as_ref()
            .filter(|m| m.metadata.connector.custody == *reference)
            .map(|m| crate::bridge::custody::StoredBridgeCredential {
                metadata: m.metadata.clone(),
                value: m.value.clone(),
            }))
    }
    async fn admit(
        &self,
        lease: &layer2_core::bridge::BridgeManagementLease,
        material: crate::bridge::custody::StoredBridgeCredential,
    ) -> Result<crate::bridge::custody::BridgeCustodyMetadata, BridgeError> {
        lease.require_current()?;
        let result = material.metadata.clone();
        *self.0.lock().await = Some(material);
        Ok(result)
    }
    async fn revoke(
        &self,
        lease: &layer2_core::bridge::BridgeManagementLease,
        _: &BridgeSecretRef,
        expected: u64,
        digest: &str,
    ) -> Result<crate::bridge::custody::BridgeCustodyMetadata, BridgeError> {
        lease.require_current()?;
        let mut state = self.0.lock().await;
        let material = state.as_mut().ok_or(BridgeError::NotFound)?;
        if material.metadata.revision != expected {
            return Err(BridgeError::Conflict);
        }
        material.metadata.revision += 1;
        material.metadata.revoked_at = Some(layer2_core::now_rfc3339());
        material.metadata.revoked_by = Some(lease.actor().clone());
        material.metadata.revocation_key_digest = Some(digest.into());
        Ok(material.metadata.clone())
    }
}
struct NoOrganization;
#[async_trait::async_trait]
impl layer2_core::resource_access::OrganizationAuthority for NoOrganization {
    async fn organization_context(
        &self,
        _: &layer2_core::Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<
        layer2_core::resource_access::CallerContext,
        layer2_core::resource_access::MembershipError,
    > {
        unreachable!()
    }
    async fn organization_choices(
        &self,
        _: &layer2_core::Caller,
        _: &str,
    ) -> Result<
        Vec<layer2_core::resource_access::OrganizationChoice>,
        layer2_core::resource_access::MembershipError,
    > {
        unreachable!()
    }
}
#[async_trait::async_trait]
impl layer2_core::bridge::BridgeConnectionValidator for NoOrganization {
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &layer2_core::bridge::BridgeConfig,
    ) -> Result<layer2_core::bridge::BridgeValidation, BridgeError> {
        unreachable!()
    }
}

#[tokio::test]
async fn managed_import_resolves_exact_retained_key_and_local_revocation_stops_original_lease() {
    use crate::bridge::custody::BridgeCredentialResolver;
    use layer2_core::bridge::*;
    use layer2_core::profiles::*;
    use layer2_core::resource_access::{CallerContext, CredentialContext, ResourceActor};
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.insert(
        "/api/v1/users/me/authorization".into(),
        (StatusCode::OK, json!({"id":"person-one","is_active":true})),
    );
    rows.insert(
        "/api/v1/channel-relay/messages/route-one".into(),
        (
            StatusCode::OK,
            json!({"messages":[],"total":0,"page":1,"per_page":1}),
        ),
    );
    let f = fixture(rows).await;
    let context = CallerContext {
        caller: layer2_core::Caller::new(
            SubjectId::new("person-one"),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: f.connector.issuer.clone(),
            subject: SubjectId::new("person-one"),
            credential_id: "browser-login".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new("person-one"),
        },
        memberships: vec![],
    };
    let config = BridgeConfig {
        display_name: "Disposable".into(),
        connector: f.connector.clone(),
        partition: BridgePartitionPolicy::Conversation,
        placement: BridgePlacementPolicy::NewSandbox {
            sandbox_profile: SandboxProfileRef::new(
                layer2_core::SandboxProfileId("sandbox-profile".into()),
                1,
            ),
            workspace_profile: WorkspaceProfileRef::new(
                layer2_core::WorkspaceProfileId("workspace-profile".into()),
                1,
            ),
            agent_profile: AgentProfileRef::new(
                layer2_core::AgentProfileId("agent-profile".into()),
                1,
            ),
        },
        admission: BridgeAdmissionPolicy::All,
        reply: BridgeReplyPolicy::FinalResponse,
        limits: BridgeLimits {
            max_partitions: 2,
            max_sandboxes: 2,
            max_queued_messages: 2,
            max_messages_per_minute: 20,
            max_payload_bytes: 4096,
            inbox_retention_seconds: 3600,
            reply_secret_retention_seconds: 600,
        },
    };
    let definitions = Arc::new(MemoryBridgeStore::default());
    let service = BridgeService::new(
        definitions.clone(),
        Arc::new(NoOrganization),
        Arc::new(NoOrganization),
    );
    let id = BridgeService::creation_id(&context, "create").unwrap();
    service
        .mutate(
            &context,
            None,
            BridgeMutation {
                id: id.clone(),
                expected_revision: 0,
                idempotency_key: "create".into(),
                change: BridgeChange::Create {
                    owner: personal(),
                    config: Box::new(config.clone()),
                },
            },
        )
        .await
        .unwrap();
    let manager = crate::bridge::ManagedBridgeCredentials {
        store: Arc::new(CredentialMemory::default()),
        provider: Arc::new(
            NyxBridgeProvider::with_endpoint(reqwest::Url::parse(&f.connector.base_url).unwrap())
                .unwrap(),
        ),
        bridges: definitions,
    };
    let imported = manager
        .import(
            &service,
            &context,
            None,
            &id,
            1,
            SecretValue::new(format!("nyxid_ag_{}", "a".repeat(64))),
        )
        .await
        .unwrap();
    assert_eq!(imported.owner, personal());
    assert_eq!(imported.created_by, context.actor);
    let lease = manager.resolve(&personal(), &config).await.unwrap();
    assert_eq!(lease.credential_subject, SubjectId::new("person-one"));
    assert_eq!(lease.key_id, "key-one");
    let before_revoke = f.state.requests.lock().unwrap().len();
    let revoked = manager
        .revoke(
            &service,
            &context,
            None,
            &crate::bridge::managed_credentials::BridgeSecretRevocation {
                bridge_id: id.clone(),
                expected_revision: 1,
                reference: config.connector.custody.clone(),
                expected_custody_revision: 1,
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(revoked.revision, 2);
    assert!(manager.revalidate(&lease).await.is_err());
    assert!(manager.resolve(&personal(), &config).await.is_err());
    assert_eq!(f.state.requests.lock().unwrap().len(), before_revoke);
    assert!(f
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|(method, _, _)| method == "GET"));
}
