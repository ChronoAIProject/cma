use super::*;
use cmaeg_core::seal::ReplyAddress;

#[tokio::test]
async fn native_whatsapp_group_context_never_advertises_controls_for_any_consumer_gate() {
    use crate::whatsapp::groups::tests::{reply, source_group, GROUP};
    for named in [false, true] {
        let (mut app, graph) = fixture_for(named).await;
        app.definition.sources = vec![source_group()];
        app.replace();
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        app.event.reply = ReplyAddress::WhatsappGroup(reply());
        app.activity.conversation.id = GROUP.into();
        app.activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
        for enabled in [false, true] {
            app.state.native_whatsapp_interactions_enabled = enabled;
            app.state.whatsapp_interactions_enabled = enabled;
            app.state.consumer_interactions_enabled = enabled;
            app.state.native_whatsapp_groups_enabled = true;
            app.state.native_whatsapp_templates_enabled = true;
            let consumer = channel.consumer(app.event.consumer.as_ref()).unwrap();
            let response = crate::tools::context::project(
                &channel,
                &consumer,
                &app.event,
                app.activity.clone(),
                true,
                None,
                &app.state,
            );
            assert_eq!(response.capabilities.buttons, "none");
            assert!(!response.capabilities.edit);
            assert_eq!(
                response.capabilities.whatsapp_template_components,
                Some(vec!["header".into(), "body".into()])
            );
        }
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}
