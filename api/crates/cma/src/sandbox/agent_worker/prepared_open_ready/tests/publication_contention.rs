//! Real Mongo failures after Ready must not strand the new connection authority.
use super::*;
use layer2_core::sandbox::store::AgentWorkerStore;
use mongodb::bson::doc;
use std::sync::atomic::Ordering;
use std::time::Duration;

async fn claimed(fixture: &Fixture) -> (Agent, LiveAgentAttachment, SystemTime) {
    assert!(fixture.publish().await.unwrap());
    let mut ready = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let expected = ready.revision;
    let now = SystemTime::now();
    ready.conversation.first_message = Some(
        FirstMessageRecord::new(
            &ready.creation_key,
            "preserve this exact input",
            rfc3339_at(now),
        )
        .unwrap(),
    );
    ready.revision += 1;
    ready.updated_at = rfc3339_at(now);
    fixture.store.save_agent(expected, ready).await.unwrap();
    let agent = fixture
        .store
        .claim_agent_work(
            &fixture.agent.id,
            "open-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(18 * 60)),
        )
        .await
        .unwrap()
        .unwrap();
    let target = LiveAgentTarget {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: 1,
    };
    let attachment = fixture
        .worker
        .dependencies
        .registry
        .staged_attachment(&target, agent.work.lease.as_ref().unwrap().fence)
        .unwrap()
        .unwrap();
    fixture
        .worker
        .dependencies
        .registry
        .publish_and_activate(&attachment)
        .unwrap();
    (agent, attachment, now)
}

async fn admin() -> mongodb::Client {
    mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap()
}

async fn disable(admin: &mongodb::Client) {
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "off",
        })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at disposable replica set"]
async fn prepared_connection_contention_releases_only_new_lease_and_preserves_input() {
    for failures in [1_i32, 40] {
        let app = format!("cma-prepared-publication-{}", uuid::Uuid::new_v4().simple());
        let fixture = Fixture::with_app(Some(app.clone())).await;
        let (agent, attachment, now) = claimed(&fixture).await;
        let admin = admin().await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": failures},
                "data": {"failCommands": ["update"], "appName": app,
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
            })
            .await
            .unwrap();
        let result = fixture
            .worker
            .finish_connection_publication(agent.clone(), &attachment, None, now)
            .await;
        disable(&admin).await;
        result.unwrap();
        let after = fixture.store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(
            after.conversation.first_message,
            agent.conversation.first_message
        );
        assert_eq!(
            after.conversation.conversation_id,
            agent.conversation.conversation_id
        );
        assert_eq!(
            after.conversation.runtime_generation,
            agent.conversation.runtime_generation
        );
        assert_eq!(after.work.receipts, agent.work.receipts);
        assert!(after.work.lease.is_none());
        assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
        if failures == 40 {
            assert_eq!(after.work.step, AgentWorkStep::PublishConnection);
            assert_eq!(after.work.attempts, 1);
            let retry = SystemTime::UNIX_EPOCH
                + Duration::from_secs(
                    layer2_core::seconds_from_rfc3339(&after.work.ready_at).unwrap(),
                );
            assert!(retry <= SystemTime::now() + Duration::from_secs(1));
        } else {
            assert_eq!(after.work.step, AgentWorkStep::DispatchFirstMessage);
        }
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at disposable replica set"]
async fn prepared_connection_unknown_commit_never_releases_as_aborted_contention() {
    let app = format!("cma-prepared-unknown-{}", uuid::Uuid::new_v4().simple());
    let fixture = Fixture::with_app(Some(app.clone())).await;
    let (agent, attachment, now) = claimed(&fixture).await;
    let admin = admin().await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 10_i32},
            "data": {"failCommands": ["commitTransaction"], "appName": app,
                "errorCode": 50_i32, "errorLabels": ["UnknownTransactionCommitResult"]},
        })
        .await
        .unwrap();
    let result = fixture
        .worker
        .finish_connection_publication(agent.clone(), &attachment, None, now)
        .await;
    disable(&admin).await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    let after = fixture.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(after, agent);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_stale_publication_cannot_release_successor_claim() {
    let fixture = Fixture::new().await;
    let (original, attachment, now) = claimed(&fixture).await;
    let later = now + Duration::from_secs(19 * 60);
    let successor = fixture
        .store
        .claim_agent_work(
            &original.id,
            "successor",
            rfc3339_at(later),
            rfc3339_at(later + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let result = fixture
        .worker
        .finish_connection_publication(original, &attachment, None, later)
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        fixture.store.get_agent(&successor.id).await.unwrap(),
        Some(successor)
    );
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_retry_guard_refuses_settings_or_input_mutation() {
    use crate::sandbox::agent_worker::connection_retry::same_connection_claim;
    let fixture = Fixture::new().await;
    let (agent, _, now) = claimed(&fixture).await;
    assert!(same_connection_claim(&agent, &agent, "open-worker", now));
    for case in 0..5 {
        let mut changed = agent.clone();
        changed.revision += 1;
        match case {
            0 => changed.conversation.settings = Some(changed.settings()),
            1 => {
                changed.conversation.first_message.as_mut().unwrap().text = "different input".into()
            }
            2 => changed.conversation.first_message.as_mut().unwrap().attempt += 1,
            3 => changed.work.lease.as_mut().unwrap().fence += 1,
            _ => changed.conversation.runtime_generation = Some(2),
        }
        assert!(
            !same_connection_claim(&agent, &changed, "open-worker", now),
            "case {case}"
        );
    }
    fixture.cleanup().await;
}
