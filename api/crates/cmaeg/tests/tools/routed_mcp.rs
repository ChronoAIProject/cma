//! Gateway-wide discovery and parsing use the same process flags.
use super::support::App;
use cmaeg::mcp::tools::{list, ToolFlags};
use cmaeg_core::channel::ReplyChoice;
use serde_json::{json, Value};

#[test]
fn flag_selected_mcp_documents_match_their_byte_goldens() {
    let legacy = include_str!("../fixtures/mcp-tools-list.json").trim_end();
    let routed = include_str!("../fixtures/mcp-tools-list-routed.json").trim_end();
    for (flag, golden) in [(false, legacy), (true, routed)] {
        assert_eq!(
            list(ToolFlags {
                media: false,
                multi_consumer: flag
            })
            .to_string(),
            golden
        );
        let media = list(ToolFlags {
            media: true,
            multi_consumer: flag,
        });
        let mut base = media["tools"].as_array().unwrap().clone();
        assert_eq!(base.pop().unwrap()["name"], "readEventAttachment");
        assert_eq!(json!({"tools":base}).to_string(), golden);
    }
    assert!(!legacy.contains("ReplyChoice"));
    assert!(!legacy.contains("RoutedReplyArguments"));
    let sentence = "A routed send queued past the tool deadline returns `unknown` (`delivery_in_progress`) and is still sent, authorized as at the call; do not resend `unknown`.";
    assert!(!legacy.contains(sentence));
    for tool in list(ToolFlags {
        media: false,
        multi_consumer: true,
    })["tools"]
        .as_array()
        .unwrap()
    {
        let sends = matches!(
            tool["name"].as_str(),
            Some("replyToEvent" | "updateMessage")
        );
        assert_eq!(
            tool["description"].as_str().unwrap().contains(sentence),
            sends
        );
    }
}

#[tokio::test]
async fn routed_mcp_parses_destination_and_legacy_mcp_refuses_it() {
    for flag in [false, true] {
        let app = App::with_routing_flag(None, flag).await;
        for destination in [None, Some("origin"), Some("default"), Some("invalid")] {
            let before = app.destinations.lock().unwrap().len();
            let mut arguments =
                json!({"event_ref":app.event_ref.as_str(),"message":{"text":"MCP"}});
            if let Some(destination) = destination {
                arguments["destination"] = destination.into();
            }
            let response: Value = app
                .rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"replyToEvent","arguments":arguments}}))
                .await
                .json()
                .await
                .unwrap();
            let valid = destination.is_none() || (flag && destination != Some("invalid"));
            if valid {
                assert_eq!(
                    response["result"]["structuredContent"]["delivery"]["outcome"],
                    "confirmed"
                );
                let expected = destination.map(|choice| {
                    if choice == "default" {
                        ReplyChoice::Default
                    } else {
                        ReplyChoice::Origin
                    }
                });
                assert_eq!(app.destinations.lock().unwrap().last(), Some(&expected));
            } else {
                assert_eq!(response["error"]["data"]["code"], "invalid_request");
                assert_eq!(app.destinations.lock().unwrap().len(), before);
            }
        }
    }
}
