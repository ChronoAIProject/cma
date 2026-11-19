use super::tests::fixture;
use crate::hosted::update_relay::UpdateRelay;
use crate::ingress_budget::{
    sizing, AdmittedConversationSendOutcome, AllocationPlan, IngressClass, IngressWorkScope,
};
use crate::{AdapterError, ConversationInput};

#[tokio::test(start_paused = true)]
async fn rpc_timeout_error_keeps_custody_after_bridge_and_operation_drop() {
    let fixture = fixture();
    let root = fixture.root.clone();
    let scope = IngressWorkScope::new(
        fixture
            .policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "RPC timeout fixture",
                    &[sizing::shared_owner_bytes::<()>()],
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let error = scope
        .initialize_owned(|| async {
            fixture
                .bridge
                .call_within(
                    "synthetic/method",
                    serde_json::Value::Null,
                    std::time::Duration::from_secs(1),
                )
                .await
        })
        .resolve_admitted_result(&fixture.policy, IngressClass::Data)
        .await
        .unwrap_err();
    assert!(
        matches!(error.as_adapter(), Some(AdapterError::Unavailable(detail))
        if detail == "synthetic/method was never answered")
    );
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
    let cloned = error.clone();
    drop((fixture, scope, error));
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        cloned.requested_bytes()
    );
    drop(cloned);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_writer_returns_owned_bridge_failure_without_runtime_effects() {
    use super::super::channel::{Admitted, Mode};
    use std::sync::{atomic::Ordering, Arc};
    let root;
    let outcome = {
        let fixture = fixture();
        root = fixture.root.clone();
        fixture.bridge.recovering_context.store(1, Ordering::SeqCst);
        let bridge = Arc::new(fixture.bridge);
        let (updates, _capture) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
        let sender = Admitted::input(&bridge, &updates).await.unwrap();
        let input = IngressWorkScope::new(
            fixture
                .policy
                .admit(
                    IngressClass::Control,
                    AllocationPlan::checked("writer error fixture", &[256]).unwrap(),
                )
                .unwrap(),
        )
        .initialize_owned(|| ConversationInput::Cancel);
        let result = sender.send_classified(input).await;
        tokio::task::yield_now().await;
        assert!(sender.is_closed());
        assert!(fixture.writes.lock().unwrap().is_empty());
        assert!(
            matches!(&result, AdmittedConversationSendOutcome::Ambiguous(error)
            if matches!(error.as_adapter(), Some(AdapterError::OperationIdentityConflict(detail))
                if detail == "admitted context recovery must complete before other input"))
        );
        result
    };
    let AdmittedConversationSendOutcome::Ambiguous(error) = outcome else {
        unreachable!()
    };
    let clone = error.clone();
    drop(error);
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        clone.requested_bytes()
    );
    drop(clone);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn missing_session_diagnostic_refuses_text_before_allocation_under_pressure() {
    use crate::allocation_probe::AllocationProbe;
    use futures_util::FutureExt;
    let fixture = fixture();
    // This measures diagnostic text; cold Bridge mutex storage has a separate
    // lifetime and must be covered by the attachment's fixed allocation plan.
    assert!(!fixture.bridge.matches_session("synthetic").await);
    let root = fixture.root.clone();
    let scope = IngressWorkScope::new(
        fixture
            .policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "missing session fixture",
                    &[sizing::shared_owner_bytes::<()>()],
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let mut probe = AllocationProbe::new();
    let error = scope
        .initialize_owned(|| async {
            let available = fixture.policy.budget(IngressClass::Data).available_bytes();
            let _pressure = fixture
                .policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("diagnostic pressure", &[available]).unwrap(),
                )
                .unwrap()
                .initialize(|| vec![0u8; available - 1024]);
            probe.track(|| {
                fixture
                    .bridge
                    .matches_active_session("synthetic")
                    .now_or_never()
                    .unwrap()
            })
        })
        .resolve_admitted_result(&fixture.policy, IngressClass::Data)
        .await
        .unwrap_err();
    assert_eq!(
        probe.allocated(),
        0,
        "missing-session error allocated without admission"
    );
    assert!(
        matches!(error.as_adapter(), Some(AdapterError::Unavailable(detail)) if detail.is_empty())
    );
    drop((scope, fixture));
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        error.requested_bytes()
    );
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn reader_diagnostic_scope_refuses_before_unowned_error_construction() {
    use super::super::{ingress::IncomingFrame, reader, Message, LANE_RETAINED_OUTPUT};
    use crate::allocation_probe::AllocationProbe;
    use futures_util::FutureExt;
    let fixture = fixture();
    let root = fixture.root.clone();
    let (updates, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let make_frame = || {
        IncomingFrame::Admitted(
            fixture
                .policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "reader error frame",
                        &[sizing::shared_owner_bytes::<Message>(), 2],
                    )
                    .unwrap(),
                )
                .unwrap()
                .initialize(|| Message::Binary(vec![LANE_RETAINED_OUTPUT, 0])),
        )
    };
    assert!(!reader::dispatch(&fixture.bridge, Ok(make_frame()), &updates).await);
    let frame = make_frame();
    let available = fixture
        .policy
        .budget(IngressClass::Control)
        .available_bytes();
    let pressure = fixture
        .policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("reader diagnostic pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let mut probe = AllocationProbe::new();
    assert!(
        !probe.track(|| reader::dispatch(&fixture.bridge, Ok(frame), &updates)
            .now_or_never()
            .unwrap())
    );
    assert_eq!(
        probe.allocated(),
        0,
        "reader allocated an error after diagnostic refusal"
    );
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(captured.take().unwrap().is_none());
    drop((pressure, updates, captured, fixture));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn independent_ack_diagnostics_keep_control_progress_and_refuse_before_send() {
    use super::super::{ingress::Ack, reader};
    use crate::allocation_probe::AllocationProbe;
    use crate::{ConversationSettingsSupport, RuntimeOutputSource};
    use futures_util::FutureExt;
    let fixture = fixture();
    fixture.activate().await;
    let root = fixture.root.clone();
    let source = fixture
        .policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked(
                "ACK diagnostic source",
                &[
                    sizing::shared_owner_bytes::<RuntimeOutputSource>(),
                    36 + 12 + 64,
                ],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| RuntimeOutputSource {
            stream_id: fixture.stream.to_string(),
            cursor: 7,
            native_conversation_id: "conversation".into(),
            payload_sha256: "0".repeat(64),
        });
    let observation = fixture.bridge.delivery_observation().await;
    let progress = observation.admitted_progress().unwrap();
    let state = fixture.bridge.output_ack_state().await.unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let data = fixture
        .policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("ACK diagnostic data pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    assert!(
        reader::acknowledge(&fixture.bridge, Ack::Admitted(source.clone()), Some(&state)).await
    );
    assert_eq!(progress.latest(), 7);
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
    let available = fixture
        .policy
        .budget(IngressClass::Control)
        .available_bytes();
    let control = fixture
        .policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("ACK diagnostic control pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let mut probe = AllocationProbe::new();
    assert!(!probe.track(|| reader::acknowledge(
        &fixture.bridge,
        Ack::Admitted(source.clone()),
        Some(&state)
    )
    .now_or_never()
    .unwrap()));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(progress.latest(), 7);
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
    drop((data, control, source, state, observation, progress, fixture));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_socket_refusal_needs_no_unowned_error_box() {
    use super::super::Message;
    use crate::allocation_probe::AllocationProbe;
    use futures_util::{FutureExt, SinkExt};
    let fixture = fixture();
    let mut sink = fixture.bridge.sink.lock().await;
    let mut probe = AllocationProbe::new();
    let failures: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| {
            sink.send(Message::Binary(Vec::new()))
                .now_or_never()
                .unwrap()
                .unwrap_err()
        })
    });
    assert_eq!(
        probe.allocated(),
        0,
        "missing write scope must refuse without error allocation"
    );
    assert!(failures.iter().all(|failure| matches!(failure,
        tokio_tungstenite::tungstenite::Error::Io(error) if error.kind() == std::io::ErrorKind::Other)));
    assert!(fixture.writes.lock().unwrap().is_empty());
    drop(failures);
    assert_eq!(probe.live(), 0);
}
