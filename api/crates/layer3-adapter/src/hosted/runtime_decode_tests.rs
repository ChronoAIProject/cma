use super::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn equivalent<T: DeserializeOwned + Serialize>(value: Value) -> bool {
    let legacy = T::deserialize(&value);
    let enabled = decode::<T>(&value, true);
    let disabled = decode::<T>(&value, false);
    match legacy {
        Ok(expected) => {
            let expected = serde_json::to_value(expected).unwrap();
            assert_eq!(serde_json::to_value(enabled.unwrap()).unwrap(), expected);
            assert_eq!(serde_json::to_value(disabled.unwrap()).unwrap(), expected);
            true
        }
        Err(expected) => {
            assert!(enabled.is_err(), "malformed input must stay rejected");
            let disabled = disabled.err().expect("disabled behavior stays rejected");
            assert_eq!(disabled.to_string(), expected.to_string());
            false
        }
    }
}

#[test]
fn session_updates_preserve_content_metadata_and_tolerant_fields() {
    let mut valid = 0;
    for value in [
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello","_meta":{"nested":[1,true,null,{"x":"y"}]}}}),
        json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}),
        json!({"sessionUpdate":"user_message_chunk","content":{"type":"image","data":"data","mimeType":"image/png"}}),
        json!({"sessionUpdate":"tool_call","toolCallId":"call","title":"read","kind":"read","status":"completed","content":[{"type":"content","content":{"type":"text","text":"output"}}],"locations":[{"path":"/workspace/file","line":1}],"rawInput":{"a":[1,2]},"rawOutput":[null,"value"]}),
        json!({"sessionUpdate":"tool_call_update","toolCallId":"call","title":null,"kind":"unknown-kind","content":[false,{"type":"content","content":{"type":"text","text":"output"}}],"_meta":{"valid":true}}),
        json!({"sessionUpdate":"plan","entries":[{"content":"work","priority":"medium","status":"pending"}]}),
        json!({"sessionUpdate":"available_commands_update","availableCommands":[]}),
        json!({"sessionUpdate":"current_mode_update","currentModeId":"mode"}),
        json!({"sessionUpdate":"config_option_update","configOptions":[]}),
        json!({"sessionUpdate":"session_info_update","title":null,"_meta":{"test":1}}),
        json!({"sessionUpdate":"usage_update","used":4,"size":100}),
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"resource","resource":{"uri":"file:///example","text":"contents"}}}),
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"unknown","text":"x"}}),
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":false}}),
        json!({"sessionUpdate":null}),
        json!({"sessionUpdate":0}),
        json!({"sessionUpdate":{}}),
        Value::Null,
        json!([]),
    ] {
        valid += usize::from(equivalent::<crate::SessionUpdate>(value));
    }
    assert!(
        valid >= 10,
        "the matrix must exercise successful typed decoding"
    );
}

#[test]
fn permission_and_terminal_shapes_keep_their_existing_results() {
    for request in [
        json!({"sessionId":"s","toolCall":{"toolCallId":"t","title":"approve"},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"}]}),
        json!({"sessionId":"s","toolCall":{"toolCallId":"t","rawInput":{"deep":[null,true,42]}},"options":[],"_meta":{"a":"b"}}),
    ] {
        assert!(equivalent::<crate::RequestPermissionRequest>(request));
    }
    for request in [
        json!({"sessionId":1,"toolCall":{},"options":[]}),
        json!({"sessionId":"s","toolCall":null,"options":false}),
        json!({}),
    ] {
        assert!(!equivalent::<crate::RequestPermissionRequest>(request));
    }
    for result in [
        json!({"stopReason":"end_turn"}),
        json!({"stopReason":"cancelled","_meta":{"nested":[1,2,3]}}),
        json!({"stopReason":"max_tokens","usage":{"inputTokens":0,"outputTokens":9,"totalTokens":9}}),
    ] {
        assert!(equivalent::<crate::PromptResponse>(result));
    }
    for result in [
        json!({}),
        json!({"stopReason":11300}),
        json!({"stopReason":"unknown"}),
    ] {
        assert!(!equivalent::<crate::PromptResponse>(result));
    }
}

#[test]
fn borrowed_value_adapters_preserve_enum_number_option_and_container_semantics() {
    #[derive(Deserialize, Serialize)]
    enum Example {
        Unit,
        New(u64),
        Tuple(u8, String),
        Object { flag: bool },
    }
    for value in [
        json!("Unit"),
        json!({"Unit":null}),
        json!({"New":9}),
        json!({"Tuple":[3,"text"]}),
        json!({"Object":{"flag":true}}),
    ] {
        assert!(equivalent::<Example>(value));
    }
    for value in [
        json!({"Unit":1}),
        json!({"New":-1}),
        json!({"New":9.0}),
        json!({"New":1,"Unit":null}),
        json!({"Tuple":[3]}),
        json!({"Tuple":[3,"text",1]}),
        json!({"Object":{"flag":0}}),
    ] {
        assert!(!equivalent::<Example>(value));
    }
    for value in [
        json!(0),
        json!(u64::MAX),
        json!(-1),
        json!(i64::MIN),
        json!(1.5),
        json!("3"),
        Value::Null,
        json!([]),
    ] {
        equivalent::<u64>(value.clone());
        equivalent::<i64>(value.clone());
        equivalent::<f64>(value.clone());
        equivalent::<Option<u64>>(value);
    }
    assert!(equivalent::<Value>(
        json!({"array":[null,true,1,-1,1.5,"text",{}],"_meta":{"unknown":"preserved"}})
    ));
}
