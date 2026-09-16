//! Production availability composition against exact published registry entries.
use super::*;
use crate::bridge::control_runtime::LocalChannelControlRuntime;
use layer2_core::sandbox::{AgentRuntimeRegistry, DurableAgentUpdateRecorder};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::ConversationInputSender;

fn context_runtime(c: &mut ControlFixture, runtime: Option<Arc<LocalChannelControlRuntime>>) {
    c.f.router = crate::bridge::channel_tools::router(
        Arc::new(crate::bridge::channel_tools::StoredChannelTools {
            capabilities: c.f.tools.clone(),
            replies: c.f.replies.clone(),
            control_runtime: runtime,
        }),
        "https://cma.example",
    );
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_production_runtime_and_discovery_require_exact_published_capabilities(
) {
    let mut c = ControlFixture::with_callback_route().await;
    let agent =
        c.f.resources
            .get_agent(&c.f.command.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
    let writes = Arc::new(AtomicUsize::new(0));
    for (index, mode) in [
        "generation",
        "conversation",
        "unpublished",
        "capability",
        "conversation-capability",
        "cancel-only",
    ]
    .into_iter()
    .enumerate()
    {
        let registry = Arc::new(crate::sandbox::LiveAgentRegistry::default());
        let observed = writes.clone();
        let input = ConversationInputSender::spawn(4, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        });
        let capabilities = match mode {
            "capability" => RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            "conversation-capability" => {
                RuntimeCapabilities::new([RuntimeCapability::Interrupt, RuntimeCapability::Steer])
            }
            "cancel-only" => RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Interrupt,
            ]),
            _ => agent.capabilities.clone(),
        };
        let (_updates, rx) = tokio::sync::mpsc::channel(4);
        let attachment = registry
            .attach(
                agent.id.clone(),
                if mode == "conversation" {
                    "another-conversation".into()
                } else {
                    agent.conversation.conversation_id.clone().unwrap()
                },
                agent.conversation.runtime_generation.unwrap() + u64::from(mode == "generation"),
                1,
                capabilities,
                input,
                rx,
                DurableAgentUpdateRecorder::new(c.f.resources.clone()),
            )
            .unwrap();
        if mode != "unpublished" {
            registry.publish_and_activate(&attachment).unwrap();
        }
        let runtime = Arc::new(LocalChannelControlRuntime::new(registry.clone()));
        context_runtime(&mut c, Some(runtime.clone()));
        let context = c.f.call("read_channel_control_context", json!({})).await;
        let context = &context["structuredContent"];
        assert_eq!(
            context["source_control_reference"], c.reference.reference,
            "{mode}"
        );
        assert_eq!(
            context["operations"],
            if mode == "cancel-only" {
                json!(["cancel"])
            } else {
                json!([])
            },
            "{mode}"
        );
        assert_eq!(
            context["availability"],
            if mode == "cancel-only" {
                "available"
            } else {
                "execution_unavailable"
            }
        );
        let value = body(
            &c,
            &format!("production-runtime-{mode}"),
            800 + index as u64,
            true,
        );
        let (status, result) = post(app(&c, true, Some(runtime)).await, &c, &value).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{mode}: {result}");
        no_message_effects(&c, 0).await;
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        AgentRuntimeRegistry::fence(registry.as_ref(), &agent.id, 1)
            .await
            .unwrap();
    }
    // A configured admission flag plus exact original local runtime advertises
    // only the actual pinned operations; discovering them performs no command.
    let runtime = Arc::new(LocalChannelControlRuntime::new(c.f.registry.clone()));
    context_runtime(&mut c, Some(runtime));
    let context = c.f.call("read_channel_control_context", json!({})).await;
    assert_eq!(
        context["structuredContent"]["operations"],
        json!(["cancel", "steer"])
    );
    assert_eq!(context["structuredContent"]["availability"], "available");
    assert_eq!(
        context["structuredContent"]["approval_mode"],
        "dashboard_only"
    );
    no_message_effects(&c, 0).await;
    context_runtime(&mut c, None);
    let context = c.f.call("read_channel_control_context", json!({})).await;
    assert_eq!(context["structuredContent"]["operations"], json!([]));
    assert_eq!(
        context["structuredContent"]["availability"],
        "execution_unavailable"
    );
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    c.f.close().await;
}
