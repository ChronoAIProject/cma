use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::hosted::acp::ingress::fixture::{attached_fixture, Fixture};
use crate::ingress_budget::{
    sizing, AllocationPlan, IngressBudget, IngressClass, IngressOwnershipPolicy, SharedOwned,
};
use futures_util::FutureExt;
use serde_json::json;
use std::time::Duration;

const ATTACHMENTS: usize = 200;
const ACK_CAPACITY: usize = 128;

fn pressure(
    policy: &IngressOwnershipPolicy,
    class: IngressClass,
    bytes: usize,
) -> SharedOwned<Vec<u8>> {
    let payload = bytes
        .checked_sub(sizing::shared_owner_bytes::<Vec<u8>>())
        .unwrap();
    policy
        .admit(
            class,
            AllocationPlan::checked("other concurrent work", &[bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0xa5; payload])
}

fn retained_frame(
    fixture: &Fixture,
    value: &serde_json::Value,
    cursor: u64,
) -> SharedOwned<Message> {
    // The synthetic peer's borrowed wire is harness storage. Admit before the
    // receiver copies it into a frame; this does not qualify a real codec.
    let wire = fixture.retained(value, cursor);
    fixture
        .policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "synthetic incoming frame",
                &[1 + wire.len(), sizing::shared_owner_bytes::<Message>()],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| {
            let mut bytes = Vec::with_capacity(1 + wire.len());
            bytes.push(LANE_RETAINED_OUTPUT);
            bytes.extend_from_slice(&wire);
            Message::Binary(bytes)
        })
}

#[tokio::test]
async fn all_attachment_readers_and_full_ack_queues_progress_under_data_pressure() {
    tokio::time::timeout(Duration::from_secs(30), exercise_cohort(false))
        .await
        .expect("cohort setup and teardown must not stall");
}

#[tokio::test]
async fn cancelling_all_pending_readers_and_acks_preserves_retry_positions() {
    tokio::time::timeout(Duration::from_secs(30), exercise_cohort(true))
        .await
        .expect("cohort setup and teardown must not stall");
}

async fn exercise_cohort(cancel: bool) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let empty = AllocationPlan::checked("no external producers", &[0]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        IngressOwnershipPolicy::conversation_bounds(
            root.capacity_bytes(),
            ATTACHMENTS,
            empty,
            empty,
        )
        .unwrap(),
    )
    .unwrap();
    let mut fixtures: Vec<_> = (0..ATTACHMENTS)
        .map(|_| attached_fixture(&root, &policy))
        .collect();
    let mut probe = AllocationProbe::new();
    probe.track(|| assert!(policy.admit_attachment().is_err()));
    assert_eq!(
        probe.allocated(),
        0,
        "N+1 must refuse before transport construction"
    );
    drop(probe);

    let session = "s".repeat(1024);
    let mut relays = Vec::with_capacity(ATTACHMENTS);
    let mut captures = Vec::with_capacity(ATTACHMENTS);
    let mut deliveries = Vec::with_capacity(ATTACHMENTS);
    let mut frames = Vec::with_capacity(ATTACHMENTS);
    let mut acks = Vec::with_capacity(ATTACHMENTS);
    let mut states = Vec::with_capacity(ATTACHMENTS);
    let mut progress = Vec::with_capacity(ATTACHMENTS);
    for fixture in &mut fixtures {
        fixture.activate_as(&session).await;
        let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
        let input = retained_frame(
            fixture,
            &json!({"method":"session/update", "params":{
                "sessionId":session, "update":{"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text", "text":"durable input"}}
            }}),
            1,
        );
        assert!(dispatch(&fixture.bridge, Ok(IncomingFrame::Admitted(input)), &relay).await);
        let delivery = captured.take().unwrap().unwrap();
        let receipt = delivery.delivery().receipt.unwrap();
        assert_eq!(receipt.source.native_conversation_id.len(), 1024);
        assert_eq!(receipt.source.stream_id.len(), 36);
        assert_eq!(receipt.source.payload_sha256.len(), 64);
        assert!(
            fixture.writes.lock().unwrap().is_empty(),
            "no ACK before durable acceptance"
        );
        // Synthetic durable acceptance: only now may this exact receipt enter
        // the real queue. Repeated receipt retries can fill all 128 slots while
        // the independent ACK task retains one already-dequeued source.
        receipt.acknowledge_admitted().unwrap();
        acks.push(fixture.acknowledgements.recv().await.unwrap());
        for _ in 0..ACK_CAPACITY {
            receipt.acknowledge_admitted().unwrap();
        }
        let mut probe = AllocationProbe::new();
        probe.track(|| {
            assert!(matches!(
                receipt.acknowledge_admitted(),
                Err(crate::runtime_output::AdmittedAckError::Full)
            ))
        });
        assert_eq!(
            probe.allocated(),
            0,
            "full ACK queue must refuse before copying"
        );
        drop(probe);
        states.push(fixture.bridge.output_ack_state().await);
        progress.push(fixture.bridge.output_observation().await.unwrap());
        frames.push(retained_frame(
            fixture,
            &json!({"id":"overflow",
                "method":"session/request_permission", "params":{
                    "sessionId":session, "toolCall":{"toolCallId":"tool", "title":"new"},
                    "options":[{"optionId":"yes", "name":"Allow", "kind":"allow_once"}]
            }}),
            2,
        ));
        relays.push(relay);
        captures.push(captured);
        deliveries.push(delivery);
    }

    let mut permission_gates = Vec::with_capacity(ATTACHMENTS);
    let mut sink_gates = Vec::with_capacity(ATTACHMENTS);
    for fixture in &fixtures {
        permission_gates.push(
            fixture
                .bridge
                .permission_state
                .as_ref()
                .unwrap()
                .lock()
                .await,
        );
        sink_gates.push(fixture.bridge.sink.lock().await);
    }
    let mut reads: Vec<_> = fixtures
        .iter()
        .zip(&relays)
        .zip(&frames)
        .map(|((fixture, relay), frame)| {
            Box::pin(dispatch(
                &fixture.bridge,
                Ok(IncomingFrame::Admitted(frame.clone())),
                relay,
            ))
        })
        .collect();
    let mut acknowledgements: Vec<_> = fixtures
        .iter()
        .zip(&states)
        .zip(acks)
        .map(|((fixture, state), ack)| {
            Box::pin(acknowledge(
                &fixture.bridge,
                Ack::Admitted(ack),
                state.as_ref(),
            ))
        })
        .collect();
    for (read, ack) in reads.iter_mut().zip(&mut acknowledgements) {
        // Manual staging must reach the held mutex. A cooperative budget yield
        // is also Pending, but does not establish the required overlap.
        assert!(
            tokio::task::unconstrained(read.as_mut())
                .now_or_never()
                .is_none(),
            "reader must reach permission lock"
        );
        assert!(
            tokio::task::unconstrained(ack.as_mut())
                .now_or_never()
                .is_none(),
            "ACK must reach sink lock"
        );
    }

    let control = policy.budget(IngressClass::Control);
    let data = policy.budget(IngressClass::Data);
    let reply = crate::hosted::permission_state::reply_plan()
        .unwrap()
        .requested_bytes()
        + 4097
        + sizing::shared_owner_bytes::<Message>();
    // Real queues, dequeued sources, ACK writes and both diagnostic scopes are
    // already alive. Materialize every remaining Control byte except the
    // declared reader reply allowance. This pressure stands for other work,
    // including input replies; it is not evidence for missing source producers.
    let other_bytes = control
        .available_bytes()
        .checked_sub(ATTACHMENTS * reply)
        .unwrap();
    let other_work = pressure(&policy, IngressClass::Control, other_bytes);
    let data_bytes = data.available_bytes();
    let held_data = pressure(&policy, IngressClass::Data, data_bytes);
    eprintln!(
        "common cohort: cancel={cancel}, attachments={ATTACHMENTS}, queued_acks={}, \
         data_bound={}, control_bound={}, transformation_bound={}, metadata={}, \
         materialized_data={data_bytes}, materialized_other_control={other_bytes}, \
         remaining_reader_replies={}",
        ATTACHMENTS * ACK_CAPACITY,
        policy.bounds().data_bytes(),
        policy.bounds().control_bytes(),
        policy.bounds().transformation_bytes(),
        policy.bounds().metadata_bytes(),
        ATTACHMENTS * reply,
    );
    assert_eq!(control.available_bytes(), ATTACHMENTS * reply);
    assert_eq!(data.available_bytes(), 0);
    assert_eq!(
        held_data.len() + sizing::shared_owner_bytes::<Vec<u8>>(),
        data_bytes
    );
    assert_eq!(
        other_work.len() + sizing::shared_owner_bytes::<Vec<u8>>(),
        other_bytes
    );
    drop(permission_gates);
    // Each remember must fail while Data remains full, then retain its error,
    // refusal JSON and encoded write at the sink. No earlier completed reader
    // may free Data for a later permission attempt.
    for read in &mut reads {
        assert!(
            tokio::task::unconstrained(read.as_mut())
                .now_or_never()
                .is_none(),
            "permission refusal must await sink"
        );
        assert_eq!(data.available_bytes(), 0);
    }
    assert!(progress.iter().all(|watch| watch.latest() == 0));
    assert!(fixtures
        .iter()
        .all(|fixture| fixture.writes.lock().unwrap().is_empty()));
    if cancel {
        drop(reads);
        drop(acknowledgements);
        drop(sink_gates);
    } else {
        drop(sink_gates);
        tokio::time::timeout(Duration::from_secs(1), async {
            let (read_results, ack_results) = tokio::join!(
                futures_util::future::join_all(reads),
                futures_util::future::join_all(acknowledgements),
            );
            assert!(read_results.into_iter().all(|accepted| !accepted));
            assert!(ack_results.into_iter().all(|sent| sent));
        })
        .await
        .expect("all ACKs and permission refusals must meet the existing deadline");
    }
    for ((fixture, captured), watch) in fixtures.iter().zip(&captures).zip(&progress) {
        assert!(
            captured.take().unwrap().is_none(),
            "refusal cannot publish a permission or receipt"
        );
        assert!(fixture.bridge.blocked.lock().await.is_empty());
        assert!(fixture.bridge.answered.lock().await.is_empty());
        assert_eq!(watch.latest(), u64::from(!cancel));
        let writes = fixture.writes.lock().unwrap();
        assert_eq!(writes.len(), if cancel { 0 } else { 2 });
        if !cancel {
            assert!(writes
                .iter()
                .any(|message| matches!(&**message, Message::Binary(bytes)
                if bytes.len() == 25 && bytes[0] == 5
                    && &bytes[1..17] == fixture.stream.as_bytes()
                    && bytes[17..25] == 1u64.to_be_bytes())));
            let refusal = writes
                .iter()
                .find_map(|message| match &**message {
                    Message::Binary(bytes) if bytes[0] == LANE_CONVERSATION => {
                        Some(serde_json::from_slice::<serde_json::Value>(&bytes[1..]).unwrap())
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(refusal["id"], "overflow");
            assert_eq!(refusal["error"]["code"], -32002);
        }
    }
    drop((held_data, other_work));
    for (((fixture, frame), relay), captured) in
        fixtures.iter_mut().zip(frames).zip(&relays).zip(&captures)
    {
        if cancel {
            let ack = fixture.acknowledgements.recv().await.unwrap();
            let state = fixture.bridge.output_ack_state().await;
            assert!(acknowledge(&fixture.bridge, Ack::Admitted(ack), state.as_ref()).await);
        }
        // The exact permission cursor remains retryable. With capacity back it
        // succeeds, proving refusal/cancellation did not accept it or ACK it.
        assert!(dispatch(&fixture.bridge, Ok(IncomingFrame::Admitted(frame)), relay).await);
        let retried = captured.take().unwrap().unwrap();
        let Some(ConversationUpdate::WithoutPromptEvidence(activity)) = retried.delivery().effect
        else {
            panic!("passive permission wrapper missing");
        };
        assert!(matches!(activity.effect(), ConversationUpdate::Asked(_)));
        assert_eq!(retried.delivery().receipt.unwrap().source.cursor, 2);
    }
    assert!(progress.iter().all(|watch| watch.latest() == 1));
    let escaped_write = fixtures[0].writes.clone();
    drop((
        fixtures, deliveries, relays, captures, progress, states, control, data,
    ));
    let remaining: Vec<_> = (0..ATTACHMENTS - 1)
        .map(|_| policy.admit_attachment().unwrap())
        .collect();
    assert!(
        policy.admit_attachment().is_err(),
        "retained write still owns one attachment"
    );
    drop(escaped_write);
    let last = policy.admit_attachment().unwrap();
    drop((last, remaining, policy));
    root.assert_idle();
}
