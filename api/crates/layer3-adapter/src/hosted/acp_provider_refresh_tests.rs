use super::*;

#[tokio::test]
async fn provider_relay_confirmation_follows_same_session_reload_and_settings_acknowledgements() {
    use crate::hosted::{HostedProviderToolBinding, HostedProviderToolSource};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Source {
        old: HostedProviderToolBinding,
        new: HostedProviderToolBinding,
        confirmed: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl HostedProviderToolSource for Source {
        async fn resolve(
            &self,
            _: &crate::RuntimeHandle,
            _: &crate::AgentOpenRequest,
            _: u64,
        ) -> crate::Result<Option<HostedProviderToolBinding>> {
            unreachable!()
        }
        async fn confirm_open(&self, _: &HostedProviderToolBinding, _: &str) -> crate::Result<()> {
            unreachable!()
        }
        async fn prepare_transport(
            &self,
            binding: &HostedProviderToolBinding,
            origin: &str,
            conversation: &str,
            sequence: u64,
        ) -> crate::Result<HostedProviderToolBinding> {
            assert_eq!(binding, &self.old);
            assert_eq!(origin, "https://new.example");
            assert_eq!(conversation, "retained-session");
            assert_eq!(sequence, 1);
            Ok(self.new.clone())
        }
        async fn confirm_transport(
            &self,
            binding: &HostedProviderToolBinding,
            conversation: &str,
            sequence: u64,
        ) -> crate::Result<()> {
            assert_eq!(binding, &self.new);
            assert_eq!(conversation, "retained-session");
            assert_eq!(sequence, 1);
            self.confirmed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let old = provider_tool_tests::binding(Some(("b", "retained-session")), true);
    let new = old
        .clone()
        .with_transport(
            "https://new.example",
            crate::RuntimeCredential::new(format!("cma_pt_{}_{}", "a".repeat(64), "c".repeat(64))),
        )
        .unwrap();
    let source = Arc::new(Source {
        old: old.clone(),
        new: new.clone(),
        confirmed: AtomicUsize::new(0),
    });
    let unchanged_tool = serde_json::json!({"name":"owner_tool","type":"http","url":"https://owner.example/mcp","headers":[]});
    let (context, files) = crate::hosted::managed_context::writer_tests::binding_with_tools(
        0,
        vec![old.acp_server(), unchanged_tool.clone()],
    );
    let context = context.with_provider(source.clone(), old);
    let retained_context = context.clone();
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let confirmation = source.clone();
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"retained-session"}),
        )
        .await;
        for method in [
            "session/close",
            "session/load",
            "session/set_model",
            "session/set_config_option",
            "session/set_mode",
        ] {
            let call = receive_json(&mut server).await;
            assert_eq!(call["method"], method);
            assert_eq!(confirmation.confirmed.load(Ordering::SeqCst), 0);
            if method == "session/load" {
                assert_eq!(call["params"]["sessionId"], "retained-session");
                assert_eq!(
                    call["params"]["mcpServers"],
                    serde_json::json!([new.acp_server(), unchanged_tool])
                );
            }
            let result = if method == "session/set_config_option" {
                serde_json::json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]})
            } else {
                serde_json::json!({})
            };
            respond(&mut server, &call, result).await;
        }
        while server.next().await.is_some() {}
    });
    let mut config = settings(None, false);
    config.context_refresh = Some(context);
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        config,
        None,
        updates,
    )
    .await
    .unwrap();
    assert!(opened
        .channel
        .input
        .managed_context_receipt(1)
        .await
        .is_none());
    assert_eq!(files.goal_probes.load(Ordering::SeqCst), 0);
    let receipt = opened
        .channel
        .input
        .managed_transport_receipt(1)
        .await
        .unwrap();
    opened
        .channel
        .input
        .send(ConversationInput::RefreshContext {
            service_origin: Some("https://new.example".into()),
            operation_sequence: 1,
            readiness_receipt: receipt,
            guidance: "New managed".into(),
            previous_guidance: vec!["Old managed".into()],
            model: "gpt-6".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Autonomous,
            working_directory: None,
        })
        .await
        .unwrap();
    assert_eq!(source.confirmed.load(Ordering::SeqCst), 1);
    assert_eq!(files.goal_probes.load(Ordering::SeqCst), 2);
    assert_eq!(
        retained_context.tools()[0]["url"],
        "https://new.example/api/v1/provider-tools"
    );
    stop(opened).await;
    serving.await.unwrap();
}

#[test]
fn provider_relay_rejects_issuer_and_access_credentials_and_other_scopes() {
    let binding = provider_tool_tests::binding(Some(("b", "retained-session")), true);
    let token = format!("cma_pt_{}_{}", "a".repeat(64), "c".repeat(64));
    for invalid in [
        "issuer-secret".to_owned(),
        format!("cma_at_{}", "a".repeat(64)),
        format!("cma_pt_{}_{}", "b".repeat(64), "c".repeat(64)),
        format!("{token}_extra"),
    ] {
        assert!(binding
            .clone()
            .with_transport("https://new.example", invalid.into())
            .is_err());
    }
    for origin in [
        "http://new.example",
        "https://user@new.example",
        "https://new.example/path",
        "https://new.example?destination=evil",
        "https://new.example#fragment",
    ] {
        assert!(binding
            .clone()
            .with_transport(origin, token.clone().into())
            .is_err());
    }
    let migrated = binding
        .clone()
        .with_transport("https://new.example", token.into())
        .unwrap();
    assert_eq!(binding.scope(), migrated.scope());
    assert_eq!(binding.observed(), migrated.observed());
    assert!(migrated.retained_for("retained-session"));
    assert_ne!(binding.acp_server(), migrated.acp_server());
    assert_eq!(migrated.acp_server()["name"], "cmaeg");
    assert_eq!(
        format!("{migrated:?}"),
        "HostedProviderToolBinding(<protected>)"
    );
}
