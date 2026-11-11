use super::*;
#[path = "shared_buffered.rs"]
mod buffered;
use crate::resource_access::*;
use buffered::BufferedStore;
#[path = "inventory.rs"]
mod inventory;
#[path = "shared_inventory.rs"]
mod shared_inventory;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

enum Reply {
    Member,
    Missing,
    Outage,
    Expiring,
}
struct Authority(Mutex<Reply>);
fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: caller(subject, true),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "request".into(),
        },
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        memberships: vec![],
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "request-credential");
        match *self.0.lock().unwrap() {
            Reply::Missing => return Err(MembershipError::NotMember),
            Reply::Outage => return Err(MembershipError::Unavailable),
            Reply::Member | Reply::Expiring => {}
        }
        let mut value = context(caller.subject.as_str());
        let now = SystemTime::now();
        let lifetime = if matches!(*self.0.lock().unwrap(), Reply::Expiring) {
            Duration::from_secs(1)
        } else {
            MAX_MEMBERSHIP_AGE
        };
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            OrganizationRole::Viewer,
            now,
            now + lifetime,
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(["org", "org-two"]
            .into_iter()
            .map(|id| OrganizationChoice {
                id: SubjectId::new(id),
                role: OrganizationRole::Viewer,
                display_name: None,
            })
            .collect())
    }
}
struct Fixture {
    store: Arc<InMemorySandboxStore>,
    authority: Arc<Authority>,
    access: Arc<ResourceAccessService>,
    reads: Arc<HierarchyReadService>,
    parent: Sandbox,
    workspace: Workspace,
    tree: Worktree,
    agent: Agent,
}
async fn fixture() -> Fixture {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_parent", "owner", SandboxStatus::Active);
    let workspace = workspace(&parent, "wks_one", None);
    let tree = worktree(&workspace, "wkt_one", "agt_one");
    let agent = agent(&workspace, &tree, "agt_one");
    insert_sandbox(&store, parent.clone()).await;
    store.create_workspace(workspace.clone()).await.unwrap();
    store.create_worktree(tree.clone()).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    let authority = Arc::new(Authority(Mutex::new(Reply::Member)));
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let reads = HierarchyReadService::new(store.clone()).with_resource_access(access.clone());
    Fixture {
        store,
        authority,
        access,
        reads,
        parent,
        workspace,
        tree,
        agent,
    }
}
async fn grant(f: &Fixture, whole: bool) -> ResourceShare {
    f.access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            ShareCommand {
                selector: if whole {
                    ResourceSelector::Sandbox {
                        sandbox_id: f.parent.id.clone(),
                    }
                } else {
                    ResourceSelector::Agent {
                        agent_id: f.agent.id.clone(),
                    }
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: if whole { "whole" } else { "narrow" }.into(),
            },
        )
        .await
        .unwrap()
        .share
}

#[tokio::test]
async fn whole_sandbox_grants_allow_safe_hierarchy_and_counts_while_narrow_grants_conceal_parents()
{
    let f = fixture().await;
    let legacy = HierarchyReadService::new(f.store.clone());
    let baseline = legacy
        .sandbox(&caller("owner", true), &f.parent.id)
        .await
        .unwrap();
    grant(&f, false).await;
    let recipient = context("recipient");
    assert!(matches!(
        f.reads
            .sandbox_authorized(&recipient, Some("request-credential"), &f.parent.id)
            .await,
        Err(HierarchyReadError::NotFound)
    ));
    assert!(matches!(
        f.reads
            .workspace_authorized(&recipient, Some("request-credential"), &f.workspace.id)
            .await,
        Err(HierarchyReadError::NotFound)
    ));
    assert!(matches!(
        f.reads
            .worktree_authorized(&recipient, Some("request-credential"), &f.tree.id)
            .await,
        Err(HierarchyReadError::NotFound)
    ));
    grant(&f, true).await;
    let SandboxHierarchyReading::Shared(shared) = f
        .reads
        .sandbox_authorized(&recipient, Some("request-credential"), &f.parent.id)
        .await
        .unwrap()
    else {
        panic!("safe shared projection")
    };
    assert_eq!(shared.counts, baseline.counts);
    assert_eq!(shared.workspaces.len(), 1);
    let WorkspaceHierarchyReading::Shared(shared) = f
        .reads
        .workspace_authorized(&recipient, Some("request-credential"), &f.workspace.id)
        .await
        .unwrap()
    else {
        panic!("safe shared Workspace")
    };
    assert_eq!(shared.counts.agents, 1);
    assert_eq!(shared.worktrees[0].agent_id.as_ref(), Some(&f.agent.id));
    assert!(matches!(
        f.reads
            .worktree_authorized(&recipient, Some("request-credential"), &f.tree.id)
            .await
            .unwrap(),
        WorktreeHierarchyReading::Shared(_)
    ));
    assert_eq!(
        f.reads
            .sandbox(&caller("owner", true), &f.parent.id)
            .await
            .unwrap(),
        baseline
    );
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(f.agent.clone())
    );
    *f.authority.0.lock().unwrap() = Reply::Missing;
    assert!(matches!(
        f.reads
            .sandbox_authorized(&recipient, Some("request-credential"), &f.parent.id)
            .await,
        Err(HierarchyReadError::NotFound)
    ));
    *f.authority.0.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        f.reads
            .workspace_authorized(&recipient, Some("request-credential"), &f.workspace.id)
            .await,
        Err(HierarchyReadError::Access(ResourceAccessError::Membership(
            MembershipError::Unavailable
        )))
    ));
    assert_eq!(
        f.reads
            .sandbox_authorized(&context("owner"), None, &f.parent.id)
            .await
            .unwrap(),
        SandboxHierarchyReading::Personal(Box::new(baseline))
    );
}

#[tokio::test]
async fn organization_owner_is_never_personal_or_a_personal_inventory_lookahead() {
    let f = fixture().await;
    // A legitimate explicit sidecar is required; equal raw owner IDs alone are personal.
    let mut owned = sandbox("sbx_z_org", "owner", SandboxStatus::Active);
    owned.name = "organization-owned".into();
    insert_sandbox(&f.store, owned.clone()).await;
    f.store.set_resource_policy_for_test(ResourcePolicyRecord {
        sandbox_id: owned.id.clone(),
        sandbox_created_at: owned.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: owned.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: owned.owner.clone(),
            admitted_by: context("admin").actor,
            verified_at: NOW.into(),
        }),
    });
    let page = f
        .reads
        .list_sandboxes(
            &caller("owner", true),
            &OwnerSandboxQuery {
                exclude_retired: false,
                status: None,
                page: PageRequest::new(1, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next, None);
    assert_eq!(page.items[0].sandbox.id, f.parent.id);
    assert!(matches!(
        f.reads.sandbox(&caller("owner", true), &owned.id).await,
        Err(HierarchyReadError::NotFound)
    ));
    assert!(matches!(
        f.reads
            .sandbox_authorized(&context("recipient"), Some("request-credential"), &owned.id)
            .await
            .unwrap(),
        SandboxHierarchyReading::Shared(_)
    ));
}

#[tokio::test]
async fn buffered_hierarchy_refuses_revocation_and_expiry_without_leaking_counts() {
    for workspace in [false, true] {
        for (expire, dependency_failure, timeout) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
            (true, false, true),
        ] {
            let f = fixture().await;
            let share = grant(&f, true).await;
            if expire {
                *f.authority.0.lock().unwrap() = Reply::Expiring;
            }
            let entered = Arc::new(tokio::sync::Notify::new());
            let resume = Arc::new(tokio::sync::Notify::new());
            let reads = HierarchyReadService::new(Arc::new(BufferedStore {
                inner: f.store.clone(),
                sandbox: None,
                dependency_failure,
                gate: Some((entered.clone(), resume.clone())),
            }))
            .with_detail_timings(true)
            .with_resource_access(f.access.clone());
            let sandbox_id = f.parent.id.clone();
            let workspace_id = f.workspace.id.clone();
            let task = tokio::spawn(async move {
                if workspace {
                    reads
                        .workspace_authorized(
                            &context("recipient"),
                            Some("request-credential"),
                            &workspace_id,
                        )
                        .await
                        .map(|_| ())
                } else {
                    reads
                        .sandbox_authorized(
                            &context("recipient"),
                            Some("request-credential"),
                            &sandbox_id,
                        )
                        .await
                        .map(|_| ())
                }
            });
            tokio::time::timeout(Duration::from_secs(2), entered.notified())
                .await
                .unwrap();
            if expire {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(2)).await;
            } else {
                f.access
                    .change_share(
                        &context("owner"),
                        None,
                        ShareCommand {
                            selector: share.selector,
                            change: ShareChange::Revoke { share_id: share.id },
                            expected_share_revision: Some(share.revision),
                            idempotency_key: "revoke".into(),
                        },
                    )
                    .await
                    .unwrap();
            }
            if timeout {
                if !expire {
                    tokio::time::pause();
                }
                tokio::time::advance(Duration::from_secs(16)).await;
            } else {
                resume.notify_one();
            }
            let result = task.await.unwrap();
            if expire || timeout {
                tokio::time::resume();
            }
            if expire {
                assert!(matches!(
                    result,
                    Err(HierarchyReadError::Access(ResourceAccessError::Access(
                        AccessError::RevalidationRequired
                    )))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(HierarchyReadError::Access(ResourceAccessError::Store(
                        ResourceAccessStoreError::Conflict
                    )))
                ));
            }
        }
    }
}

#[tokio::test]
async fn corrupt_hierarchy_targets_duplicates_and_cross_workspace_assignments_are_refused() {
    for fault in [
        "target",
        "workspace_duplicate",
        "tree_duplicate",
        "agent_duplicate",
        "cross_workspace",
        "orphan_assignment",
    ] {
        let f = fixture().await;
        grant(&f, true).await;
        let mut record = f
            .store
            .read_sandbox_hierarchy(&f.parent.id)
            .await
            .unwrap()
            .unwrap();
        match fault {
            "target" => record.sandbox.id = SandboxId("substituted".into()),
            "workspace_duplicate" => record.workspaces.push(record.workspaces[0].clone()),
            "tree_duplicate" => record.worktrees.push(record.worktrees[0].clone()),
            "agent_duplicate" => record.agents.push(record.agents[0].clone()),
            "orphan_assignment" => record.agents.clear(),
            _ => {
                let extra = workspace(&f.parent, "wks_other", None);
                record.agents[0].workspace_id = extra.id.clone();
                record.workspaces.push(extra);
            }
        }
        let reads = HierarchyReadService::new(Arc::new(BufferedStore {
            inner: f.store.clone(),
            sandbox: Some(record),
            dependency_failure: false,
            gate: None,
        }))
        .with_resource_access(f.access);
        assert!(
            matches!(
                reads
                    .sandbox_authorized(
                        &context("recipient"),
                        Some("request-credential"),
                        &f.parent.id
                    )
                    .await,
                Err(HierarchyReadError::Corrupt(_))
            ),
            "{fault}"
        );
    }
}

#[tokio::test]
async fn standalone_worktree_never_exposes_an_unverified_assigned_agent() {
    let f = fixture().await;
    grant(&f, true).await;
    let mut tree = f.tree.clone();
    tree.agent_id = Some(AgentId("private-foreign-agent".into()));
    tree.revision += 1;
    f.store.save_worktree(f.tree.revision, tree).await.unwrap();
    assert!(matches!(
        f.reads
            .worktree_authorized(
                &context("recipient"),
                Some("request-credential"),
                &f.tree.id
            )
            .await,
        Err(HierarchyReadError::Access(ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt
        )))
    ));
}
