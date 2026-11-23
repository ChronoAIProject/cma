//! Real consent/enrollment paths retain typed known-abort evidence, not new authority.
use super::*;
use crate::store::agent_access_requests::{AgentAccessPhase, AgentAccessRecord};
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::store::AgentStore;
use layer2_core::ServiceId;
#[path = "publication_retry_tests/fixture.rs"]
mod fixture;
#[path = "publication_retry_tests/maintenance.rs"]
mod maintenance;
#[path = "publication_retry_tests/owner_scope.rs"]
mod owner_scope;
use fixture::{assert_starts, Fixture};

fn proposal() -> AgentAccessProposal {
    AgentAccessProposal {
        request_key: "retry-proof".into(),
        retry_of: None,
        services: vec!["github".into()],
        reason: "Read the approved repository".into(),
    }
}

async fn consent_lineage_abort_retries_exact_owner_save_and_proposal_once_case(owner_save: bool) {
    let f = Fixture::new(true).await;
    f.abort("aggregate", Some("agents"), 1);
    let result = if owner_save {
        f.owner_save(f.original_services(), "owner-key").await
    } else {
        f.store
            .request_agent_access(f.requester(), proposal())
            .await
    };
    let (fired, commands) = f.clear().await;
    let count = f
        .independent
        .agent_access_requests
        .count_documents(doc! {})
        .await
        .unwrap();
    let replay = if owner_save {
        f.owner_save(f.original_services(), "owner-key").await
    } else {
        f.store
            .request_agent_access(f.requester(), proposal())
            .await
    };
    let mut changed = proposal();
    changed.reason = "Different request".into();
    let mismatch = if owner_save {
        f.owner_save(vec![ServiceId::new("new-scope")], "owner-key")
            .await
    } else {
        f.store.request_agent_access(f.requester(), changed).await
    };
    f.cleanup().await;
    assert!(fired);
    let record = result.expect("known-aborted consent lineage read must retry");
    assert_starts(&commands, 2);
    assert_eq!(count, 1);
    assert_eq!(replay.unwrap(), record);
    assert_eq!(mismatch, Err(AgentAccessError::Conflict));
    assert_eq!(record.revision, 1);
    assert_eq!(
        record.phase,
        if owner_save {
            AgentAccessPhase::Connecting
        } else {
            AgentAccessPhase::Pending
        }
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_start_fence_abort_retries_with_original_work_claim() {
    let f = Fixture::new(true).await;
    f.abort("update", Some("sandboxes"), 1);
    let result = f
        .with_work(f.owner_save(f.original_services(), "start-fence"))
        .await;
    let (fired, commands) = f.clear().await;
    let count = f
        .independent
        .agent_access_requests
        .count_documents(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    result.expect("known-aborted start fence must enter the consent retry loop");
    assert_starts(&commands, 2);
    assert_eq!(count, 1);
}

async fn consent_abort_bound_and_stale_scope_never_publish_a_new_authority_case(persistent: bool) {
    let f = Fixture::new(true).await;
    f.abort("aggregate", Some("agents"), if persistent { 5 } else { 1 });
    let result = f
        .owner_save(
            if persistent {
                f.original_services()
            } else {
                vec![ServiceId::new("stale")]
            },
            "guard",
        )
        .await;
    let (fired, commands) = f.clear().await;
    let count = f
        .independent
        .agent_access_requests
        .count_documents(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    assert_eq!(
        result,
        Err(if persistent {
            AgentAccessError::Unavailable
        } else {
            AgentAccessError::Conflict
        })
    );
    assert_starts(&commands, if persistent { 5 } else { 2 });
    assert_eq!(count, 0);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_unknown_commit_returns_unavailable_without_mutation_replay() {
    let f = Fixture::new(true).await;
    f.arm("commitTransaction", None, 5, doc! {"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"uncertain consent commit"}});
    let result = f
        .owner_save(f.original_services(), "unknown-owner-key")
        .await;
    let (fired, commands) = f.clear().await;
    let mut read_session = f.committed_read_session(&commands).await;
    let record = f
        .independent
        .agent_access_requests
        .find_one(doc! {})
        .max_time(std::time::Duration::from_secs(5))
        .session(&mut read_session)
        .await
        .unwrap();
    let count = f
        .independent
        .agent_access_requests
        .count_documents(doc! {})
        .max_time(std::time::Duration::from_secs(5))
        .session(&mut read_session)
        .await
        .unwrap();
    let replay = f
        .owner_save(f.original_services(), "unknown-owner-key")
        .await;
    f.cleanup().await;
    assert!(fired);
    assert_eq!(result, Err(AgentAccessError::Unavailable));
    assert_starts(&commands, 1);
    assert_eq!(count, 1);
    let replay = replay.unwrap();
    assert_eq!(replay.revision, 1);
    assert_eq!(record.unwrap().get_str("_id").unwrap(), replay.id.as_str());
}

async fn prepared_tool_receipt_abort_retries_original_resolve_and_confirm_case(confirm: bool) {
    let f = Fixture::new(false).await;
    let before = f
        .independent
        .get_agent(&f.state.creation.as_ref().unwrap().agent_id)
        .await
        .unwrap();
    let original = if confirm {
        Some(
            f.source
                .resolve(&f.runtime, &f.open, f.open_fence())
                .await
                .unwrap()
                .unwrap(),
        )
    } else {
        None
    };
    f.abort("find", Some("native_prepared_sandboxes"), 1);
    let result = if let Some(binding) = &original {
        f.source
            .confirm_open(binding, "prepared-session")
            .await
            .map(|()| binding.clone())
    } else {
        f.source
            .resolve(&f.runtime, &f.open, f.open_fence())
            .await
            .map(Option::unwrap)
    };
    let (fired, commands) = f.clear().await;
    let count = f
        .independent
        .agent_access_tools
        .count_documents(doc! {})
        .await
        .unwrap();
    let after = f
        .independent
        .get_agent(&f.state.creation.as_ref().unwrap().agent_id)
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    let binding =
        result.expect("prepared receipt Driver abort must not become authority rejection");
    assert_starts(&commands, 2);
    assert_eq!(count, 1);
    assert_eq!(before, after);
    assert_eq!(binding.scope().0, &f.runtime);
    if let Some(original) = original {
        assert_eq!(binding, original);
    }
}

async fn tool_start_abort_bound_and_prepared_identity_refusal_preserve_enrollment_case(
    failures: i32,
) {
    let f = Fixture::new(false).await;
    f.abort("update", Some("sandboxes"), failures);
    let result = f
        .with_work(f.source.resolve(&f.runtime, &f.open, f.open_fence()))
        .await;
    let (fired, commands) = f.clear().await;
    let count = f
        .independent
        .agent_access_tools
        .count_documents(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    assert_starts(&commands, if failures == 1 { 2 } else { 5 });
    if failures == 1 {
        assert!(result.unwrap().is_some());
        assert_eq!(count, 1);
    } else {
        assert!(matches!(result, Err(AdapterError::Unavailable(_))));
        assert_eq!(count, 0);
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn prepared_tool_wrong_conversation_refuses_after_abort_without_changing_receipt() {
    let f = Fixture::new(false).await;
    let binding = f
        .source
        .resolve(&f.runtime, &f.open, f.open_fence())
        .await
        .unwrap()
        .unwrap();
    let before = f
        .independent
        .agent_access_tools
        .find_one(doc! {})
        .await
        .unwrap();
    f.abort("find", Some("native_prepared_sandboxes"), 1);
    let result = f.source.confirm_open(&binding, "different-session").await;
    let (fired, commands) = f.clear().await;
    let after = f
        .independent
        .agent_access_tools
        .find_one(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    assert_starts(&commands, 2);
    assert!(matches!(result, Err(AdapterError::Rejected(_))));
    assert_eq!(before, after);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn tool_unknown_commit_returns_without_second_enrollment_transaction() {
    let f = Fixture::new(false).await;
    f.arm("commitTransaction", None, 5, doc! {"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"uncertain tool commit"}});
    let result = f.source.resolve(&f.runtime, &f.open, f.open_fence()).await;
    let (fired, commands) = f.clear().await;
    let mut read_session = f.committed_read_session(&commands).await;
    let before = f
        .independent
        .agent_access_tools
        .find_one(doc! {})
        .max_time(std::time::Duration::from_secs(5))
        .session(&mut read_session)
        .await
        .unwrap();
    let replay = f.source.resolve(&f.runtime, &f.open, f.open_fence()).await;
    let after = f
        .independent
        .agent_access_tools
        .find_one(doc! {})
        .max_time(std::time::Duration::from_secs(5))
        .session(&mut read_session)
        .await
        .unwrap();
    let count = f
        .independent
        .agent_access_tools
        .count_documents(doc! {})
        .max_time(std::time::Duration::from_secs(5))
        .session(&mut read_session)
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    assert!(matches!(result, Err(AdapterError::Unavailable(_))));
    assert_starts(&commands, 1);
    assert!(replay.unwrap().is_some());
    assert_eq!(count, 1);
    assert_eq!(before, after);
}

async fn uncertain_duplicate_error_never_authorizes_consent_or_prepared_tool_retry_case(
    consent: bool,
) {
    let f = Fixture::new(consent).await;
    f.arm(if consent {"aggregate"} else {"find"},
            Some(if consent {"agents"} else {"native_prepared_sandboxes"}), 1,
            doc! {"errorCode":11000_i32,"errorLabels":["TransientTransactionError","UnknownTransactionCommitResult"]});
    let unavailable = if consent {
        matches!(
            f.owner_save(f.original_services(), "uncertain-label").await,
            Err(AgentAccessError::Unavailable)
        )
    } else {
        matches!(
            f.source.resolve(&f.runtime, &f.open, f.open_fence()).await,
            Err(AdapterError::Unavailable(_))
        )
    };
    let (fired, commands) = f.clear().await;
    let requests = f
        .independent
        .agent_access_requests
        .count_documents(doc! {})
        .await
        .unwrap();
    let tools = f
        .independent
        .agent_access_tools
        .count_documents(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    assert!(
        unavailable,
        "uncertainty must remain unavailable even with duplicate/transient evidence"
    );
    assert_starts(&commands, 1);
    assert_eq!((requests, tools), (0, 0));
}

async fn consent_publication_retry_preserves_original_body_and_timestamp_case(owner_save: bool) {
    let f = Fixture::new(true).await;
    // The first insert is held across a wall-clock second before aborting.
    // A retry must not regenerate the approved/requested timestamp.
    f.arm(
        "insert",
        Some("agent_access_requests"),
        1,
        doc! {
            "errorCode":112_i32, "errorLabels":["TransientTransactionError"],
            "blockConnection":true, "blockTimeMS":1200_i32,
        },
    );
    let result = if owner_save {
        f.owner_save(f.original_services(), "stable-payload").await
    } else {
        f.store
            .request_agent_access(f.requester(), proposal())
            .await
    };
    let (fired, commands) = f.clear().await;
    let stored = f
        .independent
        .agent_access_requests
        .find_one(doc! {})
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    let result = result.expect("aborted request insert must retry its original immutable payload");
    assert_starts(&commands, 2);
    let attempts: Vec<_> = commands
        .iter()
        .filter(|command| command.get_str("insert").ok() == Some("agent_access_requests"))
        .map(|command| command.get_array("documents").unwrap().clone())
        .collect();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0], attempts[1]);
    assert_eq!(
        crate::store::agent_access_requests::read(&stored.unwrap()).unwrap(),
        result
    );
}

#[path = "publication_retry_tests_cases.rs"]
mod cases;
