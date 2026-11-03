use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use crate::resource_access::*;
use crate::sandbox::agent_io::*;
use tokio::sync::Notify;

use super::*;

#[path = "command_admission_result_tests.rs"]
mod admission_results;

#[path = "command_bootstrap_race_tests.rs"]
mod bootstrap_races;

#[path = "command_status_fence_tests.rs"]
mod status_fences;

#[path = "command_execution_tests.rs"]
mod execution;

#[path = "targeted_control_tests.rs"]
mod targeted_controls;

#[path = "queued_cancellation_tests.rs"]
mod queued_cancellations;

#[path = "command_model_tests.rs"]
mod models;

#[path = "suggested_reply_tests.rs"]
mod suggested_replies;

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: caller(subject, true),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "exact-request".into(),
        },
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        memberships: Vec::new(),
    }
}

struct Authority {
    answer: Mutex<Result<AgentAuthorizationRevision, CommandAuthorizationError>>,
    calls: AtomicUsize,
    membership: Mutex<Result<OrganizationRole, MembershipError>>,
    lifetime: Duration,
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
}
impl Authority {
    fn new() -> Self {
        Self {
            answer: Mutex::new(Ok(AgentAuthorizationRevision {
                authority_id: "exact-personal-custody".into(),
                policy_revision: 1,
                grant_revision: None,
            })),
            calls: AtomicUsize::new(0),
            membership: Mutex::new(Ok(OrganizationRole::Viewer)),
            lifetime: MAX_MEMBERSHIP_AGE,
            gate: None,
        }
    }
}
#[async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        assert_eq!(credential, "selected-credential");
        assert_eq!(context.caller.subject, SubjectId::new("owner"));
        assert_eq!(
            owner,
            &AgentOwner::Person {
                subject: SubjectId::new("owner")
            }
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((entered, release)) = &self.gate {
            entered.notify_one();
            release.notified().await;
        }
        self.answer.lock().unwrap().clone()
    }
}
#[async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "selected-credential");
        let role = self.membership.lock().unwrap().clone()?;
        let mut context = context(caller.subject.as_str());
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            org.clone(),
            role,
            now,
            now + self.lifetime,
        )?);
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("inventory cannot establish command authority")
    }
}

async fn setup(
    authority: Authority,
) -> (
    Arc<InMemorySandboxStore>,
    Arc<AgentCommandService>,
    Arc<Authority>,
    Agent,
) {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agent",
        "worktree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let authority = Arc::new(authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let service = AgentCommandService::new(store.clone(), access, authority.clone())
        .with_task_experience(Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode": "all", "accounts": [], "capabilities": ["suggested_followup_admission"]
            }))
            .unwrap(),
        ));
    (store, service, authority, agent)
}

fn submission(key: &str) -> CommandSubmission {
    CommandSubmission {
        agent_id: AgentId("agent".into()),
        command: AgentCommand::message("private input"),
        idempotency_key: key.into(),
        expected_agent_revision: None,
        requested_model: None,
    }
}

async fn share(store: &InMemorySandboxStore, previous: Option<ResourceShare>) -> ResourceShare {
    let target = ResourceTarget::Sandbox {
        sandbox_id: SandboxId("sandbox".into()),
    };
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let key = if previous.is_some() {
        "revoke"
    } else {
        "create"
    };
    let change = previous.as_ref().map_or(
        ShareChange::Create {
            target_org_id: SubjectId::new("org"),
            mode: ShareMode::View,
        },
        |share| ShareChange::Revoke {
            share_id: share.id.clone(),
        },
    );
    store
        .mutate_share(ShareMutation {
            selector: ResourceSelector::Sandbox {
                sandbox_id: SandboxId("sandbox".into()),
            },
            owner: ResourceOwner::Person {
                subject: SubjectId::new("owner"),
            },
            actor: ResourceActor::Human {
                subject: SubjectId::new("owner"),
            },
            key_sha256: command_key_digest(key).unwrap(),
            change,
            expected_share_revision: previous.as_ref().map(|share| share.revision),
            policy_fence: snapshot.fence(),
            proposed_id: "share".into(),
            at: crate::now_rfc3339(),
        })
        .await
        .unwrap()
        .share
}

#[tokio::test]
async fn admission_assigns_attribution_and_retries_preserve_receipt_after_agent_revision_changes() {
    let (store, service, authority, agent) = setup(Authority::new()).await;
    let before_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
    let owner = context("owner");
    let mut retry = submission("one");
    retry.command = AgentCommand::Message {
        input: AgentInput {
            content: vec![
                AgentContent::Text {
                    text: "private ".into(),
                },
                AgentContent::Text {
                    text: "input".into(),
                },
            ],
        },
        order: AgentInputOrder::Append,
        metadata: Default::default(),
    };
    let (a, b) = tokio::join!(
        service.admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("one")
        ),
        service.admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::UserInterface,
            retry
        ),
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.status, AgentResponseStatus::Queued);
    let record = store
        .get_command(&a.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.receipt.envelope.actor, owner.actor);
    assert_eq!(
        record.receipt.envelope.owner,
        AgentOwner::Person {
            subject: owner.caller.subject.clone()
        }
    );
    assert_eq!(
        record.receipt.envelope.source.reference,
        record.receipt.envelope.command_id.to_string()
    );
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("selected-credential"));
    let mut changed = agent.clone();
    changed.revision += 1;
    changed.title = Some("updated without a new command".into());
    store.save_agent(agent.revision, changed).await.unwrap();
    let mut retry = submission("one");
    retry.expected_agent_revision = Some(agent.revision);
    assert_eq!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                retry
            )
            .await
            .unwrap(),
        a
    );
    let mut stale_new = submission("two");
    stale_new.expected_agent_revision = Some(agent.revision);
    assert!(matches!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                stale_new
            )
            .await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    let mut conflict = submission("one");
    conflict.command = AgentCommand::message("different input");
    assert!(matches!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                conflict
            )
            .await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::IdempotencyConflict
        ))
    ));
    assert_eq!(
        service
            .response_status(&owner, None, a.receipt.response_id.as_ref().unwrap())
            .await
            .unwrap(),
        a
    );
    assert_eq!(
        service
            .command_status(&owner, None, &a.receipt.command_id)
            .await
            .unwrap(),
        a
    );
    assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap(), before_log);
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(authority.calls.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn forged_actor_unrelated_caller_and_missing_custody_cannot_admit_work() {
    let (store, service, authority, _) = setup(Authority::new()).await;
    let mut forged = context("owner");
    forged.actor = context("other").actor;
    for caller in [context("other"), forged] {
        assert!(service
            .admit(
                &caller,
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("one")
            )
            .await
            .is_err());
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), 0);
    for error in [
        CommandAuthorizationError::Revoked,
        CommandAuthorizationError::Unavailable,
    ] {
        *authority.answer.lock().unwrap() = Err(error.clone());
        assert!(
            matches!(service.admit(&context("owner"), "selected-credential", CommandSubmissionSource::Api, submission("one")).await, Err(CommandAdmissionError::Authority(actual)) if actual == error)
        );
    }
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn model_mismatch_and_unsupported_attachment_do_not_create_a_receipt() {
    let (store, service, _, _) = setup(Authority::new()).await;
    let mut input = submission("model");
    input.requested_model = Some("not-the-pinned-model".into());
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                input
            )
            .await,
        Err(CommandAdmissionError::ModelMismatch)
    ));
    let mut input = submission("file");
    if let AgentCommand::Message { input, .. } = &mut input.command {
        input.content.push(AgentContent::Attachment {
            reference: AgentAttachmentReference {
                id: "file".into(),
                sha256: "a".repeat(64),
                media_type: "text/plain".into(),
                size_bytes: 10,
            },
        });
    }
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                input
            )
            .await,
        Err(CommandAdmissionError::Input(
            AgentOperationError::ContentUnsupported
        ))
    ));
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn policy_change_during_credential_io_fences_both_success_and_authority_error() {
    for failed in [false, true] {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let mut authority = Authority::new();
        authority.gate = Some((entered.clone(), release.clone()));
        if failed {
            *authority.answer.lock().unwrap() = Err(CommandAuthorizationError::Revoked);
        }
        let (store, service, _, agent) = setup(authority).await;
        let work = tokio::spawn(async move {
            service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    submission("one"),
                )
                .await
        });
        entered.notified().await;
        share(&store, None).await;
        release.notify_one();
        assert!(matches!(
            work.await.unwrap(),
            Err(CommandAdmissionError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            )))
        ));
        assert!(store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    }
}

#[tokio::test]
async fn status_uses_current_resource_authority_and_does_not_require_old_execution_custody() {
    let (store, service, authority, _) = setup(Authority::new()).await;
    let receipt = service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("one"),
        )
        .await
        .unwrap();
    let grant = share(&store, None).await;
    *authority.answer.lock().unwrap() = Err(CommandAuthorizationError::Revoked);
    let reader = context("reader");
    assert_eq!(
        service
            .response_status(
                &reader,
                Some("selected-credential"),
                receipt.receipt.response_id.as_ref().unwrap()
            )
            .await
            .unwrap(),
        receipt
    );
    assert!(!format!("{receipt:?}").contains("private input"));
    assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
    *authority.membership.lock().unwrap() = Err(MembershipError::NotMember);
    assert!(service
        .command_status(
            &reader,
            Some("selected-credential"),
            &receipt.receipt.command_id
        )
        .await
        .is_err());
    *authority.membership.lock().unwrap() = Ok(OrganizationRole::Viewer);
    share(&store, Some(grant)).await;
    assert!(service
        .command_status(
            &reader,
            Some("selected-credential"),
            &receipt.receipt.command_id
        )
        .await
        .is_err());
    assert_eq!(
        service
            .command_status(&context("owner"), None, &receipt.receipt.command_id)
            .await
            .unwrap(),
        receipt
    );
}
