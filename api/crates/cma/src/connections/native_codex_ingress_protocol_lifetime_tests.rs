use super::*;
use std::{future::Future as _, pin::pin, task::Waker};

#[test]
fn cancelled_pending_read_returns_only_its_frame_storage() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    let mut input = NativeFrameStream::new(Chunked::new(b"synthetic\n", 2));
    input.reader.pending_at = Some(2);
    {
        let read = NativeFrameRead::admit(&scope, &policy).unwrap();
        let mut future = pin!(read.read(&mut input));
        assert!(future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        assert_eq!(
            root.available_bytes(),
            baseline - frame_plan().unwrap().requested_bytes()
        );
    }
    assert_eq!(root.available_bytes(), baseline);
    assert_eq!(input.reader.consumed, 2);
    assert_eq!(&input.reader.bytes[input.reader.consumed..], b"nthetic\n");
    assert!(!input.is_usable());
    let polls = input.reader.polls;
    assert_eq!(
        NativeFrameRead::admit(&scope, &policy)
            .unwrap()
            .read(&mut input)
            .now_or_never()
            .unwrap()
            .unwrap_err(),
        NativeFrameError::Unavailable
    );
    assert_eq!(input.reader.polls, polls);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn cancelled_before_first_poll_never_reads_the_source() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    let mut input = NativeFrameStream::new(Chunked::new(b"synthetic\n", 1));
    drop(
        NativeFrameRead::admit(&scope, &policy)
            .unwrap()
            .read(&mut input),
    );
    assert_eq!(input.reader.polls, 0);
    assert!(input.is_usable());
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn last_frame_clone_retains_the_attachment_after_callers_drop() {
    let root = IngressBudget::new(64 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        1,
        plan(4096),
        plan(16 * 1024 * 1024),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root, bounds).unwrap();
    let attachment = policy.admit_attachment().unwrap();
    let source = scope(attachment.policy());
    let frame = NativeFrameRead::admit(&source, attachment.policy())
        .unwrap()
        .read(&mut NativeFrameStream::new(b"{}\n".as_slice()))
        .now_or_never()
        .unwrap()
        .unwrap();
    let last = frame.clone();
    drop((frame, source, attachment));
    assert!(policy.admit_attachment().is_err());
    assert_eq!(last.expose(), "{}\n");
    drop(last);
    assert!(policy.admit_attachment().is_ok());
}

#[test]
fn cancellation_while_pending_before_consumption_keeps_stream_usable() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    let mut input = NativeFrameStream::new(Chunked::new(b"{}\n", 1));
    input.reader.pending_at = Some(0);
    {
        let read = NativeFrameRead::admit(&scope, &policy).unwrap();
        let mut future = pin!(read.read(&mut input));
        assert!(future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert_eq!(input.reader.consumed, 0);
    assert!(input.is_usable());
    assert_eq!(root.available_bytes(), baseline);
    input.reader.pending_at = None;
    let frame = NativeFrameRead::admit(&scope, &policy)
        .unwrap()
        .read(&mut input)
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(frame.expose(), "{}\n");
    assert!(input.is_usable());
}
