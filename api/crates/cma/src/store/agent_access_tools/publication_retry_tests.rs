//! Real consent/enrollment paths retain typed known-abort evidence, not new authority.
use super::*;
use crate::store::agent_access_requests::{AgentAccessPhase, AgentAccessRecord};
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::ServiceId;
#[path = "publication_retry_tests/fixture.rs"]
mod fixture;
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
