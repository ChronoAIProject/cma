use super::attachments::runtime;
use super::*;
use crate::sandbox::agent_filesystem::*;
use crate::{resource_access::*, Authorization, Caller, Capability};
pub(super) struct NoOrganizations;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganizations {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> std::result::Result<CallerContext, MembershipError> {
        panic!("unexpected organization lookup")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> std::result::Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
pub(super) fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            SubjectId(subject.into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://id.example".into(),
            subject: SubjectId(subject.into()),
            credential_id: format!("credential-{subject}"),
        },
        actor: ResourceActor::Human {
            subject: SubjectId(subject.into()),
        },
        memberships: vec![],
    }
}
async fn fixture() -> (
    Arc<AgentFileService>,
    Arc<ResourceAccessService>,
    CallerContext,
) {
    fixture_with_authority(Arc::new(NoOrganizations)).await
}
async fn fixture_with_authority(
    organizations: Arc<dyn OrganizationAuthority>,
) -> (
    Arc<AgentFileService>,
    Arc<ResourceAccessService>,
    CallerContext,
) {
    let store = Arc::new(store::InMemorySandboxStore::default());
    let mut root = sandbox();
    root.runtime = Some(RuntimeBinding {
        boot_id: Some("b".repeat(32)),
        runtime_id: "runtime-test".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    root.lifecycle.home = Some(Box::new(super::attachments::persistent_home()));
    store.create_sandbox(root).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    store.create_agent(agent()).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(store, organizations));
    (
        AgentFileService::new(
            access.clone(),
            Arc::new(runtime::UnusedRuntime {
                status: None,
                file_root: None,
            }),
        ),
        access,
        context("owner"),
    )
}

pub(super) struct OrganizationAdmin;

#[tokio::test]
async fn computer_environment_reports_authorized_scope_without_runtime_addresses() {
    let (files, _, owner) = fixture().await;
    let environment = files
        .environment(&owner.caller, Some(&owner), None, &agent().id)
        .await
        .unwrap();
    let value = serde_json::to_value(environment).unwrap();
    assert_eq!(value["agent_id"], agent().id.as_str());
    assert_eq!(value["files"]["scope"], "mounted_home");
    assert_eq!(value["files"]["state"], "available");
    assert_eq!(value["files"]["writable"], true);
    assert_eq!(value["operation_output"]["scope"], "agent_turn_history");
    assert_eq!(value["operation_output"]["state"], "available");
    assert_eq!(value["replay"]["state"], "available");
    assert_eq!(value["replay"]["scope"], "agent_turn_history");
    assert_eq!(value["replay"]["reason"], "operation_records_only");
    assert_eq!(value["replay"]["writable"], false);
    for feature in ["browser", "control"] {
        assert_eq!(value[feature]["state"], "unsupported");
    }
    for secret in ["runtime-test", &"b".repeat(32)] {
        assert!(!value.to_string().contains(secret));
    }
    let outsider = context("outsider");
    assert!(files
        .environment(&outsider.caller, Some(&outsider), None, &agent().id)
        .await
        .is_err());
}
#[async_trait::async_trait]
impl OrganizationAuthority for OrganizationAdmin {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> std::result::Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        let now = std::time::SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            OrganizationRole::Admin,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> std::result::Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn legacy_agent_operations_never_replace_revoked_full_home_write_consent() {
    let (files, access, owner) = fixture_with_authority(Arc::new(OrganizationAdmin)).await;
    let id = AgentId("agt_test".into());
    let viewer = context("viewer");
    access
        .change_share(
            &owner,
            Some("credential"),
            ShareCommand {
                selector: ResourceSelector::Sandbox {
                    sandbox_id: sandbox().id,
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId("organization".into()),
                    mode: ShareMode::OrgRoles,
                },
                expected_share_revision: None,
                idempotency_key: "legacy-interaction".into(),
            },
        )
        .await
        .unwrap();
    let token = "c".repeat(64);
    let link = access
        .create_agent_share_link(
            &owner,
            Some("credential"),
            CreateAgentShareLink {
                agent_id: id.clone(),
                audience: AgentShareAudience::Public,
                mode: ShareMode::ViewWrite,
                token: token.clone(),
                idempotency_key: "full-home-consent".into(),
            },
        )
        .await
        .unwrap();

    for accepted in [false, true, false] {
        if accepted {
            access
                .redeem_agent_share_link(&viewer, Some("credential"), &link.share.id, &token)
                .await
                .unwrap();
        }
        // Legacy authority survives both before acceptance and after revocation.
        let authorized = access
            .authorize(
                &viewer,
                Some("credential"),
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                ResourceCapability::OperateAgent,
                AccessPurpose::Mutation {
                    started_at: std::time::SystemTime::now(),
                },
            )
            .await
            .unwrap();
        assert!(!authorized
            .decision
            .allows(ResourceCapability::ManageSharing));
        files
            .list(&viewer.caller, Some(&viewer), Some("credential"), &id, "")
            .await
            .unwrap();
        let write = files
            .write(
                &viewer.caller,
                Some(&viewer),
                Some("credential"),
                &id,
                "note",
                None,
                "aGk=".into(),
                false,
                None,
            )
            .await;
        let delete = files
            .delete(
                &viewer.caller,
                Some(&viewer),
                Some("credential"),
                &id,
                "note",
                false,
            )
            .await;
        if accepted {
            write.unwrap();
            delete.unwrap();
            let current = access
                .read_share(&owner, None, &link.share.selector, &link.share.id)
                .await
                .unwrap();
            access
                .change_share(
                    &owner,
                    None,
                    ShareCommand {
                        selector: current.selector,
                        change: ShareChange::Revoke {
                            share_id: current.id,
                        },
                        expected_share_revision: Some(current.revision),
                        idempotency_key: "revoke-home-consent".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            assert!(matches!(
                write,
                Err(AgentFileError::Access(ResourceAccessError::Access(
                    AccessError::Forbidden
                )))
            ));
            assert!(matches!(
                delete,
                Err(AgentFileError::Access(ResourceAccessError::Access(
                    AccessError::Forbidden
                )))
            ));
        }
    }
}
#[tokio::test]
async fn owner_can_manage_files_and_cross_owner_has_no_access() {
    let (files, _, owner) = fixture().await;
    let id = AgentId("agt_test".into());
    files
        .list(&owner.caller, Some(&owner), None, &id, "")
        .await
        .unwrap();
    files
        .read(&owner.caller, Some(&owner), None, &id, ".agent/state", 1024)
        .await
        .unwrap();
    files
        .write(
            &owner.caller,
            Some(&owner),
            None,
            &id,
            "note",
            None,
            "aGk=".into(),
            false,
            None,
        )
        .await
        .unwrap();
    files
        .delete(&owner.caller, Some(&owner), None, &id, "note", false)
        .await
        .unwrap();
    let stranger = context("stranger");
    assert!(matches!(
        files
            .list(&stranger.caller, Some(&stranger), None, &id, "")
            .await,
        Err(AgentFileError::Access(_))
    ));
    assert!(matches!(
        files
            .write(
                &stranger.caller,
                Some(&stranger),
                None,
                &id,
                "note",
                None,
                String::new(),
                false,
                None,
            )
            .await,
        Err(AgentFileError::Access(_))
    ));
}
#[tokio::test]
async fn view_only_and_view_write_links_enforce_filesystem_capabilities_and_revocation() {
    for mode in [ShareMode::View, ShareMode::ViewWrite] {
        let (files, access, owner) = fixture().await;
        let id = AgentId("agt_test".into());
        let viewer = context("viewer");
        let token = "a".repeat(64);
        let created = access
            .create_agent_share_link(
                &owner,
                None,
                CreateAgentShareLink {
                    agent_id: id.clone(),
                    audience: AgentShareAudience::Public,
                    mode,
                    token: token.clone(),
                    idempotency_key: "files-link".into(),
                },
            )
            .await
            .unwrap();
        access
            .redeem_agent_share_link(&viewer, None, &created.share.id, &token)
            .await
            .unwrap();
        files
            .list(&viewer.caller, Some(&viewer), None, &id, "")
            .await
            .unwrap();
        files
            .read(
                &viewer.caller,
                Some(&viewer),
                None,
                &id,
                ".agent/state",
                1024,
            )
            .await
            .unwrap();
        let write = files
            .write(
                &viewer.caller,
                Some(&viewer),
                None,
                &id,
                "note",
                None,
                "aGk=".into(),
                false,
                None,
            )
            .await;
        let delete = files
            .delete(&viewer.caller, Some(&viewer), None, &id, "note", false)
            .await;
        assert_eq!(write.is_ok(), mode == ShareMode::ViewWrite, "{write:?}");
        assert_eq!(delete.is_ok(), mode == ShareMode::ViewWrite, "{delete:?}");
        let current = access
            .read_share(
                &owner,
                None,
                &ResourceSelector::Agent {
                    agent_id: id.clone(),
                },
                &created.share.id,
            )
            .await
            .unwrap();
        access
            .change_share(
                &owner,
                None,
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: id.clone(),
                    },
                    change: ShareChange::Revoke {
                        share_id: current.id,
                    },
                    expected_share_revision: Some(current.revision),
                    idempotency_key: "revoke-files".into(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            files
                .read(
                    &viewer.caller,
                    Some(&viewer),
                    None,
                    &id,
                    ".agent/state",
                    1024
                )
                .await,
            Err(AgentFileError::Access(_))
        ));
        assert!(matches!(
            files
                .write(
                    &viewer.caller,
                    Some(&viewer),
                    None,
                    &id,
                    "note",
                    None,
                    String::new(),
                    false,
                    None,
                )
                .await,
            Err(AgentFileError::Access(_))
        ));
        files
            .list(&owner.caller, Some(&owner), None, &id, "")
            .await
            .unwrap();
    }
}

#[path = "agent_environment_observation_tests.rs"]
mod environment_observation;
#[path = "agent_filesystem_tests/read_timings.rs"]
mod read_timings;
#[path = "agent_filesystem_tests/write_target.rs"]
mod write_target;
