use super::tests::fixture;
use crate::ingress_budget::{AllocationPlan, IngressClass};
use serde_json::json;

#[path = "acp_ingress_instruction_source_tests.rs"]
mod instruction_source_tests;

#[test]
fn recovery_error_inspection_preserves_exact_json_matching() {
    use crate::hosted::acp_settings::legacy_instruction_resume_failed;
    let fixture = fixture();
    let expected = json!({"code": -32603, "message": "Internal error",
        "data": {"details": "Cannot read properties of undefined (reading 'replace')"}});
    let exact = expected.to_string();
    let cases = [
        exact.clone(),
        format!("  {exact}  "),
        exact.replace("Internal", "\\u0049nternal"),
        exact.replace("-32603", "-32603.0"),
        exact.replace("-32603", "-32602"),
        exact.replace("Internal error", "Other error"),
        exact.replace("'replace'", "'other'"),
        exact.replace("\"code\":", "\"extra\":null,\"code\":"),
        exact.replace("\"details\":", "\"extra\":null,\"details\":"),
        exact.replace("\"code\":", "\"code\":0,\"code\":"),
        exact.replace("\"code\":-32603", "\"code\":-32603,\"code\":0"),
        format!("{exact} false"),
        "null".into(),
        "[]".into(),
        "{}".into(),
        "{".into(),
    ];
    let baseline = fixture.root.available_bytes();
    for text in cases {
        let matches =
            serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|value| value == expected);
        let error = crate::AdapterError::Rejected(format!("cma/agent-instructions: {text}"));
        for pools in [None, fixture.bridge.json_pools.as_ref()] {
            assert_eq!(
                legacy_instruction_resume_failed(&error, pools).unwrap(),
                matches
            );
            assert_eq!(fixture.root.available_bytes(), baseline);
        }
    }
    for error in [
        crate::AdapterError::Unavailable(format!("cma/agent-instructions: {exact}")),
        crate::AdapterError::Rejected(format!("other-method: {exact}")),
    ] {
        assert!(
            !legacy_instruction_resume_failed(&error, fixture.bridge.json_pools.as_ref()).unwrap()
        );
    }
}

pub(super) fn error_inspection_allocations() {
    use crate::{
        allocation_probe::AllocationProbe, hosted::acp_settings::legacy_instruction_resume_failed,
    };
    let fixture = fixture();
    let json = json!({"code": -32603, "message": "Internal error",
        "data": {"details": "x".repeat(65_536)}})
    .to_string();
    let error = crate::AdapterError::Rejected(format!("cma/agent-instructions: {json}"));
    let baseline = fixture.root.available_bytes();
    let pools = fixture.bridge.json_pools.as_ref().unwrap();
    let admission = pools.admit(json.as_bytes()).unwrap();
    let bound = baseline - fixture.root.available_bytes();
    drop(admission);
    let mut probe = AllocationProbe::new();
    assert!(!probe
        .track(|| legacy_instruction_resume_failed(&error, Some(pools)))
        .unwrap());
    assert!(probe.peak() >= 65_536);
    assert!(probe.peak() <= bound);
    assert_eq!(probe.live(), 0);
    assert_eq!(fixture.root.available_bytes(), baseline);
    eprintln!(
        "recovery_error requested_peak={} retained={} admitted={bound}",
        probe.peak(),
        probe.live()
    );
    drop(probe);
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("error inspection pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let mut probe = AllocationProbe::new();
    assert!(!probe
        .track(|| legacy_instruction_resume_failed(&error, None))
        .unwrap());
    assert!(probe.peak() >= 65_536);
    eprintln!(
        "recovery_error legacy_unadmitted_requested_peak={}",
        probe.peak()
    );
    drop(probe);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| legacy_instruction_resume_failed(&error, Some(pools)));
    assert!(matches!(result, Err(crate::AdapterError::Unavailable(_))));
    assert!(
        probe.peak()
            <= "retained JSON storage capacity exhausted".len()
                + "pending Agent instructions recovery inspection at capacity".len()
    );
    eprintln!("recovery_error pressure_requested_peak={}", probe.peak());
    drop(result);
    assert_eq!(probe.live(), 0);
    drop(blocker);
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[test]
fn recovery_error_inspection_refuses_under_pressure_without_claiming_no_write() {
    use crate::hosted::acp_settings::legacy_instruction_resume_failed;
    let fixture = fixture();
    let error = crate::AdapterError::Rejected(format!(
        "cma/agent-instructions: {}",
        json!({"code": -32603, "message": "Internal error",
            "data": {"details": "Cannot read properties of undefined (reading 'replace')"}})
    ));
    assert!(legacy_instruction_resume_failed(&error, None).unwrap());
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let baseline = fixture.root.available_bytes();
        let available = fixture.policy.budget(class).available_bytes();
        let blocker = fixture
            .policy
            .reserve(
                class,
                AllocationPlan::new("error inspection pressure", &[available]).unwrap(),
            )
            .unwrap()
            .initialize(|| vec![0u8; available - 512]);
        let result = legacy_instruction_resume_failed(&error, fixture.bridge.json_pools.as_ref());
        assert!(
            matches!(result, Err(crate::AdapterError::Unavailable(_))),
            "instructions may already have changed; recovery cannot parse without admission: {result:?}"
        );
        drop(blocker);
        assert_eq!(fixture.root.available_bytes(), baseline);
    }
}

pub(super) fn prepared_json_allocations() {
    use crate::{allocation_probe::AllocationProbe, hosted::retained_json::JsonFrame};
    let fixture = fixture();
    let baseline = fixture.root.available_bytes();
    drop(
        fixture
            .policy
            .reserve(
                IngressClass::Data,
                AllocationPlan::new("warm", &[1]).unwrap(),
            )
            .unwrap(),
    );
    let params = json!({"sessionId":"s".repeat(1024),"cwd":"/synthetic",
        "additionalDirectories":["x".repeat(65_536)]});
    let mut probe = AllocationProbe::new();
    let legacy = probe.track(|| JsonFrame::legacy(serde_json::to_value(&params).unwrap()));
    assert!(probe.peak() >= 65_536);
    assert_eq!(fixture.root.available_bytes(), baseline);
    eprintln!("recovery_parameters legacy_requested_peak={}", probe.peak());
    drop(legacy);
    assert_eq!(probe.live(), 0);
    drop(probe);
    let mut probe = AllocationProbe::new();
    let frame = probe
        .track(|| JsonFrame::prepare(&params, Some(&fixture.policy)))
        .unwrap();
    let charged = baseline - fixture.root.available_bytes();
    assert_eq!(*frame, params);
    assert!(probe.peak() <= charged);
    eprintln!(
        "recovery_parameters requested_peak={} retained={} admitted={charged}",
        probe.peak(),
        probe.live()
    );
    let clone = frame.shared().unwrap();
    assert!(std::ptr::eq(&*frame, &*clone));
    drop(frame);
    assert_eq!(baseline - fixture.root.available_bytes(), charged);
    assert!(probe.live() > 0);
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(fixture.root.available_bytes(), baseline);
    drop(probe);
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("prepared JSON pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let mut probe = AllocationProbe::new();
    let refused = probe.track(|| JsonFrame::prepare(&params, Some(&fixture.policy)));
    assert!(matches!(refused, Err(crate::AdapterError::AtCapacity(_))));
    assert!(probe.peak() <= "ingress allocation capacity exhausted".len());
    drop((refused, blocker));
    assert_eq!(probe.live(), 0);
}

pub(super) async fn next_request(fixture: &super::tests::Fixture) -> serde_json::Value {
    loop {
        if let Some(frame) = fixture.writes.lock().unwrap().pop() {
            let super::Message::Binary(bytes) = &*frame else {
                panic!("expected conversation frame");
            };
            return serde_json::from_slice(&bytes[1..]).unwrap();
        }
        tokio::task::yield_now().await;
    }
}

pub(super) async fn reply(
    fixture: &super::tests::Fixture,
    request: &serde_json::Value,
    result: serde_json::Value,
) {
    let payload = serde_json::to_vec(&json!({"id":request["id"],"result":result})).unwrap();
    let frame = fixture
        .bridge
        .json_pools
        .as_ref()
        .unwrap()
        .decode(&payload)
        .unwrap();
    let waiting = fixture
        .bridge
        .pending
        .lock()
        .await
        .remove(request["id"].as_str().unwrap())
        .unwrap();
    waiting.answer.send(frame).unwrap();
}

#[tokio::test]
async fn recovery_pagination_keeps_parameter_custody_after_source_replies_drop() {
    for directories in [
        None,
        Some(serde_json::Value::Null),
        Some(json!(["/synthetic/extra"])),
    ] {
        let fixture = fixture();
        let baseline = fixture.root.available_bytes();
        let mut expected = json!({"sessionId":"conversation","cwd":"/synthetic"});
        if let Some(value) = directories {
            expected["additionalDirectories"] = value;
        }
        let peer = async {
            let first = next_request(&fixture).await;
            assert_eq!(first["method"], "session/list");
            assert_eq!(first["params"], json!({"cursor":null}));
            reply(&fixture, &first, json!({"sessions":[],"nextCursor":"next"})).await;
            let second = next_request(&fixture).await;
            assert_eq!(second["params"], json!({"cursor":"next"}));
            reply(&fixture, &second, json!({"sessions":[expected]})).await;
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                fixture.bridge.retained_instruction_session("conversation"),
                peer
            )
        })
        .await
        .unwrap();
        let result = result.unwrap();
        assert_eq!(*result, expected);
        let remaining = result.shared().unwrap();
        let held = fixture.root.available_bytes();
        assert!(held < baseline);
        drop(result);
        assert_eq!(fixture.root.available_bytes(), held);
        drop(remaining);
        assert_eq!(fixture.root.available_bytes(), baseline);
        assert!(fixture.writes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn repeated_cursor_stops_pagination_without_retaining_the_source_or_cursor() {
    let fixture = fixture();
    let baseline = fixture.root.available_bytes();
    let peer = async {
        for cursor in [serde_json::Value::Null, json!("same")] {
            let request = next_request(&fixture).await;
            assert_eq!(request["params"]["cursor"], cursor);
            reply(
                &fixture,
                &request,
                json!({"sessions":[],"nextCursor":"same"}),
            )
            .await;
        }
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            fixture.bridge.retained_instruction_session("conversation"),
            peer
        )
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(crate::AdapterError::Rejected(_))));
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[tokio::test]
async fn cancelling_pagination_releases_the_cursor_while_the_pending_call_remains_owned() {
    let fixture = fixture();
    let baseline = fixture.root.available_bytes();
    let before_cancel = {
        let peer = async {
            let request = next_request(&fixture).await;
            reply(
                &fixture,
                &request,
                json!({"sessions":[],"nextCursor":"x".repeat(65_536)}),
            )
            .await;
            let request = next_request(&fixture).await;
            assert_eq!(request["params"]["cursor"].as_str().unwrap().len(), 65_536);
            fixture.root.available_bytes()
        };
        let pending = fixture.bridge.retained_instruction_session("conversation");
        tokio::pin!(pending);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::select! {
                _ = &mut pending => panic!("second page must remain pending"),
                available = peer => available,
            }
        })
        .await
        .unwrap()
    };
    assert!(fixture.root.available_bytes() >= before_cancel + 65_536);
    assert!(fixture.root.available_bytes() < baseline);
    fixture.bridge.pending.lock().await.clear();
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[tokio::test]
async fn recovery_refuses_parameter_copy_when_the_reply_exhausts_data_capacity() {
    let fixture = fixture();
    let baseline = fixture.root.available_bytes();
    let peer = async {
        let frame = loop {
            if let Some(frame) = fixture.writes.lock().unwrap().pop() {
                break frame;
            }
            tokio::task::yield_now().await;
        };
        let super::Message::Binary(bytes) = &*frame else {
            panic!("expected conversation frame");
        };
        let request: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
        assert_eq!(request["method"], "session/list");
        let payload = serde_json::to_vec(&json!({"id":request["id"],"result":{"sessions":[{
            "sessionId":"conversation", "cwd":"/synthetic",
            "additionalDirectories":["x".repeat(65_536)]
        }]}}))
        .unwrap();
        let response = fixture
            .bridge
            .json_pools
            .as_ref()
            .unwrap()
            .decode(&payload)
            .unwrap();
        let waiting = fixture
            .bridge
            .pending
            .lock()
            .await
            .remove(request["id"].as_str().unwrap())
            .unwrap();
        let available = fixture.policy.budget(IngressClass::Data).available_bytes();
        let blocker = fixture
            .policy
            .reserve(
                IngressClass::Data,
                AllocationPlan::new("recovery parameter pressure", &[available]).unwrap(),
            )
            .unwrap()
            .initialize(|| vec![0u8; available - 512]);
        // Deliver the already-owned reply after saturation, at the real RPC
        // channel boundary, so any later copied parameters need new admission.
        waiting.answer.send(response).unwrap();
        blocker
    };
    let (result, blocker) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            fixture.bridge.retained_instruction_session("conversation"),
            peer
        )
    })
    .await
    .unwrap();
    assert!(
        matches!(result, Err(crate::AdapterError::AtCapacity(_))),
        "recovery parameters were copied without admission"
    );
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(fixture.bridge.pending.lock().await.is_empty());
    drop((result, blocker));
    assert_eq!(fixture.root.available_bytes(), baseline);
}
