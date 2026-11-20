use super::*;
use futures_util::FutureExt;

async fn failure(input: OwnedWork<ConversationInput>) -> Result<(), AdmittedError> {
    let value = input
        .map(|input| match input {
            ConversationInput::Configure { model, .. } => AdapterError::Unsupported(model),
            _ => unreachable!(),
        })
        .into_shared_admitted()?;
    Err(value.into())
}

#[tokio::test]
async fn an_enqueued_result_keeps_its_error_until_the_unpolled_receiver_drops() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let mut operation = Box::pin({
        let sender = sender.clone();
        let input = input(&policy, &"x".repeat(4096));
        async move { sender.send_classified(input).await }
    });
    assert!(operation.as_mut().now_or_never().is_none());
    let ready = Arc::new(tokio::sync::Notify::new());
    let signal = ready.clone();
    let driver = tokio::spawn(receiver.run(move |input| {
        let signal = signal.clone();
        async move {
            let result = failure(input).await;
            signal.notify_one();
            result
        }
    }));
    ready.notified().await;
    drop(sender);
    assert!(root.reserved_work_bytes() >= 4096);
    drop(operation);
    driver.await.unwrap();
    root.assert_idle();
}

#[tokio::test]
async fn a_completed_unjoined_sender_keeps_the_owned_result_after_driver_teardown() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let driver = tokio::spawn(receiver.run(failure));
    let operation = tokio::spawn({
        let input = input(&policy, &"x".repeat(4096));
        async move { sender.send_classified(input).await }
    });
    while !operation.is_finished() {
        tokio::task::yield_now().await;
    }
    driver.await.unwrap();
    assert!(root.reserved_work_bytes() >= 4096);
    drop(operation);
    root.assert_idle();
}

#[tokio::test]
async fn handler_panic_is_ambiguous_and_frees_the_owned_input_after_unwind() {
    async fn handler(input: OwnedWork<ConversationInput>) -> Result<(), AdmittedError> {
        let _value = input;
        panic!("synthetic input handler failure");
    }
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let driver = tokio::spawn(receiver.run(handler));
    let result = sender
        .send_classified(input(&policy, &"x".repeat(4096)))
        .await;
    assert!(matches!(
        result,
        AdmittedConversationSendOutcome::Ambiguous(AdmittedError::Unavailable(_))
    ));
    assert!(driver.await.unwrap_err().is_panic());
    assert!(sender.is_closed());
    drop(sender);
    root.assert_idle();
}

#[test]
fn observations_reject_raw_and_foreign_root_watches() {
    let (root, policy) = fixture();
    let (other_root, other) = fixture();
    let (_, raw_progress) = watch::channel(0);
    let (_, raw_history) = watch::channel(None);
    let (_, foreign_progress) =
        AdmittedWatchSender::channel(&other, IngressClass::Data, 0).unwrap();
    let (_, foreign_history) =
        AdmittedWatchSender::channel(&other, IngressClass::Data, None).unwrap();
    for observation in [
        DeliveryObservation::legacy(Some(raw_progress), None),
        DeliveryObservation::legacy(None, Some(raw_history)),
        DeliveryObservation::admitted(Some(foreign_progress), None),
        DeliveryObservation::admitted(None, Some(foreign_history)),
    ] {
        let (sender, receiver) =
            AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
        assert!(matches!(
            sender.with_observation(observation),
            Err(AdmissionRefusal::InvalidPlan {
                phase: "input observation ownership"
            })
        ));
        drop(receiver);
        root.assert_idle();
    }
    other_root.assert_idle();
}

#[tokio::test(start_paused = true)]
async fn admitted_observations_extend_loading_then_restore_the_delivery_deadline() {
    for configure in [false, true] {
        let (root, policy) = fixture();
        let (progress, progress_rx) =
            AdmittedWatchSender::channel(&policy, IngressClass::Data, 0).unwrap();
        let (history, history_rx) =
            AdmittedWatchSender::channel(&policy, IngressClass::Data, None).unwrap();
        let (sender, receiver) =
            AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
        let sender = sender
            .with_observation(DeliveryObservation::admitted(
                Some(progress_rx),
                Some(history_rx),
            ))
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let driver = tokio::spawn(receiver.run({
            let entered = entered.clone();
            let history = history.clone();
            move |input| {
                let entered = entered.clone();
                let history = history.clone();
                async move {
                    let _input = input;
                    // Publish before waking the caller: a stale receiver clone
                    // taken after enqueue would miss this transition.
                    history
                        .send_replace(Some(tokio::time::Instant::now() + Duration::from_secs(960)));
                    entered.notify_one();
                    std::future::pending::<Result<(), AdmittedError>>().await
                }
            }
        }));
        let payload = if configure {
            input(&policy, "model")
        } else {
            IngressWorkScope::new(
                policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "refresh fixture",
                            &[64, sizing::shared_owner_bytes::<()>()],
                        )
                        .unwrap(),
                    )
                    .unwrap(),
            )
            .initialize_owned(|| ConversationInput::RefreshContext {
                service_origin: None,
                operation_sequence: 1,
                readiness_receipt: "receipt".into(),
                guidance: "guidance".into(),
                previous_guidance: Vec::new(),
                model: "model".into(),
                reasoning_effort: crate::ReasoningEffort::High,
                permission: crate::PermissionLevel::Guarded,
                working_directory: None,
            })
        };
        let send = tokio::spawn(async move { sender.send_classified(payload).await });
        entered.notified().await;
        tokio::task::yield_now().await;
        for cursor in 1..=2 {
            tokio::time::advance(Duration::from_secs(240)).await;
            tokio::task::yield_now().await;
            assert!(!send.is_finished());
            progress.send_replace(cursor);
            tokio::task::yield_now().await;
        }
        history.send_replace(None);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(14)).await;
        tokio::task::yield_now().await;
        assert!(!send.is_finished());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(matches!(
            send.await.unwrap(),
            AdmittedConversationSendOutcome::Ambiguous(AdmittedError::Unavailable(_))
        ));
        driver.await.unwrap();
        drop((progress, history));
        root.assert_idle();
    }
}
