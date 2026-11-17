//! Two independent authenticated Mongo clients and a delayed loopback receiver.
use super::*;
use crate::replicas::mongo_fixture;

async fn notified(signal: &tokio::sync::Notify) {
    tokio::time::timeout(Duration::from_secs(5), signal.notified())
        .await
        .expect("fixture barrier deadline");
}

#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn native_unknown_set_and_delete_keep_custody_and_bot_claim_after_expiry() {
    for delete in [false, true] {
        let mongo = mongo_fixture::Fixture::new().await;
        let app = Native::with_fixture(Fixture::native(&mongo).await).await;
        let (definition, channel_id) = app.reserve_child().await;
        if delete {
            binding::activate(&app.service.management, &definition, &channel_id)
                .await
                .unwrap();
            app.peer.pause_delete.store(true, Ordering::SeqCst);
        } else {
            app.peer.pause_webhook.store(true, Ordering::SeqCst);
        }
        // Remove the successful activation's notification before awaiting delete.
        let _ = app.peer.webhook_finished.notified().now_or_never();
        let state = app.service.management.clone();
        let task_definition = definition.clone();
        let task_id = channel_id.clone();
        let task = tokio::spawn(async move {
            if delete {
                binding::disconnect(&state, &task_definition, &task_id).await
            } else {
                binding::activate(&state, &task_definition, &task_id).await
            }
        });
        notified(&app.peer.webhook_started).await;
        // Exercise an actual client timeout while the accepted remote task lives.
        assert!(task.await.unwrap().is_err());
        app.expire_lease().await;
        let nonce = app.stored().await.0.webhook_lease.unwrap().nonce;
        app.fixture
            .forget_channel(definition.record_id.as_str())
            .await;
        let mut other = app.fixture.independent_service(&mongo).await;
        other.management.telegram = app.service.management.telegram.clone();
        other
            .cancel(&owner("alice", false), &app.record.id)
            .await
            .unwrap();
        let held = app.stored().await.0;
        assert_eq!(held.status, Status::Cancelled);
        assert_eq!(held.webhook_lease.as_ref().unwrap().nonce, nonce);
        assert_eq!(held.channel_id.as_deref(), Some(channel_id.as_str()));
        assert!(held.channel_record.is_some());
        assert!(held.webhook_retiring);
        assert!(matches!(
            app.fixture
                .store()
                .claim_bot("88", "alice", &uuid::Uuid::new_v4().to_string())
                .await,
            Err(Error::Conflict)
        ));
        app.peer.webhook_resume.notify_one();
        notified(&app.peer.webhook_finished).await;
        assert!(matches!(
            other.reconcile_webhook_retirement(&app.record.id).await,
            Err(Error::Conflict)
        ));
        assert_eq!(app.stored().await.0.webhook_lease.unwrap().nonce, nonce);
        assert_eq!(
            app.methods()
                .iter()
                .filter(|method| method.as_str() == "setWebhook")
                .count(),
            1
        );
        assert_eq!(
            app.methods()
                .iter()
                .filter(|method| method.as_str() == "deleteWebhook")
                .count(),
            usize::from(delete)
        );
        drop(other);
        drop(app);
        mongo.cleanup().await;
    }
}

use futures_util::FutureExt;

#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn native_pre_dispatch_expiry_and_read_failure_allow_exact_safe_retry() {
    for delete in [false, true] {
        let mongo = mongo_fixture::Fixture::new().await;
        let app = Native::with_fixture(Fixture::native(&mongo).await).await;
        let (definition, channel_id) = app.reserve_child().await;
        if delete {
            binding::activate(&app.service.management, &definition, &channel_id)
                .await
                .unwrap();
        }
        app.peer.pause_read.store(true, Ordering::SeqCst);
        let state = app.service.management.clone();
        let task_definition = definition.clone();
        let task_id = channel_id.clone();
        let task = tokio::spawn(async move {
            if delete {
                binding::disconnect(&state, &task_definition, &task_id).await
            } else {
                binding::activate(&state, &task_definition, &task_id).await
            }
        });
        notified(&app.peer.read_started).await;
        app.expire_lease().await;
        app.peer.read_resume.notify_one();
        let result = task.await.unwrap();
        if !delete {
            assert!(result.is_err());
        }
        assert!(app.stored().await.0.webhook_lease.is_none());
        let mut other = app.fixture.independent_service(&mongo).await;
        other.management.telegram = app.service.management.telegram.clone();
        app.peer.fail_read.store(true, Ordering::SeqCst);
        let result = if delete {
            binding::disconnect(&other.management, &definition, &channel_id).await
        } else {
            binding::activate(&other.management, &definition, &channel_id).await
        };
        assert!(result.is_err());
        assert!(app.stored().await.0.webhook_lease.is_none());
        if delete {
            binding::disconnect(&other.management, &definition, &channel_id)
                .await
                .unwrap();
        } else {
            binding::activate(&other.management, &definition, &channel_id)
                .await
                .unwrap();
        }
        assert!(app.stored().await.0.webhook_lease.is_none());
        assert_eq!(
            app.methods()
                .iter()
                .filter(|method| method.as_str() == "setWebhook")
                .count(),
            1
        );
        assert_eq!(
            app.methods()
                .iter()
                .filter(|method| method.as_str() == "deleteWebhook")
                .count(),
            usize::from(delete)
        );
        drop(other);
        drop(app);
        mongo.cleanup().await;
    }
}
