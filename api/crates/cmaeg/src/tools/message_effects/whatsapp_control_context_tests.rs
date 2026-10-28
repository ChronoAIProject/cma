use super::*;

#[tokio::test]
async fn native_whatsapp_control_discovery_requires_its_own_gate_and_exact_owner() {
    for named in [false, true] {
        let (mut app, graph) = fixture_for(named).await;
        for (native, relay, consumers, expected) in [
            (false, false, false, false),
            (false, true, true, false),
            (true, false, false, !named),
            (true, false, true, true),
            (false, false, true, false),
        ] {
            app.state.native_whatsapp_interactions_enabled = native;
            app.state.whatsapp_interactions_enabled = relay;
            app.state.consumer_interactions_enabled = consumers;
            let response = context(&app).await;
            assert_eq!(
                response.capabilities.buttons,
                if expected {
                    "native_interactive"
                } else {
                    "numbered_text"
                }
            );
            assert!(!response.capabilities.edit);
        }
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_media_control_discovery_requires_all_private_owner_gates() {
    for named in [false, true] {
        let (mut app, graph) = fixture_for(named).await;
        for (media, controls, native, consumers, expected) in [
            (true, true, true, true, true),
            (false, true, true, true, false),
            (true, false, true, true, false),
            (true, true, false, true, false),
            (true, true, true, false, !named),
        ] {
            app.state.native_whatsapp_outbound_media_enabled = media;
            app.state.native_whatsapp_media_controls_enabled = controls;
            app.state.native_whatsapp_interactions_enabled = native;
            app.state.consumer_interactions_enabled = consumers;
            app.state.native_telegram_media_controls_enabled = true;
            app.state.native_discord_media_controls_enabled = true;
            app.state.whatsapp_interactions_enabled = true;
            let response = context(&app).await;
            let actual = response
                .capabilities
                .native_media
                .and_then(|native| native.controls);
            assert_eq!(actual.is_some(), expected && media);
            if let Some(actual) = actual {
                assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    json!({
                        "kinds":["image","file","video"], "max_attachments":1,
                        "buttons":true, "choices":false, "caption_max_chars":0, "update":false,
                    })
                );
            }
        }
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}
