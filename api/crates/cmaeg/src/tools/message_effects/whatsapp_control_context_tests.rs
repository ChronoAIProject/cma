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
