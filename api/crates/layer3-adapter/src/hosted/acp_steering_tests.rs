use super::*;

#[tokio::test]
async fn steering_keeps_one_prompt_waiter_and_cancel_survives_a_long_turn() {
    for supported in [true, false] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let (release, held) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(&mut server, &initialize, serde_json::json!({})).await;
            let open = receive_json(&mut server).await;
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"held-turn"}),
            )
            .await;
            let prompt = receive_json(&mut server).await;
            assert_eq!(prompt["method"], "session/prompt");
            let support = receive_json(&mut server).await;
            assert_eq!(support["method"], "cma/steer");
            assert_eq!(support["params"]["action"], "support");
            if supported {
                respond(
                    &mut server,
                    &support,
                    serde_json::json!({"supported":true,"version":1}),
                )
                .await;
                let steer = receive_json(&mut server).await;
                assert_eq!(steer["method"], "cma/steer");
                assert_eq!(steer["params"]["sessionId"], "held-turn");
                respond(
                    &mut server,
                    &steer,
                    serde_json::json!({"injected":true,"sessionId":"held-turn"}),
                )
                .await;
            } else {
                reject(&mut server, &support).await;
            }
            // The long task runs with no pending transport call or setup timer.
            held.await.unwrap();
            let cancel = receive_json(&mut server).await;
            assert_eq!(cancel["method"], "session/cancel");
            respond(
                &mut server,
                &prompt,
                serde_json::to_value(PromptResponse::new(TurnStopReason::Cancelled)).unwrap(),
            )
            .await;
            while server.next().await.is_some() {}
        });
        let (updates, mut receiver) = mpsc::channel(8);
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            settings(None, false),
            None,
            updates,
        )
        .await
        .unwrap();
        opened
            .channel
            .input
            .send(ConversationInput::RecoverablePrompt {
                content: vec![ContentBlock::from("Long-running task")],
                prompt_id: crate::PromptIdentity::new("held-turn", 7, 1),
            })
            .await
            .unwrap();
        let result = opened
            .channel
            .input
            .send_classified(ConversationInput::Steer(vec![ContentBlock::from(
                "Refine current work",
            )]))
            .await;
        if supported {
            assert!(matches!(
                result,
                crate::ConversationSendOutcome::WriteConfirmed
            ));
        } else {
            assert!(matches!(
                result,
                crate::ConversationSendOutcome::PreWriteFailure(crate::AdapterError::Unsupported(
                    _
                ))
            ));
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(90 * 60)).await;
        tokio::task::yield_now().await;
        assert!(
            receiver.try_recv().is_err(),
            "elapsed time must not invent a failure or terminal"
        );
        assert_eq!(
            opened
                .bridge
                .owned_prompt_waiters
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        tokio::time::resume();
        release.send(()).unwrap();
        opened
            .channel
            .input
            .send(ConversationInput::Cancel)
            .await
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(outcome, ConversationUpdate::TurnEnded(response) if response.stop_reason == TurnStopReason::Cancelled)
        );
        assert!(
            receiver.try_recv().is_err(),
            "steering has no second terminal"
        );
        stop(opened).await;
        serving.await.unwrap();
    }
}
