use super::{Input, *};
use crate::owned_websocket::{HeaderParser, Tungstenite024, Tungstenite029};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite_029::tungstenite::protocol::Role as Role029;
use tokio_tungstenite_029::{
    tungstenite::{
        protocol::frame::FrameHeader as Header029, protocol::WebSocketConfig, Message as Message029,
    },
    WebSocketStream as Socket029,
};

struct MaskedFramesWithBlockedWriter {
    frame: usize,
    offset: usize,
    payload_bytes: usize,
}

impl io::Read for MaskedFramesWithBlockedWriter {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.frame > 13 || output.is_empty() {
            return Ok(0);
        }
        // Two large binary frames surround twelve one-byte Pings. All masks
        // are synthetic zero keys; input is generated without payload storage.
        let binary = self.frame == 0 || self.frame == 13;
        let mut header = [0u8; 14];
        header[0] = if binary { 0x82 } else { 0x89 };
        header[1] = if binary { 0xff } else { 0x81 };
        let (header_len, length) = if binary {
            header[2..10].copy_from_slice(&(self.payload_bytes as u64).to_be_bytes());
            (14, self.payload_bytes)
        } else {
            (6, 1)
        };
        let count = if self.offset < header_len {
            let count = output.len().min(header_len - self.offset);
            output[..count].copy_from_slice(&header[self.offset..self.offset + count]);
            count
        } else {
            let count = output.len().min(header_len + length - self.offset);
            output[..count].fill(0);
            count
        };
        self.offset += count;
        if self.offset == header_len + length {
            self.frame += 1;
            self.offset = 0;
        }
        Ok(count)
    }
}

impl io::Write for MaskedFramesWithBlockedWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::WouldBlock.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::WouldBlock.into())
    }
}

#[test]
fn blocked_automatic_pong_retains_large_backing_across_the_next_read() {
    use crate::allocation_probe::AllocationProbe;
    use tokio_tungstenite_029::tungstenite::WebSocket;

    let length = 8 * 1024 * 1024;
    let mut probe = AllocationProbe::new();
    let mut socket = probe.track(|| {
        WebSocket::from_raw_socket(
            MaskedFramesWithBlockedWriter {
                frame: 0,
                offset: 0,
                payload_bytes: length,
            },
            Role029::Server,
            Some(
                WebSocketConfig::default()
                    .read_buffer_size(4096)
                    .write_buffer_size(0)
                    .max_write_buffer_size(32)
                    .max_message_size(Some(length))
                    .max_frame_size(Some(length)),
            ),
        )
    });
    let first = probe.track(|| socket.read().unwrap());
    assert_eq!(first.len(), length);
    drop(first);
    for _ in 0..12 {
        let ping = probe.track(|| socket.read().unwrap());
        assert_eq!(ping, Message029::Ping(bytes::Bytes::from_static(&[0])));
        drop(ping);
    }
    let retained_before = probe.live();
    assert!(retained_before >= length);
    let second = probe.track(|| socket.read().unwrap());
    assert_eq!(second.len(), length);
    drop(second);
    let retained_after = probe.live();
    assert!(
        retained_after >= 2 * length,
        "the tiny pending Pong pins old backing alongside the new read buffer"
    );
    eprintln!("blocked_pong_before={retained_before} after={retained_after}");
    drop(socket);
    assert_eq!(probe.live(), 0);
}

#[tokio::test]
async fn coalesced_headers_and_one_byte_splits_use_the_matching_parser() {
    for split in [false, true] {
        let input = Input::new(&[0x82, 1, b'a', 0x82, 126, 0, 1, b'b'], split);
        let consumed = input.consumed.clone();
        let positions = Arc::new(Mutex::new(Vec::new()));
        let observed = positions.clone();
        let mut socket = Socket029::from_raw_socket(
            AdmissionIo::<_, _, Tungstenite029>::after_upgrade_with_parser(
                input,
                move |_: &Header029, length| {
                    assert_eq!(length, 1);
                    observed
                        .lock()
                        .unwrap()
                        .push(consumed.load(Ordering::SeqCst));
                    Ok(())
                },
            ),
            Role029::Client,
            None,
        )
        .await;
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message029::binary(vec![b'a'])
        );
        assert_eq!(*positions.lock().unwrap(), [2]);
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message029::binary(vec![b'b'])
        );
        assert_eq!(*positions.lock().unwrap(), [2, 7]);
    }
}

#[tokio::test]
async fn masked_extended_header_and_fragmented_utf8_remain_upstream_validated() {
    let input = Input::new(
        &[0x82, 255, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 3, 4, b'x' ^ 1],
        true,
    );
    let consumed = input.consumed.clone();
    let mut socket = Socket029::from_raw_socket(
        AdmissionIo::<_, _, Tungstenite029>::after_upgrade_with_parser(
            input,
            move |header: &Header029, length| {
                assert_eq!(header.mask, Some([1, 2, 3, 4]));
                assert_eq!(consumed.load(Ordering::SeqCst), 14);
                assert_eq!(length, 1);
                Ok(())
            },
        ),
        Role029::Server,
        None,
    )
    .await;
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message029::binary(vec![b'x'])
    );
    let mut socket = Socket029::from_raw_socket(
        AdmissionIo::<_, _, Tungstenite029>::after_upgrade_with_parser(
            Input::new(
                &[0x01, 1, 0xe2, 0x89, 0, 0x80, 2, 0x82, 0xac, 0x88, 0],
                true,
            ),
            |_: &Header029, _| Ok(()),
        ),
        Role029::Client,
        None,
    )
    .await;
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message029::Ping(Default::default())
    );
    assert_eq!(socket.next().await.unwrap().unwrap(), Message029::text("€"));
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message029::Close(None)
    );
}

#[tokio::test]
async fn invalid_frames_still_fail_in_the_029_protocol_engine() {
    let cases: &[(&'static [u8], Role029)] = &[
        (&[0xc2, 1, b'x'], Role029::Client),
        (&[0x83, 0], Role029::Client),
        (&[0x80, 1, b'x'], Role029::Client),
        (&[0x81, 1, 0xff], Role029::Client),
        (&[0x09, 0], Role029::Client),
        (&[0x82, 1, b'x'], Role029::Server),
        (&[0x82, 129, 1, 2, 3, 4, b'x' ^ 1], Role029::Client),
        (&[0x01, 1, 0xe2, 0x80, 0], Role029::Client),
    ];
    for &(bytes, role) in cases {
        let mut socket = Socket029::from_raw_socket(
            AdmissionIo::<_, _, Tungstenite029>::after_upgrade_with_parser(
                Input::new(bytes, true),
                |_: &Header029, _| Ok(()),
            ),
            role,
            None,
        )
        .await;
        assert!(socket.next().await.unwrap().is_err(), "wire {bytes:?}");
        assert!(socket.next().await.is_none());
    }
}

#[tokio::test]
async fn cancelled_029_header_and_full_width_refusal_preserve_the_boundary() {
    let input = Input::new(
        &[0x82, 127, 255, 255, 255, 255, 255, 255, 255, 255, b'x'],
        true,
    );
    let consumed = input.consumed.clone();
    let mut io = AdmissionIo::<_, _, Tungstenite029>::after_upgrade_with_parser(
        input,
        |_: &Header029, length| {
            assert_eq!(length, u64::MAX);
            Err(io::ErrorKind::PermissionDenied.into())
        },
    );
    let mut output = [0x55; 32];
    assert!(io.read(&mut output).now_or_never().is_none());
    assert_eq!(
        io.read(&mut output).await.unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(consumed.load(Ordering::SeqCst), 10);
    assert_eq!(output, [0x55; 32]);
    assert_eq!(
        io.read(&mut output).await.unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[tokio::test]
async fn refused_029_header_prevents_the_advertised_reserve_without_payload() {
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let operation = async {
        let input = Input::new(&[0x82, 127, 0, 0, 0, 0, 1, 0, 0, 0], false);
        let consumed = input.consumed.clone();
        let mut socket = Socket029::from_raw_socket(
            AdmissionIo::<_, _, Tungstenite029>::after_upgrade_with_parser(
                input,
                |_: &Header029, length| {
                    assert_eq!(length, 16 * 1024 * 1024);
                    Err(io::ErrorKind::PermissionDenied.into())
                },
            ),
            Role029::Client,
            Some(WebSocketConfig::default().read_buffer_size(4096)),
        )
        .await;
        assert!(socket.next().await.unwrap().is_err());
        assert_eq!(consumed.load(Ordering::SeqCst), 10);
    };
    probe.track(|| operation.now_or_never().expect("ready synthetic I/O"));
    assert!(probe.peak() < 32 * 1024);
    assert_eq!(probe.live(), 0);
}

#[tokio::test]
async fn ungated_029_header_alone_exceeds_the_allocation_refusal_bound() {
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let operation = async {
        let input = Input::new(&[0x82, 127, 0, 0, 0, 0, 1, 0, 0, 0], false);
        let consumed = input.consumed.clone();
        let mut socket = Socket029::from_raw_socket(
            input,
            Role029::Client,
            Some(WebSocketConfig::default().read_buffer_size(4096)),
        )
        .await;
        assert!(socket.next().await.unwrap().is_err());
        assert_eq!(consumed.load(Ordering::SeqCst), 10);
    };
    probe.track(|| operation.now_or_never().expect("ready synthetic I/O"));
    assert!(probe.peak() >= 16 * 1024 * 1024);
    assert_eq!(probe.live(), 0);
}

struct Interrupted {
    input: Input,
    after: usize,
    interrupted: bool,
}

impl AsyncRead for Interrupted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.interrupted && self.input.consumed.load(Ordering::SeqCst) >= self.after {
            self.interrupted = true;
            return Poll::Ready(Err(io::ErrorKind::Interrupted.into()));
        }
        Pin::new(&mut self.input).poll_read(cx, output)
    }
}

impl AsyncWrite for Interrupted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.input).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.input).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.input).poll_shutdown(cx)
    }
}

async fn interruption_is_terminal<P: HeaderParser>() {
    for after in [1, 4] {
        let input = Input::new(&[0x82, 126, 0, 1, b'x'], true);
        let consumed = input.consumed.clone();
        let mut io = AdmissionIo::<_, _, P>::after_upgrade_with_parser(
            Interrupted {
                input,
                after,
                interrupted: false,
            },
            |_: &P::Header, _| Ok(()),
        );
        let mut output = Vec::new();
        assert_eq!(
            io.read_to_end(&mut output).await.unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(consumed.load(Ordering::SeqCst), after);
        assert_eq!(output.len(), if after == 1 { 0 } else { 4 });
        assert_eq!(
            io.read_to_end(&mut output).await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(consumed.load(Ordering::SeqCst), after);
        io.write_all(&[0x88, 0]).await.unwrap();
    }
}

#[tokio::test]
async fn interrupted_024_reads_are_terminal_but_control_writes_remain_possible() {
    interruption_is_terminal::<Tungstenite024>().await;
}

#[tokio::test]
async fn interrupted_029_reads_are_terminal_but_control_writes_remain_possible() {
    interruption_is_terminal::<Tungstenite029>().await;
}
