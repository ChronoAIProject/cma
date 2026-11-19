//! Copy-phase allocation evidence; codec construction/read peaks are separate.

use bytes::Bytes;
use tokio_tungstenite::tungstenite as ws024;
use tokio_tungstenite_029::tungstenite as ws029;

use crate::{
    allocation_probe::AllocationProbe,
    ingress_budget::{
        sizing::shared_owner_bytes, AllocationPlan, ClassBounds, IngressBudget, IngressClass,
        IngressOwnershipPolicy, SharedOwned,
    },
    owned_websocket::{
        copy_conversation_024, copy_conversation_029, copy_desktop_024, copy_desktop_029,
    },
    AdapterError, DesktopFrame,
};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let floor = AllocationPlan::new("fixture_progress", &[1024 * 1024]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn source<T>(
    policy: &IngressOwnershipPolicy,
    bytes: usize,
    initialize: impl FnOnce() -> T,
) -> SharedOwned<T> {
    policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("fixture_source", &[bytes, shared_owner_bytes::<T>()]).unwrap(),
        )
        .unwrap()
        .initialize(initialize)
}

#[test]
fn maximum_desktop_and_conversation_copies_fit_their_destination_plans() {
    for length in [0, 1, 4 * 1024 * 1024 + 25, 8 * 1024 * 1024] {
        let (root, policy) = policy();
        let input = source(&policy, length, || {
            ws024::Message::Binary(vec![0x5a; length])
        });
        let before = root.available_bytes();
        let mut probe = AllocationProbe::new();
        let copied = probe.track(|| copy_desktop_024(&policy, &input).unwrap());
        let charged = before - root.available_bytes();
        assert_eq!(charged, copied.requested_bytes());
        assert!(probe.peak() <= charged);
        let DesktopFrame::Binary(bytes) = &*copied else {
            panic!("binary kind must survive the copy")
        };
        assert_eq!(bytes.len(), length);
        assert_eq!(bytes.capacity(), length);
        if length != 0 {
            let ws024::Message::Binary(source) = &*input else {
                unreachable!()
            };
            assert_ne!(bytes.as_ptr(), source.as_ptr());
        }
        let clone = copied.clone();
        assert!(copied.ptr_eq(&clone));
        drop(input);
        drop(copied);
        assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
        assert!(probe.live() >= length);
        drop(clone);
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }

    let (root, policy) = policy();
    let length = 4 * 1024 * 1024 + 25;
    let input = source(&policy, length, || {
        ws029::Message::Binary(vec![0x5a; length].into())
    });
    let before = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let copied = probe.track(|| copy_conversation_029(&policy, &input).unwrap());
    assert_eq!(copied.len(), length);
    assert!(probe.peak() <= before - root.available_bytes());
    drop(input);
    assert_eq!(copied.len(), length);
    drop(copied);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

struct SourceBacking(SharedOwned<Vec<u8>>);

impl AsRef<[u8]> for SourceBacking {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[test]
fn maximum_current_desktop_and_legacy_conversation_copies_fit_their_plans() {
    let (root, policy) = policy();
    {
        let length = 8 * 1024 * 1024;
        let input = source(&policy, length, || {
            ws029::Message::Binary(vec![0x5a; length].into())
        });
        let mut probe = AllocationProbe::new();
        let copied = probe.track(|| copy_desktop_029(&policy, &input).unwrap());
        assert!(probe.peak() <= copied.requested_bytes());
        let DesktopFrame::Binary(data) = &*copied else {
            panic!("binary kind must survive the copy")
        };
        assert_eq!(data.len(), length);
        drop(input);
        assert_eq!(
            root.capacity_bytes() - root.available_bytes(),
            copied.requested_bytes()
        );
        drop(copied);
        assert_eq!(probe.live(), 0);
    }
    {
        let length = 4 * 1024 * 1024 + 25;
        let input = source(&policy, length, || ws024::Message::Text("x".repeat(length)));
        let mut probe = AllocationProbe::new();
        let copied = probe.track(|| copy_conversation_024(&policy, &input).unwrap());
        assert_eq!(copied.len(), length);
        assert!(probe.peak() <= copied.requested_bytes());
        drop(input);
        assert_eq!(
            root.capacity_bytes() - root.available_bytes(),
            copied.requested_bytes()
        );
        drop(copied);
        assert_eq!(probe.live(), 0);
    }
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn a_short_shared_source_slice_keeps_its_backing_until_the_source_drops() {
    let (root, policy) = policy();
    let length = 256 * 1024;
    // bytes::Owned<T> stores a refcount, pointer, length and T. Include its
    // allocation in the source's plan before Bytes::from_owner constructs it.
    let owner_layout = size_of::<SourceBacking>() + 3 * size_of::<usize>();
    let backing = source(&policy, length + owner_layout, || vec![0x5a; length]);
    let original = Bytes::from_owner(SourceBacking(backing));
    let message = ws029::Message::Binary(original.slice(7..23));
    let source_charge = root.capacity_bytes() - root.available_bytes();
    let mut probe = AllocationProbe::new();
    let copied = probe.track(|| copy_desktop_029(&policy, &message).unwrap());
    let DesktopFrame::Binary(data) = &*copied else {
        panic!("binary kind must survive the copy")
    };
    assert_eq!(data, &[0x5a; 16]);
    let ws029::Message::Binary(source) = &message else {
        unreachable!()
    };
    assert_ne!(data.as_ptr(), source.as_ptr());
    assert!(probe.peak() <= copied.requested_bytes());
    drop(original);
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        source_charge + copied.requested_bytes()
    );
    drop(message);
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        copied.requested_bytes()
    );
    drop(copied);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn capacity_refusal_precedes_the_large_payload_copy_and_leaves_source_intact() {
    let (root, policy) = policy();
    let input = source(&policy, 64 * 1024, || {
        ws029::Message::Binary(vec![0x5a; 64 * 1024].into())
    });
    let data = policy.budget(IngressClass::Data);
    let full = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("fixture_saturation", &[data.available_bytes()]).unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| copy_desktop_029(&policy, &input));
    assert!(matches!(result, Err(AdapterError::AtCapacity(_))));
    // The existing common refusal allocates a bounded diagnostic String.
    // This assertion distinguishes that cost from a copied 64 KiB payload.
    assert!(probe.peak() < 1024);
    assert_eq!(input.len(), 64 * 1024);
    drop(result);
    assert_eq!(probe.live(), 0);
    drop(full);
    let copied = copy_desktop_029(&policy, &input).unwrap();
    drop(input);
    drop(copied);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn both_codec_versions_preserve_text_and_control_semantics() {
    let (root, policy) = policy();
    let inputs = [
        ws024::Message::Text("synthetic text 🦀".into()),
        ws024::Message::Binary(vec![1, 2, 3]),
        ws024::Message::Ping(vec![4, 5]),
        ws024::Message::Pong(vec![6, 7]),
        ws024::Message::Close(None),
        ws024::Message::Close(Some(ws024::protocol::CloseFrame {
            code: ws024::protocol::frame::coding::CloseCode::Normal,
            reason: "synthetic close".into(),
        })),
    ];
    for input in inputs {
        let converted = match &input {
            ws024::Message::Text(value) => ws029::Message::text(value.clone()),
            ws024::Message::Binary(value) => ws029::Message::binary(value.clone()),
            ws024::Message::Ping(value) => ws029::Message::Ping(value.clone().into()),
            ws024::Message::Pong(value) => ws029::Message::Pong(value.clone().into()),
            ws024::Message::Close(close) => {
                ws029::Message::Close(close.as_ref().map(|close| ws029::protocol::CloseFrame {
                    code: u16::from(close.code).into(),
                    reason: close.reason.to_string().into(),
                }))
            }
            ws024::Message::Frame(_) => unreachable!(),
        };
        let a = copy_conversation_024(&policy, &input).unwrap();
        let b = copy_conversation_029(&policy, &converted).unwrap();
        assert_eq!(*a, input);
        assert_eq!(*b, input);
        let desktop_a = copy_desktop_024(&policy, &input).unwrap();
        let desktop_b = copy_desktop_029(&policy, &converted).unwrap();
        assert_eq!(*desktop_a, *desktop_b);
    }
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn negative_control_unadmitted_copy_allocates_while_data_capacity_is_full() {
    let (root, policy) = policy();
    let input = source(&policy, 64 * 1024, || {
        ws024::Message::Binary(vec![0x5a; 64 * 1024])
    });
    let data = policy.budget(IngressClass::Data);
    let full = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("fixture_saturation", &[data.available_bytes()]).unwrap(),
        )
        .unwrap();
    let available = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let copy = probe.track(|| (*input).clone());
    assert_eq!(data.available_bytes(), 0);
    assert_eq!(root.available_bytes(), available);
    assert!(probe.peak() >= 64 * 1024);
    drop(copy);
    assert_eq!(probe.live(), 0);
    drop(full);
    drop(input);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn raw_frames_are_refused_without_discarding_their_metadata() {
    let (root, policy) = policy();
    let a = ws024::Message::Frame(ws024::protocol::frame::Frame::message(
        vec![1, 2, 3],
        ws024::protocol::frame::coding::OpCode::Data(
            ws024::protocol::frame::coding::Data::Continue,
        ),
        false,
    ));
    let b = ws029::Message::Frame(ws029::protocol::frame::Frame::message(
        Bytes::from_static(&[1, 2, 3]),
        ws029::protocol::frame::coding::OpCode::Data(
            ws029::protocol::frame::coding::Data::Continue,
        ),
        false,
    ));
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        assert!(matches!(
            copy_desktop_024(&policy, &a),
            Err(AdapterError::Unsupported(_))
        ));
        assert!(matches!(
            copy_desktop_029(&policy, &b),
            Err(AdapterError::Unsupported(_))
        ));
        assert!(matches!(
            copy_conversation_024(&policy, &a),
            Err(AdapterError::Unsupported(_))
        ));
        assert!(matches!(
            copy_conversation_029(&policy, &b),
            Err(AdapterError::Unsupported(_))
        ));
    });
    assert_eq!(probe.peak(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn control_copies_progress_without_borrowing_saturated_data_capacity() {
    let (root, policy) = policy();
    let data = policy.budget(IngressClass::Data);
    let full = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("fixture_saturation", &[data.available_bytes()]).unwrap(),
        )
        .unwrap();
    let control = policy.budget(IngressClass::Control);
    let before = control.available_bytes();
    let a = ws024::Message::Ping(vec![0; 125]);
    let b = ws029::Message::Pong(vec![0; 125].into());
    let close = ws029::Message::Close(Some(ws029::protocol::CloseFrame {
        code: ws029::protocol::frame::coding::CloseCode::Normal,
        reason: "x".repeat(123).into(),
    }));
    let legacy_close = ws024::Message::Close(Some(ws024::protocol::CloseFrame {
        code: ws024::protocol::frame::coding::CloseCode::Normal,
        reason: "x".repeat(123).into(),
    }));
    let held = [
        copy_conversation_024(&policy, &a).unwrap(),
        copy_conversation_029(&policy, &b).unwrap(),
        copy_conversation_024(&policy, &legacy_close).unwrap(),
        copy_conversation_029(&policy, &close).unwrap(),
    ];
    let desktop = [
        copy_desktop_024(&policy, &a).unwrap(),
        copy_desktop_029(&policy, &b).unwrap(),
        copy_desktop_024(&policy, &legacy_close).unwrap(),
        copy_desktop_029(&policy, &close).unwrap(),
    ];
    let charged: usize = held.iter().map(SharedOwned::requested_bytes).sum::<usize>()
        + desktop
            .iter()
            .map(SharedOwned::requested_bytes)
            .sum::<usize>();
    assert_eq!(before - control.available_bytes(), charged);
    assert_eq!(data.available_bytes(), 0);
    assert!(matches!(
        copy_desktop_029(&policy, &ws029::Message::binary(vec![1])),
        Err(AdapterError::AtCapacity(_))
    ));
    drop(held);
    drop(desktop);
    assert_eq!(control.available_bytes(), before);
    drop(full);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn oversized_controls_cannot_consume_the_progress_allowance() {
    let (root, policy) = policy();
    for input in [
        ws024::Message::Ping(vec![0; 126]),
        ws024::Message::Pong(vec![0; 126]),
        ws024::Message::Close(Some(ws024::protocol::CloseFrame {
            code: ws024::protocol::frame::coding::CloseCode::Normal,
            reason: "x".repeat(124).into(),
        })),
    ] {
        let converted = match &input {
            ws024::Message::Ping(value) => ws029::Message::Ping(value.clone().into()),
            ws024::Message::Pong(value) => ws029::Message::Pong(value.clone().into()),
            ws024::Message::Close(_) => ws029::Message::Close(Some(ws029::protocol::CloseFrame {
                code: ws029::protocol::frame::coding::CloseCode::Normal,
                reason: "x".repeat(124).into(),
            })),
            _ => unreachable!(),
        };
        let before = root.available_bytes();
        assert!(matches!(
            copy_desktop_024(&policy, &input),
            Err(AdapterError::Rejected(_))
        ));
        assert!(matches!(
            copy_desktop_029(&policy, &converted),
            Err(AdapterError::Rejected(_))
        ));
        assert!(matches!(
            copy_conversation_024(&policy, &input),
            Err(AdapterError::Rejected(_))
        ));
        assert!(matches!(
            copy_conversation_029(&policy, &converted),
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(root.available_bytes(), before);
    }
}
