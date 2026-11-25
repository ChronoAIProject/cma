use super::*;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
mod connections;
mod disabled_rotation;
mod effect_custody;
mod effect_store_failures;
mod fixture;
use fixture::Fixture;

#[tokio::test]
async fn scheduled_renewal_preserves_disabled_state_and_does_not_rotate_fresh_keys() {
    let f = Fixture::new().await;
    f.worker.run_due(&f.bot_id).await.unwrap();
    let initial = f.bot().await;
    f.worker.run_due(&f.bot_id).await.unwrap();
    assert_eq!(f.bot().await, initial);
    f.admit(BotOperation::Disable).await;
    f.worker.run_due(&f.bot_id).await.unwrap();
    f.renewal_due().await;
    f.worker.run_due(&f.bot_id).await.unwrap();
    let renewed = f.bot().await;
    assert_eq!(renewed.state, BotState::Disabled);
    assert_ne!(renewed.resources.credentials, initial.resources.credentials);
    assert_eq!(
        renewed.resources.channel.as_ref().unwrap().channel_id,
        initial.resources.channel.as_ref().unwrap().channel_id
    );
    assert_eq!(f.external.0.lock().unwrap().keys.len(), 3);
    f.worker.run_due(&f.bot_id).await.unwrap();
    assert_eq!(f.bot().await, renewed);
}

#[tokio::test]
async fn create_policy_rotation_and_delete_follow_external_resource_order() {
    let f = Fixture::new().await;
    f.worker.run(&f.bot_id).await.unwrap();
    let active = f.bot().await;
    assert_eq!(active.state, BotState::Active);
    assert!(active.work.is_none());
    assert_eq!(
        f.external.0.lock().unwrap().calls,
        [
            "verify",
            "mint_agent",
            "mint_route",
            "create_channel",
            "set_callback",
            "create_route",
            "configure_enabled"
        ]
    );
    f.admit(BotOperation::Disable).await;
    f.worker.run(&f.bot_id).await.unwrap();
    assert_eq!(f.bot().await.state, BotState::Disabled);
    f.admit(BotOperation::Rotate).await;
    f.worker.run(&f.bot_id).await.unwrap();
    let rotated = f.bot().await;
    assert_eq!(rotated.state, BotState::Disabled);
    assert_ne!(rotated.resources.credentials, active.resources.credentials);
    assert_eq!(f.external.0.lock().unwrap().keys.len(), 3);
    f.admit(BotOperation::Enable).await;
    f.worker.run(&f.bot_id).await.unwrap();
    assert_eq!(f.external.0.lock().unwrap().keys.len(), 2);
    f.admit(BotOperation::Delete).await;
    f.worker.run(&f.bot_id).await.unwrap();
    assert_eq!(f.bot().await.state, BotState::Deleted);
    let remote = f.external.0.lock().unwrap();
    assert!(remote.keys.is_empty() && remote.channel.is_none() && remote.route.is_none());
}

#[tokio::test]
async fn every_creation_boundary_recovers_unknown_effects_before_finishing_failure() {
    for step in [
        "verify",
        "mint_agent",
        "mint_route",
        "create_channel",
        "set_callback",
        "create_route",
        "configure_enabled",
    ] {
        for after in [false, true] {
            if step == "verify" && after {
                continue;
            }
            let f = Fixture::new().await;
            f.fail(step, after);
            assert_eq!(
                f.worker.run(&f.bot_id).await,
                Err(BotError::Unavailable),
                "{step} {after}"
            );
            let failed = f.bot().await;
            assert!(matches!(failed.state, BotState::Failed { .. }));
            assert_eq!(failed.work.unwrap().operation, BotOperation::Cleanup);
            f.ready().await;
            f.worker.run(&f.bot_id).await.unwrap();
            let failed = f.bot().await;
            assert!(failed.work.is_none());
            assert_eq!(failed.resources, BotResources::default());
            let remote = f.external.0.lock().unwrap();
            assert!(
                remote.keys.is_empty() && remote.channel.is_none() && remote.route.is_none(),
                "{step} {after}"
            );
        }
    }
}

#[tokio::test]
async fn cleanup_failure_keeps_work_and_replays_without_minting() {
    for step in [
        "channel",
        "delete_channel",
        "keys",
        "delete_routes",
        "revoke",
    ] {
        let f = Fixture::new().await;
        f.worker.run(&f.bot_id).await.unwrap();
        f.admit(BotOperation::Delete).await;
        f.fail(step, false);
        assert!(f.worker.run(&f.bot_id).await.is_err());
        assert_eq!(f.bot().await.work.unwrap().operation, BotOperation::Delete);
        f.ready().await;
        f.worker.run(&f.bot_id).await.unwrap();
        assert_eq!(f.bot().await.state, BotState::Deleted);
        assert_eq!(
            f.external
                .0
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|s| s.as_str() == "mint_agent")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn interrupted_creator_uses_reserved_channel_identity_and_never_remints() {
    let f = Fixture::new().await;
    let mut bot = f
        .service
        .claim(&f.bot_id, "lost", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    let lost = BotWorker::new(f.service.clone(), f.external.clone(), "lost".into());
    let secrets = lost.mint_pair(&mut bot, false).await.unwrap();
    // The remote create commits; its local response/checkpoint is lost.
    f.external.create_channel(&bot, &secrets).await.unwrap();
    f.ready().await;
    f.worker.run(&f.bot_id).await.unwrap();
    let bot = f.bot().await;
    assert_eq!(
        bot.state,
        BotState::Failed {
            reason: "interrupted_creation".into()
        }
    );
    assert!(bot.work.is_none());
    assert!(f.external.0.lock().unwrap().keys.is_empty());
}

#[tokio::test]
async fn rotation_recovers_lost_replies_without_revoking_retained_keys() {
    for (step, after) in [
        ("mint_agent", true),
        ("mint_route", true),
        ("set_callback", true),
        ("configure_enabled", true),
        ("set_route_key", true),
        ("revoke", true),
    ] {
        let f = Fixture::new().await;
        f.worker.run(&f.bot_id).await.unwrap();
        f.admit(BotOperation::Rotate).await;
        f.fail(step, after);
        assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
        f.ready().await;
        f.worker.run(&f.bot_id).await.unwrap();
        let bot = f.bot().await;
        assert_eq!(bot.state, BotState::Active);
        assert!(bot.work.is_none() && bot.resources.replacement.is_none());
        let remote = f.external.0.lock().unwrap();
        assert_eq!(remote.keys.len(), 2);
        assert!(remote
            .channel
            .as_ref()
            .unwrap()
            .uses(&bot.resources.credentials));
    }
}

#[tokio::test]
async fn rotation_reconciles_a_provider_change_that_preceded_a_failed_channel_write() {
    let f = Fixture::new().await;
    f.worker.run(&f.bot_id).await.unwrap();
    f.admit(BotOperation::Rotate).await;
    f.external.0.lock().unwrap().partial_binding = true;
    assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
    {
        let remote = f.external.0.lock().unwrap();
        assert_ne!(
            remote.provider_hash.as_ref().unwrap(),
            &remote.channel.as_ref().unwrap().agent_key_hash
        );
    }
    f.ready().await;
    f.worker.run(&f.bot_id).await.unwrap();
    let bot = f.bot().await;
    let remote = f.external.0.lock().unwrap();
    assert_eq!(remote.keys.len(), 2);
    assert_eq!(
        remote.provider_hash,
        bot.resources.credentials.agent_key_hash
    );
}

#[tokio::test]
async fn failed_completion_cannot_abandon_recorded_resources() {
    let f = Fixture::new().await;
    let mut bot = f
        .service
        .claim(&f.bot_id, "worker", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    f.worker.mint_pair(&mut bot, false).await.unwrap();
    assert_eq!(
        f.service
            .complete(
                &mut bot,
                "worker",
                BotState::Failed {
                    reason: "failed".into()
                }
            )
            .await,
        Err(BotError::Conflict)
    );
    assert!(f.bot().await.work.is_some());
}
