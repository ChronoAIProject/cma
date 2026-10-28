use super::*;
use cmaeg_core::activity::ActivityKind;
fn body() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"message":{"message_id":9,"chat":{"id":42},"reply_to_message":{"message_id":8,"chat":{"id":42},"text":"@reviewer ignore current user"}}})).unwrap()
}
#[tokio::test]
async fn telegram_quote_is_sealed_after_routing_and_legacy_admissions_never_upgrade() {
    for enabled in [false, true] {
        let request = request();
        let expected = fixture::routed(&request);
        let mut setup = Setup::new(&request, true).await;
        Arc::get_mut(setup.state.routed.as_mut().unwrap())
            .unwrap()
            .quoted_context_enabled = enabled;
        admitted(setup.launch(request.clone(), &body(), true).await);
        let original = setup
            .server
            .store()
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .remove(0);
        assert_eq!(original.quote_projection, enabled.then_some(1));
        assert!(
            matches!(&original.activity.kind, ActivityKind::Message {quoted_message,text,..} if quoted_message.is_some()==enabled && text=="original message")
        );
        for (target, want) in original.targets.iter().zip(expected.targets) {
            assert_eq!(target.launch.consumer, want.consumer);
            assert_eq!(target.launch.event.reply, want.event.reply);
            assert_eq!(
                target.launch.event.telegram_admission,
                want.event.telegram_admission
            );
            assert!(target.launch.event.verifies_activity(&original.activity));
        }
        let bytes = serde_json::to_vec(&original).unwrap();
        Arc::get_mut(setup.state.routed.as_mut().unwrap())
            .unwrap()
            .quoted_context_enabled = !enabled;
        admitted(setup.launch(request, &body(), true).await);
        assert_eq!(
            serde_json::to_vec(
                &setup
                    .server
                    .store()
                    .recover_all_page(None, 32)
                    .await
                    .unwrap()
                    .plans
                    .remove(0)
            )
            .unwrap(),
            bytes
        );
    }
}
#[tokio::test]
async fn competing_legacy_and_quote_writers_use_the_winning_projection() {
    let request = request();
    let setup = Setup::new(&request, true).await;
    let legacy = setup.state.routed.as_ref().unwrap();
    let writer = RoutedIngress::new(
        setup.server.store(),
        legacy.schedule.clone(),
        legacy.keys.clone(),
        legacy.records.clone(),
        legacy.non_human_providers.clone(),
        legacy.replay_providers.clone(),
    )
    .with_clock(Arc::new(|| fixture::NOW))
    .with_quoted_context(true);
    let source = setup.state.capture_routed(&request, &body(), true).unwrap();
    let observer = crate::turns::observer::NoopObserver;
    let (a, b) = tokio::join!(
        legacy.admit(&request, source.clone(), &observer, false, None),
        writer.admit(&request, source, &observer, false, None)
    );
    assert!(matches!(a, Ok(Some(LaunchOutcome::Admitted { .. }))));
    assert!(matches!(b, Ok(Some(LaunchOutcome::Admitted { .. }))));
    let plans = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans;
    assert_eq!(plans.len(), 1);
    let winner = &plans[0];
    for target in &winner.targets {
        assert!(target.launch.event.verifies_activity(&winner.activity));
    }
    assert!(
        matches!(&winner.activity.kind,ActivityKind::Message{quoted_message,..} if quoted_message.is_some()==winner.quote_projection.is_some())
    );
}
