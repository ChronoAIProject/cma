use super::super::{
    io::{ObservedIo, Socket},
    observation::{ActiveWrite, Observation, Pending},
};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

/// Simulate a TLS writer accepting plaintext while its underlying socket is blocked.
struct Buffered {
    socket: Socket<tokio::io::DuplexStream>,
    pending: Vec<u8>,
}
impl AsyncRead for Buffered {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_read(cx, buffer)
    }
}
impl AsyncWrite for Buffered {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.pending.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while !this.pending.is_empty() {
            let count =
                std::task::ready!(Pin::new(&mut this.socket).poll_write(cx, &this.pending))?;
            this.pending.drain(..count);
        }
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}
#[tokio::test]
async fn buffered_plaintext_does_not_set_a_socket_timestamp() {
    let (writer, _reader) = tokio::io::duplex(4096);
    let active: ActiveWrite = Default::default();
    let observation = Observation::new(None);
    let pending: Pending = Default::default();
    pending.lock().unwrap().insert(0, observation.clone());
    let buffered = Buffered {
        socket: Socket {
            inner: writer,
            active: active.clone(),
        },
        pending: vec![],
    };
    let mut io = ObservedIo::new(Box::new(buffered), active, false, pending);
    io.write_all(b"POST /responses HTTP/1.1\r\n").await.unwrap();
    assert!(observation.at.get().is_none());
    let before = Instant::now();
    io.flush().await.unwrap();
    assert!(*observation.at.get().unwrap() >= before);
}
#[tokio::test]
async fn split_http2_headers_keep_the_first_byte_time_and_ignore_control_frames() {
    let (writer, _reader) = tokio::io::duplex(4096);
    let active: ActiveWrite = Default::default();
    let pending: Pending = Default::default();
    let observation = Observation::new(None);
    pending.lock().unwrap().insert(3, observation.clone());
    let socket = Socket {
        inner: writer,
        active: active.clone(),
    };
    let mut io = ObservedIo::new(Box::new(socket), active, true, pending);
    io.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    io.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap(); // SETTINGS
    assert!(observation.at.get().is_none());
    let before = Instant::now();
    io.write_all(&[0]).await.unwrap();
    let first = Instant::now();
    assert!(observation.at.get().is_none());
    tokio::task::yield_now().await;
    io.write_all(&[0, 0, 1, 4, 0, 0, 0, 3]).await.unwrap(); // HEADERS on stream 3
    let at = *observation.at.get().unwrap();
    assert!(at >= before && at <= first);
    // Subsequent frames for the same stream never overwrite its first timestamp.
    io.write_all(&[0, 0, 0, 1, 4, 0, 0, 0, 3]).await.unwrap();
    assert_eq!(observation.at.get(), Some(&at));
    assert_eq!(Arc::strong_count(&observation), 1);
}
