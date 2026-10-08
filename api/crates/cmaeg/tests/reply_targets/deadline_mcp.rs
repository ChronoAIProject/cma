//! Real HTTP MCP harness: a queued default reply survives a twenty-second hold.
use super::support::App;
use cmaeg_core::channel::Source;
use serde_json::{json, Value};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn mcp_default_reply_answers_by_eight_seconds_and_sends_after_twenty() {
    for routed in [false, true] {
        let logs = Logs::default();
        let writer = logs.clone();
        let _capture = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        );
        let mut app = App::new(vec![]).await;
        if routed {
            let route = "6f1c9a3e-2b4d-4c5e-8f70-112233445566";
            let Source::NyxidRelay { route_ids, .. } = &mut app.definition.sources[0] else {
                panic!("relay fixture")
            };
            route_ids.push(route.into());
            app.definition.routing = Some(
                serde_json::from_value(json!({"version":1,"default_consumer":{"name":"lead"},
                "consumers":[],"default_targets":["default"],"default_reply":{
                    "type":"relay_conversation","source_id":"relay_telegram","route_id":route
                }}))
                .unwrap(),
            );
            app.update().await;
            app.state.multi_consumer_enabled = true;
        }
        let reference = app.event("alice", "held-mcp").await;
        let commands = app.hold_delivery().await;
        let mut arguments = json!({"event_ref":reference,"message":{"text":"held MCP reply"}});
        if routed {
            arguments["destination"] = "default".into();
        }
        let started = Instant::now();
        let response = app
            .client
            .post(format!("{}/mcp", app.base))
            .bearer_auth(&app.key)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"replyToEvent","arguments":arguments}}))
            .send()
            .await
            .unwrap();
        let value: Value = response.json().await.unwrap();
        assert!(started.elapsed() <= Duration::from_millis(8100), "{value}");
        if routed {
            assert_eq!(
                value["result"]["structuredContent"]["delivery"],
                json!({"outcome":"unknown","reason":"delivery_in_progress"})
            );
        } else {
            assert_eq!(value["error"]["data"]["status"], 504);
            assert_eq!(value["error"]["data"]["detail"], "delivery_timeout");
        }
        assert!(app.relay.requests.lock().unwrap().is_empty());
        tokio::time::sleep_until(started + Duration::from_secs(20)).await;
        app.release_delivery(commands);
        app.drain().await;
        let requests = app.relay.requests.lock().unwrap();
        assert_eq!(requests.len(), usize::from(routed));
        if routed {
            assert_eq!(requests[0].path, "/api/v1/channel-relay/send");
            assert!(requests[0].body["message"]["text"]
                .as_str()
                .unwrap()
                .ends_with("held MCP reply"));
        }
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            logs.matches("routed tool send finished after its caller was answered")
                .count(),
            usize::from(routed)
        );
    }
}
