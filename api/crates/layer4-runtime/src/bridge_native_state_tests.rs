use super::*;
use serde_json::json;

fn input(state: &mut NativeObservation, value: Value) {
    state.input(&serde_json::to_vec(&value).unwrap());
}
fn output(state: &mut NativeObservation, value: Value) {
    state.output(&serde_json::to_vec(&value).unwrap());
}
fn loaded() -> NativeObservation {
    let mut state = NativeObservation::default();
    input(
        &mut state,
        json!({"id":"load", "method":"session/load", "params":{"sessionId":"conversation"}}),
    );
    assert!(state.conversation_id.is_none());
    output(&mut state, json!({"id":"load", "result":{}}));
    assert_eq!(state.conversation_id.as_deref(), Some("conversation"));
    assert_eq!(state.native_state, Some(ConfirmedAgentState::Idle));
    state
}
fn working() -> (NativeObservation, String) {
    let mut state = loaded();
    let prompt = PromptIdentity::new("conversation", 1, 1)
        .as_str()
        .to_owned();
    input(
        &mut state,
        json!({"id":prompt, "method":"session/prompt", "params":{"sessionId":"conversation"}}),
    );
    assert_eq!(
        state.native_state,
        Some(ConfirmedAgentState::Working {
            prompt_id: prompt.clone()
        })
    );
    (state, prompt)
}
fn permission(call: Value) -> Value {
    json!({"id":call, "method":"session/request_permission", "params":{
        "sessionId":"conversation", "toolCall":{"toolCallId":"tool","title":"Permission"},
        "options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]
    }})
}

#[test]
fn open_requires_correlated_success_and_invalidates_previous_identity() {
    for method in ["session/load", "session/new"] {
        for result in [json!({}), json!({"sessionId":"conversation"})] {
            let mut state = loaded();
            input(
                &mut state,
                json!({"id":42,"method":method,"params":{"sessionId":"conversation"}}),
            );
            assert!(state.conversation_id.is_none());
            assert!(state.native_state.is_none());
            output(
                &mut state,
                json!({"id":41,"result":{"sessionId":"unrelated"}}),
            );
            assert!(state.conversation_id.is_none());
            output(&mut state, json!({"id":42,"result":result}));
            let succeeds = method == "session/load" || result.get("sessionId").is_some();
            assert_eq!(state.native_state.is_some(), succeeds);
            assert_eq!(state.conversation_id.is_some(), succeeds);
        }
    }
    for reply in [
        json!({"error":{"code":-32603,"message":"failed"}}),
        json!({"error":{},"result":{"sessionId":"conversation"}}),
        json!({"result":null}),
        json!({}),
        json!({"result":{"sessionId":"different"}}),
        json!({"result":{"sessionId":7}}),
    ] {
        let mut state = loaded();
        input(
            &mut state,
            json!({"id":"load", "method":"session/load", "params":{"sessionId":"conversation"}}),
        );
        let mut reply = reply;
        reply["id"] = json!("load");
        output(&mut state, reply);
        assert!(state.native_state.is_none());
        assert!(state.conversation_id.is_none());
    }
}

#[test]
fn unsolicited_duplicate_or_overlapping_open_responses_never_establish_identity() {
    let mut state = NativeObservation::default();
    output(
        &mut state,
        json!({"id":"unsolicited","result":{"sessionId":"conversation"}}),
    );
    assert!(state.conversation_id.is_none());
    for id in ["first", "second"] {
        input(
            &mut state,
            json!({"id":id,"method":"session/load","params":{"sessionId":"conversation"}}),
        );
    }
    for id in ["first", "second"] {
        output(&mut state, json!({"id":id,"result":{}}));
        assert!(state.conversation_id.is_none());
    }
    let (mut state, prompt) = working();
    output(
        &mut state,
        json!({"id":"load","result":{"sessionId":"different"}}),
    );
    assert_eq!(state.conversation_id.as_deref(), Some("conversation"));
    assert_eq!(
        state.native_state,
        Some(ConfirmedAgentState::Working { prompt_id: prompt })
    );
}

#[test]
fn only_the_exact_terminal_prompt_result_can_settle_work() {
    for unrelated in [
        json!({"id":"initialize","error":{"code":-32603,"message":"Already initialized"}}),
        json!({"id":"settings","result":{}}),
        json!({"id":71,"result":{"stopReason":"end_turn"}}),
        json!({"id":PromptIdentity::new("conversation",1,2).as_str(),"result":{"stopReason":"end_turn"}}),
        json!({"id":"native-call","method":"fs/read_text_file","params":{}}),
    ] {
        let (mut state, prompt) = working();
        output(&mut state, unrelated);
        assert_eq!(
            state.native_state,
            Some(ConfirmedAgentState::Working {
                prompt_id: prompt.clone()
            })
        );
        output(
            &mut state,
            json!({"id":prompt,"result":{"stopReason":"end_turn"}}),
        );
        assert_eq!(state.native_state, Some(ConfirmedAgentState::Idle));
    }
    for terminal in [
        json!({"result":{}}),
        json!({"error":{"code":-32603,"message":"failed"}}),
        json!({"result":{"stopReason":"unknown"}}),
    ] {
        let (mut state, prompt) = working();
        let mut terminal = terminal;
        terminal["id"] = json!(prompt);
        output(&mut state, terminal);
        assert!(state.native_state.is_none());
    }
}

#[test]
fn standard_permission_identity_survives_repeat_reply_and_cancellation() {
    for call in [json!(71), json!("permission")] {
        for answer in [
            json!({"outcome":"selected","optionId":"allow"}),
            json!({"outcome":"cancelled"}),
        ] {
            let (mut state, prompt) = working();
            let frame = permission(call.clone());
            let request: RequestPermissionRequest =
                serde_json::from_value(frame["params"].clone()).unwrap();
            let expected = layer3_adapter::hosted::observed_permission_id(&request, &call).unwrap();
            for _ in 0..2 {
                output(&mut state, frame.clone());
                assert_eq!(
                    state.native_state,
                    Some(ConfirmedAgentState::WaitingApproval {
                        prompt_id: prompt.clone(),
                        approval_id: expected.clone()
                    })
                );
            }
            output(&mut state, json!({"id":"settings","result":{}}));
            input(
                &mut state,
                json!({"id":"unrelated","result":{"outcome":answer}}),
            );
            assert!(matches!(
                state.native_state,
                Some(ConfirmedAgentState::WaitingApproval { .. })
            ));
            input(&mut state, json!({"id":call,"result":{"outcome":answer}}));
            assert_eq!(
                state.native_state,
                Some(ConfirmedAgentState::Working {
                    prompt_id: prompt.clone()
                })
            );
            output(&mut state, frame);
            output(
                &mut state,
                json!({"id":prompt,"result":{"stopReason":"cancelled"}}),
            );
            assert_eq!(state.native_state, Some(ConfirmedAgentState::Idle));
        }
    }
}

#[test]
fn malformed_or_conflicting_permissions_and_uncertain_sessions_fail_closed() {
    for variant in 0..6 {
        let (mut state, _) = working();
        let mut frame = permission(json!(71));
        match variant {
            0 => frame["params"]["sessionId"] = json!("foreign"),
            1 => frame["id"] = Value::Null,
            2 => frame["params"]["options"] = json!([]),
            3 => {
                output(&mut state, permission(json!(70)));
            }
            4 => frame["params"] = json!({}),
            _ => frame["params"]["options"][0]["kind"] = json!("unknown"),
        }
        output(&mut state, frame);
        assert!(state.native_state.is_none(), "variant {variant}");
    }
    for answer in [
        json!({"error":{}}),
        json!({"result":{"outcome":{"outcome":"selected","optionId":"foreign"}}}),
    ] {
        let (mut state, _) = working();
        output(&mut state, permission(json!(71)));
        let mut answer = answer;
        answer["id"] = json!(71);
        input(&mut state, answer);
        assert!(state.native_state.is_none());
    }
    for method in ["session/close", "session/cancel"] {
        let mut state = loaded();
        input(
            &mut state,
            json!({"method":method,"params":{"sessionId":"conversation"}}),
        );
        assert!(state.conversation_id.is_none());
    }
    for inbound in [false, true] {
        let mut state = loaded();
        if inbound {
            state.input(b"invalid");
        } else {
            state.output(b"invalid");
        }
        assert!(state.native_state.is_none());
    }
}

#[test]
fn reloading_cannot_erase_work_or_uncertainty() {
    for waiting in [false, true] {
        let (mut state, _) = working();
        if waiting {
            output(&mut state, permission(json!(71)));
        }
        for _ in 0..2 {
            input(
                &mut state,
                json!({"id":"reload", "method":"session/load", "params":{"sessionId":"conversation"}}),
            );
            output(&mut state, json!({"id":"reload", "result":{}}));
            assert!(state.native_state.is_none());
            assert!(state.conversation_id.is_none());
        }
    }
}
