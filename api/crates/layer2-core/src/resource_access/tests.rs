use std::time::{Duration, SystemTime};

use crate::{Authorization, Caller, Capability, SubjectId};

use super::*;
use ResourceCapability::*;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
}
fn person(subject: &str) -> ResourceOwner {
    ResourceOwner::Person {
        subject: SubjectId::new(subject),
    }
}
fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "credential-a".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: Vec::new(),
    }
}
fn member(subject: &str, org: &str, role: OrganizationRole) -> CallerContext {
    let mut result = context(subject);
    result.memberships.push(
        OrganizationMembership::new(
            result.credential.clone(),
            SubjectId::new(org),
            role,
            now(),
            now() + Duration::from_secs(3600),
        )
        .unwrap(),
    );
    result
}
fn scope(owner: ResourceOwner) -> ResourceScope {
    ResourceScope {
        sandbox_id: crate::SandboxId("sandbox".into()),
        agent_id: Some(crate::AgentId("agent".into())),
        owner,
        sharing_isolation: SharingIsolation::Unproved,
    }
}
fn decision(
    caller: &CallerContext,
    scope: &ResourceScope,
    shares: &[ResourceShare],
) -> AccessDecision {
    AccessDecision::evaluate(caller, scope, shares, AccessPurpose::Read, now()).unwrap()
}
fn share(mode: ShareMode) -> ResourceShare {
    ResourceShare {
        id: "share-a".into(),
        owner: person("owner"),
        selector: ResourceSelector::Agent {
            agent_id: crate::AgentId("agent".into()),
        },
        target_org_id: SubjectId::new("org"),
        link: None,
        mode,
        issuer_actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        created_at: crate::rfc3339_at(now()),
        revoked_at: None,
        revision: 1,
    }
}

#[test]
fn legacy_identity_is_personal_even_when_its_subject_looks_like_an_org() {
    let resource = scope(person("org"));
    let own = decision(&context("org"), &resource, &[]);
    assert!(own.allows(ManageSharing));
    assert_eq!(own.revalidate_at, None);
    let unrelated = decision(
        &member("admin", "org", OrganizationRole::Admin),
        &resource,
        &[],
    );
    assert_eq!(unrelated.require(ViewHistory), Err(AccessError::NotFound));
    assert_eq!(
        AccessDecision::personal(&context("other").caller, &SubjectId::new("org"))
            .require(Interact),
        Err(AccessError::NotFound)
    );
    let mut no_capability = context("org");
    no_capability.caller.authorization = Authorization::none();
    assert_eq!(
        decision(&no_capability, &resource, &[]).require(ViewHistory),
        Err(AccessError::NotFound)
    );
}

#[test]
fn org_role_matrix_retains_the_actual_actor_and_excludes_platform_authority() {
    let resource = scope(ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    });
    for role in [
        OrganizationRole::Admin,
        OrganizationRole::Member,
        OrganizationRole::Viewer,
    ] {
        let caller = member("human", "org", role);
        let access = decision(&caller, &resource, &[]);
        assert!(access.allows(ViewHistory));
        assert!(access.allows(ViewFiles));
        for capability in [
            Interact,
            ResolveApproval,
            OperateAgent,
            ManageSandbox,
            ManageBridge,
            ManageSharing,
        ] {
            assert_eq!(access.allows(capability), role == OrganizationRole::Admin);
        }
        assert_eq!(
            caller.actor,
            ResourceActor::Human {
                subject: SubjectId::new("human")
            }
        );
        assert_eq!(caller.caller.subject.as_str(), "human");
        assert!(!caller
            .caller
            .authorization
            .allows(Capability::ControlAnySandbox));
        assert_eq!(access.revalidate_at, Some(now() + MAX_MEMBERSHIP_AGE));
    }
    assert_eq!(
        decision(&context("former"), &resource, &[]).require(ViewHistory),
        Err(AccessError::NotFound)
    );
}

#[test]
fn membership_has_a_fixed_expiry_and_mutations_need_new_evidence() {
    let resource = scope(ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    });
    let caller = member("human", "org", OrganizationRole::Admin);
    let check = |at, purpose| AccessDecision::evaluate(&caller, &resource, &[], purpose, at);
    assert!(check(now() + Duration::from_secs(59), AccessPurpose::Read).is_ok());
    assert_eq!(
        check(now() + MAX_MEMBERSHIP_AGE, AccessPurpose::Read),
        Err(AccessError::RevalidationRequired)
    );
    let later = now() + Duration::from_secs(1);
    assert_eq!(
        check(later, AccessPurpose::Mutation { started_at: later }),
        Err(AccessError::RevalidationRequired)
    );
    assert!(check(now(), AccessPurpose::Mutation { started_at: now() })
        .unwrap()
        .allows(Interact));
    let mut forged = caller.clone();
    forged.memberships[0].expires_at += Duration::from_secs(1);
    assert_eq!(
        AccessDecision::evaluate(&forged, &resource, &[], AccessPurpose::Read, now()),
        Err(AccessError::RevalidationRequired)
    );
    forged = caller.clone();
    forged.memberships[0].observed_at += Duration::from_secs(1);
    assert_eq!(
        AccessDecision::evaluate(&forged, &resource, &[], AccessPurpose::Read, now()),
        Err(AccessError::RevalidationRequired)
    );
}

#[test]
fn identity_issuer_credential_and_org_evidence_cannot_cross_boundaries() {
    let resource = scope(ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    });
    for mutation in 0..6 {
        let mut caller = member("human", "org", OrganizationRole::Admin);
        match mutation {
            0 => caller.memberships[0].credential.subject = SubjectId::new("other"),
            1 => caller.memberships[0].credential.issuer = "https://other.test".into(),
            2 => caller.memberships[0].credential.credential_id = "replacement".into(),
            3 => caller.memberships[0].organization = SubjectId::new("other-org"),
            4 => {
                caller.actor = ResourceActor::Human {
                    subject: SubjectId::new("org"),
                }
            }
            _ => caller.caller.subject = SubjectId::new("org"),
        }
        assert_eq!(
            decision(&caller, &resource, &[]).require(Interact),
            Err(AccessError::NotFound)
        );
    }
    let mut duplicate = member("human", "org", OrganizationRole::Admin);
    duplicate.memberships.push(duplicate.memberships[0].clone());
    assert_eq!(
        AccessDecision::evaluate(&duplicate, &resource, &[], AccessPurpose::Read, now()),
        Err(AccessError::RevalidationRequired)
    );
}

#[test]
fn channel_and_delegation_attribution_cannot_become_an_ambient_owner() {
    let resource = scope(person("owner"));
    for actor in [
        ResourceActor::Channel {
            bridge_id: "bridge".into(),
            grant_id: "grant".into(),
            platform: "telegram".into(),
            chat_id: "chat".into(),
            sender_id: "sender".into(),
        },
        ResourceActor::Api {
            subject: SubjectId::new("owner"),
            delegation_id: Some("delegation".into()),
        },
    ] {
        let mut caller = context("owner");
        caller.actor = actor;
        caller.caller.authorization =
            Authorization::new([Capability::UseOwnSandboxes, Capability::ControlAnySandbox]);
        assert_eq!(
            decision(&caller, &resource, &[]).require(ViewHistory),
            Err(AccessError::NotFound)
        );
    }
}

#[test]
fn narrow_view_share_does_not_grant_files_siblings_sandbox_or_resharing() {
    let grant = share(ShareMode::View);
    let resource = scope(person("owner"));
    for role in [
        OrganizationRole::Admin,
        OrganizationRole::Member,
        OrganizationRole::Viewer,
    ] {
        let caller = member("recipient", "org", role);
        let access = decision(&caller, &resource, std::slice::from_ref(&grant));
        assert!(access.allows(ViewHistory));
        for capability in [
            ViewFiles,
            Interact,
            ResolveApproval,
            OperateAgent,
            ManageSandbox,
            ManageSharing,
            ManageBridge,
        ] {
            assert_eq!(access.require(capability), Err(AccessError::Forbidden));
        }
        for agent_id in [None, Some(crate::AgentId("sibling".into()))] {
            let other = ResourceScope {
                agent_id,
                ..resource.clone()
            };
            assert_eq!(
                decision(&caller, &other, std::slice::from_ref(&grant)).require(ViewHistory),
                Err(AccessError::NotFound)
            );
        }
    }
}

#[test]
fn interactive_narrow_share_requires_current_isolation_or_whole_sandbox_grant() {
    let narrow = share(ShareMode::OrgRoles);
    let mut resource = scope(person("owner"));
    let admin = member("recipient", "org", OrganizationRole::Admin);
    assert_eq!(
        narrow.require_safe_interaction(&resource, &[]),
        Err(AccessError::UnsafeInteractiveShare)
    );
    assert!(!decision(&admin, &resource, std::slice::from_ref(&narrow)).allows(Interact));
    resource.sharing_isolation = SharingIsolation::IndependentlyIsolated;
    narrow.require_safe_interaction(&resource, &[]).unwrap();
    let access = decision(&admin, &resource, std::slice::from_ref(&narrow));
    assert!(access.allows(Interact));
    assert!(!access.allows(ManageSandbox));
    resource.sharing_isolation = SharingIsolation::Unproved;
    let mut whole = narrow.clone();
    whole.id = "whole".into();
    whole.selector = ResourceSelector::Sandbox {
        sandbox_id: resource.sandbox_id.clone(),
    };
    narrow
        .require_safe_interaction(&resource, std::slice::from_ref(&whole))
        .unwrap();
    let access = decision(&admin, &resource, &[narrow.clone(), whole.clone()]);
    assert!(access.allows(Interact));
    assert!(access.allows(ManageSandbox));
    assert!(!access.allows(ManageSharing));
    whole.revoked_at = Some(crate::rfc3339_at(now()));
    assert_eq!(
        narrow.require_safe_interaction(&resource, std::slice::from_ref(&whole)),
        Err(AccessError::UnsafeInteractiveShare)
    );
    assert!(!decision(&admin, &resource, &[narrow, whole]).allows(Interact));
}

#[test]
fn revocation_and_expired_optional_grants_preserve_independent_access() {
    let resource = scope(person("owner"));
    let mut first = share(ShareMode::View);
    first.revoked_at = Some(crate::rfc3339_at(now()));
    let admin = member("recipient", "org", OrganizationRole::Admin);
    assert_eq!(
        decision(&admin, &resource, std::slice::from_ref(&first)).require(ViewHistory),
        Err(AccessError::NotFound)
    );
    let mut second = share(ShareMode::View);
    second.id = "independent".into();
    second.target_org_id = SubjectId::new("second-org");
    let mut recipient = member("recipient", "second-org", OrganizationRole::Viewer);
    let mut stale = OrganizationMembership::new(
        recipient.credential.clone(),
        SubjectId::new("org"),
        OrganizationRole::Admin,
        now() - Duration::from_secs(61),
        now() + Duration::from_secs(60),
    )
    .unwrap();
    stale.expires_at = now() - Duration::from_secs(1);
    recipient.memberships.push(stale.clone());
    first.revoked_at = None;
    assert!(decision(&recipient, &resource, &[first.clone(), second]).allows(ViewHistory));
    let mut owner = context("owner");
    stale.credential = owner.credential.clone();
    owner.memberships.push(stale);
    let own = decision(&owner, &resource, &[first]);
    assert!(own.allows(Interact));
    assert_eq!(own.revalidate_at, None);
}

#[test]
fn operator_access_is_explicit_and_does_not_grant_share_or_bridge_custody() {
    let resource = scope(person("owner"));
    let mut reader = context("operator");
    reader.caller.authorization = Authorization::new([Capability::ReadAllSandboxes]);
    let access = decision(&reader, &resource, &[]);
    assert!(access.allows(ViewHistory));
    assert!(!access.allows(Interact));
    reader.caller.authorization = Authorization::new([Capability::ControlAnySandbox]);
    let access = decision(&reader, &resource, &[]);
    assert!(access.allows(ManageSandbox));
    assert!(!access.allows(ManageSharing));
    assert!(!access.allows(ManageBridge));
}

#[test]
fn attribution_wire_shapes_remain_compatible_with_command_receipts() {
    let serialized = serde_json::to_value(person("owner")).unwrap();
    assert_eq!(
        serialized,
        serde_json::json!({"kind":"person","subject":"owner"})
    );
    let old: crate::sandbox::agent_io::AgentOwner = serde_json::from_value(serialized).unwrap();
    assert_eq!(old, person("owner"));
    assert!(serde_json::from_value::<OrganizationRole>(serde_json::json!("owner")).is_err());
}

#[tokio::test(start_paused = true)]
async fn wall_clock_rollback_cannot_extend_a_membership_observation() {
    let caller = member("human", "org", OrganizationRole::Admin);
    let evidence = &caller.memberships[0];
    assert!(evidence.is_current(&caller.credential, now()));
    tokio::time::advance(MAX_MEMBERSHIP_AGE).await;
    assert!(!evidence.is_current(&caller.credential, now()));
}

#[tokio::test(start_paused = true)]
async fn held_decisions_expire_without_removing_independent_operator_reads() {
    let resource = scope(ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    });
    let mut caller = member("human", "org", OrganizationRole::Admin);
    caller.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::ReadAllSandboxes]);
    let access = decision(&caller, &resource, &[]);
    assert!(access.allows(Interact));
    assert!(access.allows(ManageBridge));
    tokio::time::advance(MAX_MEMBERSHIP_AGE).await;
    assert_eq!(
        access.require(Interact),
        Err(AccessError::RevalidationRequired)
    );
    assert_eq!(access.require(ViewHistory), Ok(()));
    assert!(!access.capabilities().any(|cap| cap == Interact));
    let refreshed =
        AccessDecision::evaluate(&caller, &resource, &[], AccessPurpose::Read, now()).unwrap();
    assert!(refreshed.allows(ViewHistory));
    assert!(!refreshed.allows(Interact));
}

#[test]
fn audit_observations_separate_operator_and_membership_capabilities() {
    let mut caller = member("reader", "org", OrganizationRole::Viewer);
    caller.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::ControlAnySandbox]);
    let mut share = share(ShareMode::OrgRoles);
    share.selector = ResourceSelector::Sandbox {
        sandbox_id: crate::SandboxId("sandbox".into()),
    };
    let access = decision(&caller, &scope(person("owner")), &[share.clone()]);
    let observations = access.audit_observations();
    assert_eq!(observations.len(), 2);
    assert!(matches!(
        observations[0].basis,
        ResourceAuthorizationBasis::GlobalOperator
    ));
    assert!(observations[0].capabilities.contains(&Interact));
    assert!(!observations[0].capabilities.contains(&ManageSharing));
    assert!(!observations[1].capabilities.contains(&Interact));
    assert!(observations[1].capabilities.contains(&ViewHistory));
    assert!(
        matches!(&observations[1].basis, ResourceAuthorizationBasis::Organization {
        organization, role: OrganizationRole::Viewer, share: Some(observed), ..
    } if organization.as_str() == "org" && observed.id == share.id && observed.revision == share.revision)
    );
}

#[test]
fn audit_org_owner_and_unsafe_narrow_share_never_invent_capabilities() {
    let caller = member("admin", "org", OrganizationRole::Admin);
    let owner = decision(
        &caller,
        &scope(ResourceOwner::Organization {
            subject: SubjectId::new("org"),
        }),
        &[],
    );
    assert!(owner.audit_observations()[0]
        .capabilities
        .contains(&ManageSharing));
    assert!(matches!(
        owner.audit_observations()[0].basis,
        ResourceAuthorizationBasis::Organization { share: None, .. }
    ));
    let narrow = decision(
        &caller,
        &scope(person("owner")),
        &[share(ShareMode::OrgRoles)],
    );
    assert_eq!(
        narrow.audit_observations()[0].capabilities,
        vec![ViewDetails, ViewHistory, ViewArtifacts]
    );
    assert!(!narrow.allows(Interact));
    let own = decision(
        &context("owner"),
        &scope(person("owner")),
        &[share(ShareMode::OrgRoles)],
    );
    assert_eq!(own.audit_observations().len(), 1);
    assert!(matches!(
        own.audit_observations()[0].basis,
        ResourceAuthorizationBasis::PersonalOwner
    ));
}
