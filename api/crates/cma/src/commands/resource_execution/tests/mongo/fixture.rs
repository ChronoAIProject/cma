use super::super::*;
use crate::store::MongoSandboxStore;
use layer2_core::resource_access::*;
use layer2_core::sandbox::store::{WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::*;
use layer2_core::{profiles::*, Authorization, Caller, Capability, SubjectId, WorkspaceProfileId};
use mongodb::{bson::doc, Client};
use std::time::{Duration, SystemTime};

pub(super) fn context(subject: &str) -> CallerContext {
    let subject = SubjectId::new(subject);
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "synthetic".into(),
            subject: subject.clone(),
            credential_id: "synthetic".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: Vec::new(),
    }
}

struct Organizations;
#[async_trait::async_trait]
impl OrganizationAuthority for Organizations {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            OrganizationRole::Admin,
            SystemTime::now(),
            SystemTime::now() + Duration::from_secs(60),
        )?);
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(Vec::new())
    }
}

pub(super) struct Fixture {
    client: Client,
    name: String,
    pub sandbox: MongoSandboxStore,
    pub store: Arc<dyn ResourceExecutionStore>,
    pub coordinator: ResourceExecutionCoordinator,
    pub access: ResourceAccessService,
    pub target: ResourceTarget,
    pub selector: ResourceSelector,
    pub share: ResourceShare,
}

impl Fixture {
    pub fn database(&self) -> mongodb::Database {
        self.client.database(&self.name)
    }

    pub async fn auth_store(
        &self,
        key: &crate::store::AuthEncryptionKey,
    ) -> Arc<crate::store::MongoAuthStore> {
        Arc::new(
            crate::store::MongoAuthStore::open(&self.client, &self.name, "revocation-test", key)
                .await
                .unwrap(),
        )
    }

    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("Mongo replica-set URI required");
        let name = format!("cma_revocation_worker_{}", uuid::Uuid::new_v4().simple());
        let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
        options.app_name = Some(name.clone());
        let client = Client::with_options(options).unwrap();
        let sandbox = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (parent, agent) = crate::store::sandbox_maintenance::mongo_integration_tests::fixture(
            &sandbox,
            "worker-contract",
        )
        .await;
        hierarchy(&sandbox, &parent, &agent).await;
        let target = ResourceTarget::Agent { agent_id: agent.id };
        let selector = ResourceSelector::Sandbox {
            sandbox_id: parent.id,
        };
        let snapshot = sandbox.resource_access_snapshot(&target).await.unwrap();
        let share = sandbox
            .mutate_share(ShareMutation {
                selector: selector.clone(),
                owner: ResourceOwner::Person {
                    subject: SubjectId::new("owner"),
                },
                actor: context("owner").actor,
                key_sha256: digest('a').as_str().into(),
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("synthetic-org"),
                    mode: ShareMode::OrgRoles,
                },
                expected_share_revision: None,
                policy_fence: snapshot.fence(),
                proposed_id: "worker-share".into(),
                at: parent.created_at,
            })
            .await
            .unwrap()
            .share;
        // Activation is confined to this disposable contract database.
        sandbox.initialize_resource_execution_once().await.unwrap();
        sandbox
            .activate_resource_execution_after_drain(generation(1), &digest('d'))
            .await
            .unwrap();
        let store = sandbox.open_resource_execution().await.unwrap();
        let policy = ResourceExecutionPolicy::enforce(store.clone(), true)
            .await
            .unwrap();
        let access = ResourceAccessService::new(Arc::new(sandbox.clone()), Arc::new(Organizations))
            .with_execution_policy(policy.clone());
        let coordinator = ResourceExecutionCoordinator::new(access.clone(), policy);
        Self {
            client,
            name,
            sandbox,
            store,
            coordinator,
            access,
            target,
            selector,
            share,
        }
    }

    pub async fn authority(
        &self,
        subject: &str,
        capability: ResourceCapability,
    ) -> ExecutionAuthority {
        self.coordinator
            .authorize_execution(
                &context(subject),
                Some("synthetic"),
                &self.target,
                capability,
                None,
            )
            .await
            .unwrap()
    }

    pub fn execution(&self, suffix: &str) -> wire::ExecutionKey {
        let ResourceTarget::Agent { agent_id } = &self.target else {
            panic!()
        };
        wire::ExecutionKey {
            command_id: id(suffix),
            command_fingerprint: digest('a'),
            dispatch_sequence: generation(1),
            prompt_id: id("prompt"),
            turn_id: id("turn"),
            agent_id: id(agent_id.as_str()),
            receiver: wire::ReceiverKey {
                kind: wire::ReceiverKind::Runtime,
                instance: id("runtime"),
                incarnation: digest('b'),
                boot: id("boot"),
            },
            attempt: generation(1),
        }
    }

    pub async fn register(&self, suffix: &str) -> ExecutionRecordV1 {
        self.coordinator
            .register_execution(
                &self
                    .authority("recipient", ResourceCapability::Interact)
                    .await,
                &self.execution(suffix),
            )
            .await
            .unwrap()
    }

    pub async fn release(&self, suffix: &str) -> ExecutionRecordV1 {
        self.coordinator
            .register_release(
                &self
                    .authority("recipient", ResourceCapability::ViewHistory)
                    .await,
                &wire::ReleaseKey {
                    response_id: id(suffix),
                    content_fingerprint: digest('c'),
                    receiver: wire::ReceiverKey {
                        kind: wire::ReceiverKind::ReleaseProducer,
                        instance: id("api"),
                        incarnation: digest('d'),
                        boot: id("api-boot"),
                    },
                },
            )
            .await
            .unwrap()
    }

    pub fn command(&self) -> ShareCommand {
        ShareCommand {
            selector: self.selector.clone(),
            change: ShareChange::Revoke {
                share_id: self.share.id.clone(),
            },
            expected_share_revision: Some(self.share.revision),
            idempotency_key: "worker-freeze".into(),
        }
    }

    pub async fn freeze(&self) -> RevocationOperationV1 {
        self.coordinator
            .begin_revocation(&context("owner"), None, self.command())
            .await
            .unwrap()
    }

    pub async fn reopened(&self) -> Arc<dyn ResourceExecutionStore> {
        MongoSandboxStore::open(&self.client, &self.name)
            .await
            .unwrap()
            .open_resource_execution()
            .await
            .unwrap()
    }

    pub async fn ambiguous_commits(&self, enabled: bool) {
        let command = if enabled {
            doc! {
                "configureFailPoint":"failCommand", "mode":{"times":5_i32},
                "data":{"failCommands":["commitTransaction"], "appName":&self.name,
                    "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"synthetic ambiguous worker commit"}}
            }
        } else {
            doc! {"configureFailPoint":"failCommand", "mode":"off"}
        };
        self.client
            .database("admin")
            .run_command(command)
            .await
            .unwrap();
        if !enabled {
            crate::store::test_visibility::majority_barrier(&self.client, &self.name).await;
        }
    }

    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

async fn hierarchy(store: &MongoSandboxStore, parent: &Sandbox, agent: &Agent) {
    let at = parent.created_at.clone();
    store
        .create_workspace(Workspace {
            id: agent.workspace_id.clone(),
            sandbox_id: parent.id.clone(),
            owner: parent.owner.clone(),
            origin: WorkspaceOrigin::Manual,
            profile: WorkspaceProfileSnapshot {
                id: WorkspaceProfileId("wkp_worker".into()),
                revision: 1,
                digest: "d".repeat(64),
                content: WorkspaceProfileContent {
                    location: WorkspaceLocation::Directory {
                        name: "project".into(),
                    },
                    name: "project".into(),
                    setup_commands: vec![],
                    agent_profiles: vec![],
                },
            },
            name: "project".into(),
            path: "/home/cma/project".into(),
            source: WorkspaceSource::Empty,
            resolved_commit: Some("a".repeat(40)),
            status: WorkspaceStatus::Ready,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::None, at.clone()),
            next_worktree_number: 1,
            created_at: at.clone(),
            ready_at: Some(at),
            failed: None,
            removed_at: None,
            revision: 1,
        })
        .await
        .unwrap();
    store
        .create_worktree(Worktree {
            id: agent.worktree_id.clone(),
            workspace_id: agent.workspace_id.clone(),
            sandbox_id: parent.id.clone(),
            owner: parent.owner.clone(),
            name: "worker".into(),
            path: "/home/cma/project/.worktrees/worker".into(),
            branch: "cma/worker".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
}
