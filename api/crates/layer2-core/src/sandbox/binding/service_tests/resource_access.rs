use super::*;
use crate::resource_access::*;

struct NoExternalAuthority;
#[async_trait::async_trait]
impl OrganizationAuthority for NoExternalAuthority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("binding access must stay local")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("binding access must stay local")
    }
}
#[async_trait::async_trait]
impl SandboxResourceReader for NoExternalAuthority {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("binding access must not read parent runtime resources")
    }
}

pub(super) fn context(principal: &BindingPrincipal) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Binding,
        caller: principal.caller(),
        credential: CredentialContext {
            issuer: "binding:local".into(),
            subject: principal.owner().clone(),
            credential_id: principal.key_sha256().into(),
        },
        actor: ResourceActor::Api {
            subject: principal.owner().clone(),
            delegation_id: None,
        },
        memberships: vec![],
    }
}

pub(super) async fn ready_conversation(
    f: &Fixture,
    binding: &ProviderBinding,
) -> BindingConversation {
    let (mut conversation, _) = f
        .service
        .ensure_conversation(&f.caller, &binding.binding_id, "scoped-conversation")
        .await
        .unwrap();
    f.service
        .reconcile_conversation(binding, &conversation)
        .await
        .unwrap();
    let revision = conversation.revision;
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    f.store
        .save_binding_conversation(binding.revision, Some(revision), conversation.clone())
        .await
        .unwrap();
    conversation
}

#[tokio::test]
async fn binding_resource_access_grants_only_mapped_agent_interaction() {
    mapped_agent_interaction("human").await;
}

#[tokio::test]
async fn organization_binding_grants_mapped_agent_access_without_organization_or_parent_authority()
{
    mapped_agent_interaction("organization").await;
}

async fn mapped_agent_interaction(owner_kind: &str) {
    let f = fixture();
    let mut profile = profile("Original instruction");
    profile["metadata"]["owner"]["kind"] = owner_kind.into();
    let binding = f.ready_with_profile(profile, None).await;
    let conversation = ready_conversation(&f, &binding).await;
    let bindings = BindingAccessService::new(f.store.clone());
    let context = context(
        &bindings
            .authenticate(&f.key.sha256())
            .await
            .unwrap()
            .unwrap(),
    );
    let access = ResourceAccessService::new(f.store.clone(), Arc::new(NoExternalAuthority))
        .with_bindings(bindings);
    let target = ResourceTarget::Agent {
        agent_id: conversation.agent_id.clone(),
    };
    let held = access
        .authorize(
            &context,
            None,
            &target,
            ResourceCapability::Interact,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    assert_eq!(
        held.snapshot.scope(&target).unwrap().owner,
        if owner_kind == "organization" {
            ResourceOwner::Organization {
                subject: binding.owner.clone(),
            }
        } else {
            ResourceOwner::Person {
                subject: binding.owner.clone(),
            }
        }
    );
    assert_eq!(
        held.decision.capabilities().collect::<Vec<_>>(),
        vec![
            ResourceCapability::ViewDetails,
            ResourceCapability::ViewHistory,
            ResourceCapability::Interact,
            ResourceCapability::OperateAgent,
        ]
    );
    let audit = held
        .authorization_audit(&context.actor, &target, ResourceCapability::Interact)
        .unwrap();
    audit.validate().unwrap();
    assert_eq!(
        audit.observations[0].basis,
        ResourceAuthorizationBasis::ExternalBinding {
            binding_id: binding.binding_id.clone(),
            generation: binding.generation,
        }
    );
    for capability in [
        ResourceCapability::ResolveApproval,
        ResourceCapability::ViewFiles,
        ResourceCapability::ViewArtifacts,
        ResourceCapability::ManageSandbox,
        ResourceCapability::ManageSharing,
        ResourceCapability::ManageBridge,
    ] {
        assert!(access
            .authorize(&context, None, &target, capability, AccessPurpose::Read)
            .await
            .is_err());
    }
    let parent = ResourceTarget::Sandbox {
        sandbox_id: binding.sandbox_id.clone(),
    };
    assert!(access
        .authorize(
            &context,
            None,
            &parent,
            ResourceCapability::ViewDetails,
            AccessPurpose::Read
        )
        .await
        .is_err());
    let mut forged = context.clone();
    forged.credential_source = CredentialSource::Retained;
    assert!(access
        .authorize(
            &forged,
            None,
            &target,
            ResourceCapability::Interact,
            AccessPurpose::Read
        )
        .await
        .is_err());
    forged = context.clone();
    forged.caller.authorization = Authorization::new([
        Capability::UseOwnSandboxes,
        Capability::UseBindingConversations,
    ]);
    assert!(access
        .authorize(
            &forged,
            None,
            &target,
            ResourceCapability::Interact,
            AccessPurpose::Read
        )
        .await
        .is_err());

    let reads = AgentReadService::new(f.store.clone(), Arc::new(NoExternalAuthority))
        .with_resource_access(Arc::new(access.clone()));
    assert!(matches!(
        reads
            .detail_authorized(&context, None, &conversation.agent_id)
            .await
            .unwrap(),
        AgentDetailReading::Shared(_)
    ));
    let stream = reads
        .authorize_event_stream(&context, None, &conversation.agent_id)
        .await
        .unwrap();
    reads.validate_event_stream(&stream).await.unwrap();
    f.service
        .retire_conversation(
            &f.caller,
            &binding.binding_id,
            &conversation.conversation_id,
        )
        .await
        .unwrap();
    assert!(reads.validate_event_stream(&stream).await.is_err());
    assert!(access
        .validate_read_result(&held, &target, ResourceCapability::Interact)
        .await
        .is_err());
}

#[tokio::test]
async fn binding_access_rejects_owner_kind_mismatch_even_when_subject_and_lineage_match() {
    for organization in [false, true] {
        let f = fixture();
        let mut profile = profile("Scoped binding");
        profile["metadata"]["owner"]["kind"] = if organization {
            "organization"
        } else {
            "human"
        }
        .into();
        let binding = f.ready_with_profile(profile, None).await;
        let conversation = ready_conversation(&f, &binding).await;
        let bindings = BindingAccessService::new(f.store.clone());
        let context = context(
            &bindings
                .authenticate(&f.key.sha256())
                .await
                .unwrap()
                .unwrap(),
        );
        let access = ResourceAccessService::new(f.store.clone(), Arc::new(NoExternalAuthority))
            .with_bindings(bindings);
        let target = ResourceTarget::Agent {
            agent_id: conversation.agent_id,
        };
        let held = access
            .authorize(
                &context,
                None,
                &target,
                ResourceCapability::Interact,
                AccessPurpose::Read,
            )
            .await
            .unwrap();
        let parent = &held.snapshot.lineage.sandbox;
        // A retained record with the opposite owner kind must not be treated as
        // equivalent merely because its subject string and all parent IDs agree.
        f.store.set_resource_policy_for_test(ResourcePolicyRecord {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            owner: if organization {
                ResourceOwner::Person {
                    subject: binding.owner.clone(),
                }
            } else {
                ResourceOwner::Organization {
                    subject: binding.owner.clone(),
                }
            },
            organization_evidence: (!organization).then(|| OrganizationOwnershipEvidence {
                issuer: "fixture-authority".into(),
                organization: binding.owner.clone(),
                admitted_by: ResourceActor::Human {
                    subject: binding.manager.clone(),
                },
                verified_at: crate::now_rfc3339(),
            }),
            revision: 2,
        });
        assert!(matches!(
            access
                .authorize(
                    &context,
                    None,
                    &target,
                    ResourceCapability::Interact,
                    AccessPurpose::Read
                )
                .await,
            Err(ResourceAccessError::Access(AccessError::NotFound))
        ));
        assert!(access
            .validate_read_result(&held, &target, ResourceCapability::Interact)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn binding_resource_proofs_preserve_refused_updates_and_reject_stale_keys_or_generations() {
    for rotate_key in [true, false] {
        let f = fixture();
        let binding = f.ready().await;
        let conversation = ready_conversation(&f, &binding).await;
        let bindings = BindingAccessService::new(f.store.clone());
        let context = context(
            &bindings
                .authenticate(&f.key.sha256())
                .await
                .unwrap()
                .unwrap(),
        );
        let access = ResourceAccessService::new(f.store.clone(), Arc::new(NoExternalAuthority))
            .with_bindings(bindings);
        let target = ResourceTarget::Agent {
            agent_id: conversation.agent_id,
        };
        let mut held = access
            .authorize(
                &context,
                None,
                &target,
                ResourceCapability::ViewHistory,
                AccessPurpose::Read,
            )
            .await
            .unwrap();
        if rotate_key {
            let next = binding
                .rotate_credential("b".repeat(64), crate::now_rfc3339())
                .unwrap();
            f.store
                .save_binding(Some(binding.revision), next)
                .await
                .unwrap();
        } else {
            let changed = profile("Changed instruction");
            let digest = format!("sha256:{}", content_digest(&changed).unwrap());
            assert!(matches!(
                f.service
                    .put(&f.caller, &binding.binding_id, changed, &digest, &f.key)
                    .await,
                Err(BindingError::ReconfigurationUnsupported)
            ));
            assert_eq!(f.binding().await, binding);
            access
                .validate_read_result(&held, &target, ResourceCapability::ViewHistory)
                .await
                .unwrap();
            access
                .authorize(
                    &context,
                    None,
                    &target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read,
                )
                .await
                .unwrap();
            // Persisted proofs from another generation must still fail even
            // though new reconfiguration can no longer replace an Agent.
            let proof = held.binding.as_mut().unwrap();
            proof.binding.generation += 1;
            proof.conversation.generation = proof.binding.generation;
        }
        assert!(access
            .validate_read_result(&held, &target, ResourceCapability::ViewHistory)
            .await
            .is_err());
        if rotate_key {
            assert!(access
                .authorize(
                    &context,
                    None,
                    &target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read
                )
                .await
                .is_err());
        }
    }
}
