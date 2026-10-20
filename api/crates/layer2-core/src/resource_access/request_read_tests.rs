use super::*;
use crate::{Authorization, Caller, Capability, SandboxId, SubjectId};
use std::time::{Duration, SystemTime};

fn read_context(expires_at: SystemTime) -> CallerContext {
    let subject = SubjectId::new("owner");
    CallerContext {
        credential_source: CredentialSource::RequestBoundRead { expires_at },
        caller: Caller::new(
            subject.clone(),
            Authorization::new([
                Capability::UseOwnSandboxes,
                Capability::CreateSandbox,
                Capability::DeleteOwnSandbox,
                Capability::ControlAnySandbox,
            ]),
        ),
        credential: CredentialContext {
            issuer: "provider".into(),
            subject: subject.clone(),
            credential_id: "exact-read".into(),
        },
        actor: ResourceActor::Api {
            subject,
            delegation_id: None,
        },
        memberships: vec![],
    }
}

#[tokio::test]
async fn short_read_authority_never_inherits_owner_write_capabilities_or_unbounded_lifetime() {
    let now = SystemTime::now();
    let context = read_context(now + Duration::from_secs(30));
    let scope = ResourceScope {
        sandbox_id: SandboxId("sandbox".into()),
        agent_id: None,
        owner: ResourceOwner::Person {
            subject: context.caller.subject.clone(),
        },
        sharing_isolation: SharingIsolation::Unproved,
    };
    let read = AccessDecision::evaluate(&context, &scope, &[], AccessPurpose::Read, now).unwrap();
    assert!(read.allows(ResourceCapability::ViewDetails));
    assert!(read.allows(ResourceCapability::ViewHistory));
    assert!(read
        .revalidation_deadline(ResourceCapability::ViewHistory)
        .is_some());
    for capability in [
        ResourceCapability::Interact,
        ResourceCapability::ResolveApproval,
        ResourceCapability::OperateAgent,
        ResourceCapability::ManageSandbox,
        ResourceCapability::ManageSharing,
        ResourceCapability::ManageBridge,
        ResourceCapability::ViewFiles,
        ResourceCapability::ViewArtifacts,
    ] {
        assert!(!read.allows(capability), "{capability:?}");
    }
    let mutation = AccessDecision::evaluate(
        &context,
        &scope,
        &[],
        AccessPurpose::Mutation { started_at: now },
        now,
    )
    .unwrap();
    assert!(mutation.capabilities().next().is_none());
    let expired = AccessDecision::evaluate(
        &context,
        &scope,
        &[],
        AccessPurpose::Read,
        now + Duration::from_secs(31),
    )
    .unwrap();
    assert!(expired.capabilities().next().is_none());
    let expired_context = read_context(now - Duration::from_secs(1));
    assert!(!expired_context.validate_principal());
}
