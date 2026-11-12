use super::*;
use crate::allocation_probe::AllocationProbe;
use serde_json::json;

fn frame(update: Value) -> Value {
    json!({"method":"session/update","params":{"sessionId":"s","update":update}})
}

fn metadata(fields: Value) -> Value {
    frame(json!({"sessionUpdate":"session_info_update","_meta":{"cma":{"runtimeMetadata":fields}}}))
}

fn usage(turn: &str) -> Value {
    frame(
        json!({"sessionUpdate":"usage_update","_meta":{"cma":{"usageSnapshot":{
            "turnId":turn,"inputTokens":1,"outputTokens":2,"lastInputTokens":3,
            "lastOutputTokens":4,"contextUsed":5,"contextWindow":6,"countersReset":true
        }}}}),
    )
}

fn equivalent(value: &Value, pool: &Arc<Semaphore>) {
    let legacy = crate::hosted::acp_observations::observation(&value["params"]["update"]);
    let admitted = admit(value, pool, None).unwrap();
    match (legacy, admitted.as_deref().map(ConversationUpdate::effect)) {
        (
            Some(ConversationUpdate::Metadata(expected)),
            Some(ConversationUpdate::Metadata(actual)),
        ) => {
            assert_eq!(actual, &expected);
        }
        (Some(ConversationUpdate::Usage(expected)), Some(ConversationUpdate::Usage(actual))) => {
            assert_eq!(actual, &expected);
        }
        (None, None) => {}
        _ => panic!("enabled and legacy observation behavior differ"),
    }
}

#[test]
fn borrowed_selection_preserves_metadata_usage_and_malformed_extension_behavior() {
    let pool = Arc::new(Semaphore::new(PROCESS_BYTES));
    for fields in [
        json!({"workingDirectory":"/workspace","description":"title","descriptionSource":"provider_title","descriptionInput":"exact\ninput","descriptionInputPosition":"first"}),
        json!({"descriptionSource":"prompt_preview","descriptionInput":"","descriptionInputPosition":"current"}),
        json!({"descriptionSource":"unknown","descriptionInput":"discarded","descriptionInputPosition":"unknown"}),
        json!({"workingDirectory":false,"description":null,"descriptionInput":17}),
        json!({}),
        Value::Null,
        json!(false),
        json!([]),
    ] {
        equivalent(&metadata(fields), &pool);
        assert_eq!(pool.available_permits(), PROCESS_BYTES);
    }
    let valid = usage("turn");
    equivalent(&valid, &pool);
    for key in [
        "turnId",
        "inputTokens",
        "outputTokens",
        "lastInputTokens",
        "lastOutputTokens",
        "contextUsed",
    ] {
        for invalid in [Value::Null, json!(-1), json!(false), json!({})] {
            let mut value = valid.clone();
            value["params"]["update"]["_meta"]["cma"]["usageSnapshot"][key] = invalid;
            equivalent(&value, &pool);
        }
    }
    for value in [
        frame(json!({})),
        frame(json!({"sessionUpdate":"agent_message_chunk"})),
        frame(json!({"sessionUpdate":"usage_update","used":3,"size":10})),
    ] {
        equivalent(&value, &pool);
    }
    let mut precedence = valid;
    precedence["params"]["update"]["_meta"]["cma"]["runtimeMetadata"] = json!({});
    equivalent(&precedence, &pool);
    assert_eq!(pool.available_permits(), PROCESS_BYTES);
}

#[test]
fn wrong_method_and_missing_session_refuse_selection_without_allocating_an_effect() {
    let pool = Arc::new(Semaphore::new(0));
    let mut value = metadata(json!({"description":"x".repeat(1024 * 1024)}));
    value["method"] = json!("other");
    assert!(admit(&value, &pool, None).unwrap().is_none());
    value["method"] = json!("session/update");
    value["params"]["sessionId"] = Value::Null;
    assert!(admit(&value, &pool, None).unwrap().is_none());
}

#[test]
fn maximal_metadata_and_usage_match_their_allowance_through_final_clone_drop() {
    for kind in ["metadata", "usage"] {
        let mut value = if kind == "metadata" {
            metadata(
                json!({"workingDirectory":"/workspace","description":"","descriptionInput":"exact input","descriptionInputPosition":"current"}),
            )
        } else {
            usage("")
        };
        let extra = 4 * 1024 * 1024 - serde_json::to_vec(&value).unwrap().len();
        let extension = &mut value["params"]["update"]["_meta"]["cma"];
        if kind == "metadata" {
            extension["runtimeMetadata"]["description"] = json!("x".repeat(extra));
        } else {
            extension["usageSnapshot"]["turnId"] = json!("x".repeat(extra));
        }
        let encoded = serde_json::to_vec(&value).unwrap();
        assert_eq!(encoded.len(), 4 * 1024 * 1024);
        crate::hosted::output_json_budget::inspect(&encoded).unwrap();
        let allowance = Fields::read(&value["params"]["update"])
            .unwrap()
            .weight()
            .unwrap();
        let pool = Arc::new(Semaphore::new(PROCESS_BYTES));
        let aggregate = IngressBudget::new(PROCESS_BYTES);
        drop(IngressReservation::try_acquire(&pool, Some(&aggregate), 1).unwrap());
        let mut probe = AllocationProbe::new();
        let owned = probe.track(|| admit(&value, &pool, Some(&aggregate)).unwrap().unwrap());
        let (retained, peak, allocated) = (probe.live(), probe.peak(), probe.allocated());
        assert_eq!(retained, allowance);
        assert_eq!(peak, allowance);
        let clone = probe.track(|| owned.clone());
        assert_eq!(probe.allocated(), allocated);
        drop(owned);
        assert_eq!(probe.live(), retained);
        assert_eq!(pool.available_permits(), PROCESS_BYTES - allowance);
        assert_eq!(aggregate.available_bytes(), PROCESS_BYTES - allowance);
        drop(clone);
        assert_eq!(probe.live(), 0);
        assert_eq!(pool.available_permits(), PROCESS_BYTES);
        assert_eq!(aggregate.available_bytes(), PROCESS_BYTES);
        eprintln!("Observation allocation {kind}: encoded={} allowance={allowance} retained={retained} peak={peak} final=0", encoded.len());
    }
}

#[test]
fn discarded_selectors_and_input_do_not_create_transient_string_copies() {
    let value = metadata(
        json!({"descriptionSource":"x".repeat(1024 * 1024),"descriptionInput":"y".repeat(1024 * 1024),"descriptionInputPosition":"unsupported"}),
    );
    let pool = Arc::new(Semaphore::new(PROCESS_BYTES));
    drop(IngressReservation::try_acquire(&pool, None, 1).unwrap());
    for bounded in [false, true] {
        let mut probe = AllocationProbe::new();
        let owned = probe.track(|| {
            if bounded {
                ConversationUpdate::RetainedEffect(admit(&value, &pool, None).unwrap().unwrap())
            } else {
                crate::hosted::acp_observations::observation(&value["params"]["update"]).unwrap()
            }
        });
        let peak = probe.peak();
        assert!(
            matches!(owned.effect(), ConversationUpdate::Metadata(metadata) if metadata == &ConversationMetadata::default())
        );
        if bounded {
            assert_eq!(peak, RuntimeOutputUpdate::ACCOUNTED_STORAGE_BYTES);
        } else {
            assert!(peak > 1024 * 1024);
        }
        drop(owned);
        assert_eq!(probe.live(), 0);
        eprintln!("Observation discarded-field allocation bounded={bounded}: peak={peak} final=0");
    }
}

#[test]
fn local_and_shared_refusal_copy_no_large_fields_and_rollback_partial_admission() {
    let value = metadata(json!({"description":"x".repeat(1024 * 1024)}));
    for shared in [false, true] {
        let pool = Arc::new(Semaphore::new(if shared { PROCESS_BYTES } else { 0 }));
        let aggregate = IngressBudget::new(0);
        if shared {
            drop(IngressReservation::try_acquire(&pool, None, 1).unwrap());
        }
        let before = pool.available_permits();
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| admit(&value, &pool, shared.then_some(&aggregate)).unwrap_err());
        assert!(matches!(error, AdapterError::AtCapacity(_)));
        assert!(
            probe.peak() < 1024,
            "refusal must not copy the selected megabyte"
        );
        drop(error);
        assert_eq!(probe.live(), 0);
        assert_eq!(pool.available_permits(), before);
        assert_eq!(aggregate.available_bytes(), 0);
    }
}

#[test]
fn observation_pressure_keeps_the_sign_in_component_independent() {
    let sign_ins = Arc::new(Semaphore::new(64 * 1024));
    let pools = crate::hosted::retained_json::JsonPools::new(64 * 1024, 64 * 1024)
        .with_sign_in_pool(sign_ins.clone())
        .with_observation_pool(Arc::new(Semaphore::new(0)));
    assert!(matches!(
        pools.admit_observation(&metadata(json!({}))),
        Err(AdapterError::AtCapacity(_))
    ));
    let notice =
        json!({"method":"elicitation/create","params":{"url":"https://example.invalid/login"}});
    let admitted = pools.admit_sign_in(&notice).unwrap().unwrap();
    assert!(sign_ins.available_permits() < 64 * 1024);
    drop(admitted);
    assert_eq!(sign_ins.available_permits(), 64 * 1024);
}
