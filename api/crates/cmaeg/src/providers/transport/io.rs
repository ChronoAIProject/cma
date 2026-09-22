//! Observe the real TCP write, below TLS, with attribution from plaintext frame boundaries.
use super::observation::{ActiveWrite, Frames, Pending, Stamp};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Instant,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
pub type BoxIo = Box<dyn Stream>;

pub struct Socket<T> {
    pub inner: T,
    pub active: ActiveWrite,
}
impl<T: AsyncRead + Unpin> AsyncRead for Socket<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for Socket<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let count = ready!(Pin::new(&mut self.inner).poll_write(cx, bytes))?;
        if count > 0 {
            let at = Instant::now();
            if let Some(stamp) = self.active.lock().unwrap().take() {
                stamp.written(at);
            }
        }
        Poll::Ready(Ok(count))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub struct ObservedIo {
    inner: BoxIo,
    active: ActiveWrite,
    frames: Frames,
}
impl ObservedIo {
    pub fn new(inner: BoxIo, active: ActiveWrite, h2: bool, pending: Pending) -> Self {
        Self {
            inner,
            active,
            frames: Frames::new(h2, pending),
        }
    }
}
impl AsyncRead for ObservedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}
impl AsyncWrite for ObservedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // Flush earlier TLS records (including handshake/control bytes) before arming
        // the next segment. A buffered plaintext acceptance never sets the timestamp.
        ready!(Pin::new(&mut self.inner).poll_flush(cx))?;
        let limit = bytes.len().min(self.frames.limit());
        let stamp = Arc::new(Stamp::default());
        *self.active.lock().unwrap() = Some(stamp.clone());
        let count = ready!(Pin::new(&mut self.inner).poll_write(cx, &bytes[..limit]))?;
        self.frames.accepted(&bytes[..count], stamp);
        Poll::Ready(Ok(count))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
