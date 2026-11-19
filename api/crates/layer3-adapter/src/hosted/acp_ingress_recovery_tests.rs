use super::tests::fixture;
use crate::ingress_budget::{AllocationPlan, IngressClass};
use serde_json::json;

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

async fn next_request(fixture: &super::tests::Fixture) -> serde_json::Value {
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

async fn reply(
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
