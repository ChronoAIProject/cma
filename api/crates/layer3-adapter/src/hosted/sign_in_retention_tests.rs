use super::*;
use crate::allocation_probe::AllocationProbe;
use serde_json::json;

#[test]
fn selected_fields_preserve_legacy_precedence_and_invalid_input() {
    for params in [
        Value::Null,
        json!({}),
        json!({"url":12}),
        json!({"url":"","userCode":"","code":"fallback","message":""}),
        json!({"url":"login","userCode":false,"code":"fallback","message":null}),
        json!({"url":"login","userCode":"first","code":"second","message":"info"}),
    ] {
        let legacy = crate::hosted::acp_inbound::sign_in_from(Some(&params));
        let selected = Fields::read(Some(&params)).map(|fields| {
            let ConversationUpdate::SignInRequired { url, code, message } = fields.owned() else {
                panic!("sign-in effect expected")
            };
            (url, code, message)
        });
        assert_eq!(selected, legacy);
    }
    assert!(Fields::read(None).is_none());
}

#[test]
fn local_and_shared_refusal_roll_back_before_string_copies() {
    let frame = json!({"method":"elicitation/create","params":{"url":"x".repeat(1024*1024)}});
    let fields = Fields::read(frame.get("params")).unwrap();
    let weight = fields.weight().unwrap();
    let pool = Arc::new(Semaphore::new(weight));
    let root = IngressBudget::new(weight - 1);
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| admit(&frame, &pool, Some(&root))),
        Err(AdapterError::AtCapacity(_))
    ));
    assert!(probe.peak() < 1024, "refusal cannot copy the large URL");
    assert_eq!(probe.live(), 0);
    assert_eq!(pool.available_permits(), weight);
    assert_eq!(root.available_bytes(), weight - 1);
    let blocker = pool.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        admit(&frame, &pool, None),
        Err(AdapterError::AtCapacity(_))
    ));
    drop(blocker);
    assert_eq!(pool.available_permits(), weight);
    assert!(admit(
        &json!({"method":"elicitation/create","params":{}}),
        &pool,
        Some(&root)
    )
    .unwrap()
    .is_none());
    assert!(admit(
        &json!({"method":"different","params":{"url":"unused"}}),
        &pool,
        Some(&root)
    )
    .unwrap()
    .is_none());
}

#[test]
fn maximal_notice_allocation_fits_its_reservation_through_the_final_clone() {
    let frame = json!({"method":"elicitation/create","params":{
        "url":"https://example.invalid/sign-in", "userCode":"1234", "code":"unused",
        "message":"x".repeat(4 * 1024 * 1024 - 512)
    }});
    let encoded = serde_json::to_vec(&frame).unwrap();
    crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    let fields = Fields::read(frame.get("params")).unwrap();
    let weight = fields.weight().unwrap();
    let pool = Arc::new(Semaphore::new(weight));
    let root = IngressBudget::new(weight);
    let mut probe = AllocationProbe::new();
    let notice = probe.track(|| {
        admit(&frame, &pool, Some(&root))
            .unwrap()
            .unwrap()
            .wrap(fields.owned())
    });
    assert_eq!(pool.available_permits(), 0);
    assert_eq!(root.available_bytes(), 0);
    assert!(matches!(
        notice.effect(),
        ConversationUpdate::SignInRequired { .. }
    ));
    assert!(notice.turn_evidence().is_none());
    let clone = probe.track(|| notice.clone());
    let retained = probe.live();
    let peak = probe.peak();
    assert!(
        peak <= weight,
        "every requested effect allocation must fit the explicit layout allowance"
    );
    drop(notice);
    assert_eq!(probe.live(), retained);
    assert_eq!(pool.available_permits(), 0);
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(pool.available_permits(), weight);
    assert_eq!(root.available_bytes(), weight);
    eprintln!(
        "Sign-in allocation: encoded={} allowance={weight} retained={retained} peak={peak} final=0",
        encoded.len()
    );
}
