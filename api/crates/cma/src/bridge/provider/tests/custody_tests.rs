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
                &SubjectId::new("person-one"),
                "fixture-human-oauth",
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
            &f.connector,
            &SubjectId::new("person-one"),
            "fixture-human-oauth",
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
            .verify_secret(
                &personal(),
                &SecretValue::new(token.into()),
                &f.connector,
                &SubjectId::new("person-one"),
                "fixture-human-oauth"
            )
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
                control_plane: m.control_plane.clone(),
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
    rows.insert(
        "/userinfo".into(),
        (
            StatusCode::OK,
            json!({"sub":"person-one","permissions":["cma:sandbox:use"]}),
        ),
    );
    let f = fixture(rows).await;
    let auth_store = Arc::new(crate::auth::persistence::MemoryAuthStore::default());
    let auth = crate::auth::AuthState::with_store(
        crate::auth::AuthMode::Provider {
            base_url: f.connector.base_url.clone(),
            client_id: "fixture".into(),
            client_secret: "fixture-client-secret".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        auth_store.clone(),
    );
    *auth.metadata.write().await = Some(crate::auth::ProviderMetadata {
        authorization_endpoint: format!("{}/authorize", f.connector.base_url),
        token_endpoint: format!("{}/token", f.connector.base_url),
        userinfo_endpoint: Some(format!("{}/userinfo", f.connector.base_url)),
        introspection_endpoint: None,
    });
    auth_store
        .put_subject(crate::auth::persistence::SubjectAuthState::new(
            SubjectId::new("person-one"),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
            crate::auth::profile::Profile::default(),
            Some(crate::auth::persistence::DurableSubjectTokens {
                access_token: SecretValue::new("fixture-human-oauth".into()),
                refresh_token: Some(SecretValue::new("fixture-human-refresh".into())),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        ))
        .await
        .unwrap();
    let mut context = CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
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
    context.credential = auth
        .resource_request_context(&crate::auth::RequestAuth {
            credential_source: layer2_core::resource_access::CredentialSource::Retained,
            subject: SubjectId::new("person-one"),
            authorization: context.caller.authorization.clone(),
            profile: crate::auth::profile::Profile::default(),
            delegated_credential: Some("fixture-human-oauth".into()),
        })
        .credential;
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
    let custody = Arc::new(CredentialMemory::default());
    let manager = crate::bridge::ManagedBridgeCredentials {
        store: custody.clone(),
        provider: Arc::new(
            NyxBridgeProvider::with_endpoint(reqwest::Url::parse(&f.connector.base_url).unwrap())
                .unwrap(),
        ),
        bridges: definitions,
        auth,
        provisioning: None,
    };
    let imported = manager
        .import(
            &service,
            &context,
            Some("fixture-human-oauth"),
            layer2_core::bridge::setup::BridgeCredentialImport {
                id: id.clone(),
                expected_revision: 1,
                material: layer2_core::bridge::setup::BridgeKeyMaterial::new(format!(
                    "nyxid_ag_{}",
                    "a".repeat(64)
                )),
                retain_control_plane_authority: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(imported.owner, personal());
    assert_eq!(imported.created_by, context.actor);
    assert!(imported.key_authority_v1.is_some());
    // Model a cold persisted import at version one, followed by provider usage.
    // Only explicit import supplied its complete snapshot; old partial records
    // must not be silently given one by resolution.
    custody
        .0
        .lock()
        .await
        .as_mut()
        .unwrap()
        .metadata
        .key_state_version = 1;
    let snapshot = custody
        .0
        .lock()
        .await
        .as_mut()
        .unwrap()
        .metadata
        .key_authority_v1
        .take();
    let legacy = custody.0.lock().await.as_ref().unwrap().metadata.clone();
    assert!(serde_json::to_value(&legacy)
        .unwrap()
        .get("key_authority_v1")
        .is_none());
    assert!(manager.resolve(&personal(), &config).await.is_err());
    custody
        .0
        .lock()
        .await
        .as_mut()
        .unwrap()
        .metadata
        .key_authority_v1 = snapshot;
    for _ in 0..3 {
        let before = f.state.requests.lock().unwrap().len();
        let lease = manager.resolve(&personal(), &config).await.unwrap();
        assert_eq!(
            lease.key_state_version, 1,
            "local outbox pin remains stable"
        );
        manager.revalidate(&lease).await.unwrap();
        let requests = f.state.requests.lock().unwrap();
        let fresh = &requests[before..];
        assert_eq!(
            fresh
                .iter()
                .filter(|(_, uri, _)| uri == "/api/v1/users/me/authorization")
                .count(),
            1,
            "one fresh human verification per key resolution"
        );
        assert_eq!(
            fresh
                .iter()
                .filter(|(_, uri, _)| uri == "/api/v1/api-keys")
                .count(),
            2,
            "binding authority remains checked before and after the possession probe"
        );
        assert_eq!(
            fresh
                .iter()
                .filter(|(_, uri, _)| uri.starts_with("/api/v1/channel-relay/messages/"))
                .count(),
            1,
            "every resolution still proves selected-key possession"
        );
    }
    let original_rows = f.state.replies.lock().unwrap().clone();
    for path in [
        "/api/v1/users/me/authorization",
        "/api/v1/channel-relay/messages/route-one",
    ] {
        f.state.replies.lock().unwrap().get_mut(path).unwrap().0 = StatusCode::FORBIDDEN;
        assert!(
            manager.resolve(&personal(), &config).await.is_err(),
            "combined resolution must not return a lease without successful {path}"
        );
        *f.state.replies.lock().unwrap() = original_rows.clone();
    }
    for patch in [
        json!({"scopes":"read"}),
        json!({"allowed_service_ids":[]}),
        json!({"allow_all_services":true}),
        json!({"allow_auto_connected_services":true}),
        json!({"allowed_node_ids":["another-node"]}),
        json!({"allow_all_nodes":true}),
        json!({"expires_at":"2099-01-01T00:00:00Z"}),
        json!({"platform":null}),
        json!({"rate_limit_per_second":10}),
        json!({"rate_limit_burst":20}),
        json!({"purpose":"scheduled_invocation"}),
        json!({"scheduled_write_enabled":true}),
        json!({"rotation_predecessor_id":"different-key"}),
        json!({"is_active":false}),
        json!({"state_version":0}),
        json!({"bindings_count":1}),
        json!({"callback_url":"https://other.example/callback"}),
    ] {
        {
            let mut rows = f.state.replies.lock().unwrap();
            *rows = original_rows.clone();
            let key = &mut rows.get_mut("/api/v1/api-keys").unwrap().1["keys"][0];
            for (field, value) in patch.as_object().unwrap() {
                key[field] = value.clone();
            }
        }
        assert!(
            manager.resolve(&personal(), &config).await.is_err(),
            "{patch}"
        );
    }
    *f.state.replies.lock().unwrap() = original_rows;
    let lease = manager.resolve(&personal(), &config).await.unwrap();
    assert_eq!(lease.credential_subject, SubjectId::new("person-one"));
    assert_eq!(lease.key_id, "key-one");
    let control = manager
        .resolve_control_plane(&personal(), &config)
        .await
        .unwrap();
    assert_eq!(control.owner(), &personal());
    assert_eq!(control.actor_subject(), &SubjectId::new("person-one"));
    assert_eq!(control.credential(), "fixture-human-oauth");
    assert_ne!(control.credential(), lease.value.expose());
    for (_, uri, auth) in f.state.requests.lock().unwrap().iter() {
        assert_eq!(
            auth.starts_with("Bearer nyxid_ag_"),
            uri.starts_with("/api/v1/channel-relay/")
        );
    }
    let key_reference = config.connector.custody.clone();
    let mut stored = manager.store.get(&key_reference).await.unwrap().unwrap();
    let grant = stored.control_plane.clone().unwrap();
    assert_eq!(
        layer2_core::seconds_from_rfc3339(&grant.expires_at).unwrap()
            - layer2_core::seconds_from_rfc3339(&grant.issued_at).unwrap(),
        super::super::super::control_plane::GRANT_LIFETIME.as_secs()
    );
    // Historical key-only custody cannot silently borrow the creator's login.
    stored.control_plane = None;
    let management = service
        .management_lease(&context, None, &id, 1)
        .await
        .unwrap();
    manager.store.admit(&management, stored).await.unwrap();
    assert!(matches!(
        manager.resolve(&personal(), &config).await,
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::RenewableAuthorityUnavailable
        ))
    ));
    assert!(manager.revalidate_control_plane(&control).await.is_err());
    let mut stored = manager.store.get(&key_reference).await.unwrap().unwrap();
    let mut expired = grant.clone();
    expired.issued_at = "2020-01-01T00:00:00Z".into();
    expired.expires_at = "2020-01-31T00:00:00Z".into();
    stored.control_plane = Some(expired);
    manager.store.admit(&management, stored).await.unwrap();
    assert!(manager
        .resolve_control_plane(&personal(), &config)
        .await
        .is_err());
    let mut stored = manager.store.get(&key_reference).await.unwrap().unwrap();
    stored.control_plane = Some(grant);
    manager.store.admit(&management, stored).await.unwrap();
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
