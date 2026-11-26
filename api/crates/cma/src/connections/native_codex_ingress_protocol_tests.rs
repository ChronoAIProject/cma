use super::*;
use futures_util::FutureExt as _;
use layer3_adapter::ingress_budget::{ClassBounds, IngressBudget};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic native frame fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(32 * 1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn scope(policy: &IngressOwnershipPolicy) -> IngressWorkScope {
    IngressWorkScope::new(
        policy
            .admit(IngressClass::Data, plan(sizing::shared_owner_bytes::<()>()))
            .unwrap(),
    )
}

fn padded(size: usize) -> Vec<u8> {
    let mut bytes = vec![b' '; size];
    *bytes.last_mut().unwrap() = b'\n';
    bytes
}

struct Chunked {
    bytes: Vec<u8>,
    chunk: usize,
    consumed: usize,
    polls: usize,
    fail: Option<(usize, io::ErrorKind)>,
    pending_at: Option<usize>,
}

impl Chunked {
    fn new(bytes: &[u8], chunk: usize) -> Self {
        Self {
            bytes: bytes.to_vec(),
            chunk,
            consumed: 0,
            polls: 0,
            fail: None,
            pending_at: None,
        }
    }
}

impl AsyncBufRead for Chunked {
    fn poll_fill_buf(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        this.polls += 1;
        if this.pending_at.is_some_and(|at| this.consumed >= at) {
            return Poll::Pending;
        }
        if let Some((at, kind)) = this.fail {
            if this.consumed >= at {
                return Poll::Ready(Err(kind.into()));
            }
        }
        Poll::Ready(Ok(&this.bytes[this.consumed
            ..this.consumed + this.chunk.min(this.bytes.len() - this.consumed)]))
    }

    fn consume(self: Pin<&mut Self>, count: usize) {
        let this = self.get_mut();
        this.consumed += count;
    }
}

impl AsyncRead for Chunked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let bytes = std::task::ready!(self.as_mut().poll_fill_buf(cx))?;
        let count = bytes.len().min(output.remaining());
        output.put_slice(&bytes[..count]);
        self.consume(count);
        Poll::Ready(Ok(()))
    }
}

#[test]
fn exact_limit_preserves_bytes_and_custody() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    let bytes = padded(MAX_FRAME_BYTES);
    let frame = NativeFrameRead::admit(&scope, &policy)
        .unwrap()
        .read(&mut NativeFrameStream::new(std::io::Cursor::new(
            bytes.clone(),
        )))
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(frame.expose().as_bytes(), bytes);
    assert!(frame.shares_custody_domain(&policy));
    assert_eq!(
        baseline - root.available_bytes(),
        frame_plan().unwrap().requested_bytes()
    );
    drop(frame);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn one_byte_over_is_unavailable_and_returns_the_allowance() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    let bytes = padded(READ_BYTES);
    let mut input = NativeFrameStream::new(Chunked::new(&bytes, usize::MAX));
    let result = NativeFrameRead::admit(&scope, &policy)
        .unwrap()
        .read(&mut input)
        .now_or_never()
        .unwrap();
    assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
    assert_eq!(input.reader.consumed, READ_BYTES);
    assert!(!input.is_usable());
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn oversized_newline_free_input_stops_at_the_detection_byte() {
    let (_, policy) = policy();
    let scope = scope(&policy);
    let bytes = vec![b'x'; 3 * READ_BYTES];
    let mut input = NativeFrameStream::new(Chunked::new(&bytes, 8192));
    let result = NativeFrameRead::admit(&scope, &policy)
        .unwrap()
        .read(&mut input)
        .now_or_never()
        .unwrap();
    assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
    assert_eq!(input.reader.consumed, READ_BYTES);
    assert!(!input.is_usable());
    assert_eq!(input.reader.polls, READ_BYTES.div_ceil(8192));
}

#[test]
fn empty_and_truncated_frames_are_unavailable() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    for bytes in [b"".as_slice(), b"{\"id\":1}", b"\r"] {
        let mut input = NativeFrameStream::new(bytes);
        let result = NativeFrameRead::admit(&scope, &policy)
            .unwrap()
            .read(&mut input)
            .now_or_never()
            .unwrap();
        assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
        assert_eq!(root.available_bytes(), baseline);
    }
}

#[test]
fn newline_stops_before_the_next_buffered_frame() {
    let (_, policy) = policy();
    let scope = scope(&policy);
    let mut input = NativeFrameStream::new(b"{\"id\":1}\n{\"id\":2}\n".as_slice());
    for expected in ["{\"id\":1}\n", "{\"id\":2}\n"] {
        let frame = NativeFrameRead::admit(&scope, &policy)
            .unwrap()
            .read(&mut input)
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(frame.expose(), expected);
    }
    assert!(input.reader.is_empty());
    assert!(input.is_usable());
}

#[test]
fn one_byte_chunks_preserve_split_utf8_and_crlf() {
    let (_, policy) = policy();
    let scope = scope(&policy);
    let bytes = "{\"value\":\"合成🦀\"}\r\n".as_bytes();
    let mut input = NativeFrameStream::new(Chunked::new(bytes, 1));
    let frame = NativeFrameRead::admit(&scope, &policy)
        .unwrap()
        .read(&mut input)
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(frame.expose().as_bytes(), bytes);
    assert_eq!(input.reader.consumed, bytes.len());
}

#[test]
fn invalid_utf8_is_unavailable_and_releases_bytes() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    let result = NativeFrameRead::admit(&scope, &policy)
        .unwrap()
        .read(&mut NativeFrameStream::new([0xff, b'\n'].as_slice()))
        .now_or_never()
        .unwrap();
    assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn byte_framing_leaves_json_validation_to_the_parser() {
    let (_, policy) = policy();
    let scope = scope(&policy);
    for bytes in [b"\n".as_slice(), b"not-json\n", b"{\"id\":1,\"id\":2}\n"] {
        let mut input = NativeFrameStream::new(bytes);
        let frame = NativeFrameRead::admit(&scope, &policy)
            .unwrap()
            .read(&mut input)
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(frame.expose().as_bytes(), bytes);
    }
}

#[test]
fn partial_io_failure_preserves_the_existing_error_class() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let baseline = root.available_bytes();
    for kind in [io::ErrorKind::Interrupted, io::ErrorKind::BrokenPipe] {
        let mut input = NativeFrameStream::new(Chunked::new(b"synthetic\n", 2));
        input.reader.fail = Some((2, kind));
        let result = NativeFrameRead::admit(&scope, &policy)
            .unwrap()
            .read(&mut input)
            .now_or_never()
            .unwrap();
        assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
        assert_eq!(input.reader.consumed, 2);
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
}

#[test]
fn data_pressure_refuses_before_buffer_construction() {
    let (root, policy) = policy();
    let scope = scope(&policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![1u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    let baseline = root.available_bytes();
    assert!(matches!(
        NativeFrameRead::admit(&scope, &policy),
        Err(NativeFrameError::Refused(
            AdmissionRefusal::AtCapacity { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), baseline);
    drop(pressure);
}

#[test]
fn foreign_scope_is_rejected_before_buffer_construction() {
    let (root, local) = policy();
    let (_, foreign) = policy();
    let foreign_scope = scope(&foreign);
    let baseline = root.available_bytes();
    assert!(matches!(
        NativeFrameRead::admit(&foreign_scope, &local),
        Err(NativeFrameError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn process_attachment_and_sibling_domains_are_not_interchangeable() {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        2,
        plan(4096),
        plan(16 * 1024 * 1024),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root, bounds).unwrap();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let process_scope = scope(&policy);
    let first_scope = scope(first.policy());
    for (source, target) in [
        (&process_scope, first.policy()),
        (&first_scope, &policy),
        (&first_scope, second.policy()),
    ] {
        assert!(matches!(
            NativeFrameRead::admit(source, target),
            Err(NativeFrameError::Refused(
                AdmissionRefusal::InvalidPlan { .. }
            ))
        ));
    }
}

#[test]
fn actual_buffered_reader_preserves_legacy_framing_and_parser_outcomes() {
    use tokio::io::AsyncReadExt as _;
    let (_, policy) = policy();
    let source = scope(&policy);
    let mut exact = padded(MAX_FRAME_BYTES);
    exact[..2].copy_from_slice(b"{}");
    let mut oversized = padded(READ_BYTES);
    oversized[..2].copy_from_slice(b"{}");
    for bytes in [
        b"{\"id\":1}\n{\"id\":2}\n".to_vec(),
        b"{\"id\":1}\r\n".to_vec(),
        b"not-json\n".to_vec(),
        b"\n".to_vec(),
        b"{}".to_vec(),
        vec![0xff, b'\n'],
        exact,
        oversized,
    ] {
        for capacity in [1, 8192] {
            let mut legacy = tokio::io::BufReader::with_capacity(capacity, bytes.as_slice());
            let mut admitted = NativeFrameStream::new(tokio::io::BufReader::with_capacity(
                capacity,
                std::io::Cursor::new(bytes.clone()),
            ));
            for _ in 0..2 {
                let mut original = Vec::new();
                let size = (&mut legacy)
                    .take(READ_BYTES as u64)
                    .read_until(b'\n', &mut original)
                    .now_or_never()
                    .unwrap()
                    .unwrap();
                let expected: Result<serde_json::Value, NativeFrameError> =
                    if size == 0 || size > MAX_FRAME_BYTES || original.last() != Some(&b'\n') {
                        Err(NativeFrameError::Unavailable)
                    } else {
                        serde_json::from_slice(&original).map_err(|_| NativeFrameError::Unavailable)
                    };
                // Parsing here is only a semantic comparison. Its Value storage
                // is outside this byte helper's accounting and production API.
                let actual = NativeFrameRead::admit(&source, &policy)
                    .unwrap()
                    .read(&mut admitted)
                    .now_or_never()
                    .unwrap()
                    .and_then(|frame| {
                        serde_json::from_str(frame.expose())
                            .map_err(|_| NativeFrameError::Unavailable)
                    });
                assert_eq!(actual, expected);
            }
        }
    }
}

#[path = "native_codex_ingress_protocol_lifetime_tests.rs"]
mod lifetime;
