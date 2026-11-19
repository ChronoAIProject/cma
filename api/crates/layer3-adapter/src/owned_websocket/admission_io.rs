use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::tungstenite::protocol::frame::FrameHeader;

use super::header_gate::HeaderGate;

/// Reserve the complete codec growth plan without waiting before this returns.
/// The implementation retains reservations through the allocations they cover,
/// including fragments and messages that outlive the next read.
pub trait HeaderAdmission {
    fn admit(&mut self, header: &FrameHeader, payload_bytes: u64) -> io::Result<()>;
}

impl<F> HeaderAdmission for F
where
    F: FnMut(&FrameHeader, u64) -> io::Result<()>,
{
    fn admit(&mut self, header: &FrameHeader, payload_bytes: u64) -> io::Result<()> {
        self(header, payload_bytes)
    }
}

/// Withhold each complete header until its allocation plan is admitted.
///
/// Construct this at the raw upgrade boundary, including any bytes read ahead
/// during HTTP upgrade. Codec construction, HTTP work and outgoing codec growth
/// require their own prior reservations. This adapter does not provide them.
pub struct AdmissionIo<S, A> {
    stream: S,
    admission: A,
    header: HeaderGate,
    ended: bool,
    failed: bool,
}

impl<S, A: HeaderAdmission> AdmissionIo<S, A> {
    pub fn after_upgrade(stream: S, admission: A) -> Self {
        Self {
            stream,
            admission,
            header: HeaderGate::default(),
            ended: false,
            failed: false,
        }
    }
}

impl<S: AsyncRead + Unpin, A: HeaderAdmission + Unpin> AdmissionIo<S, A> {
    fn read_frame_bytes(
        &mut self,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.header.begin();
        while self.header.needs_header() {
            if let Some((header, length)) = self.header.parse()? {
                self.admission.admit(&header, length)?;
                self.header.admit(length);
                break;
            }
            // Reading only the unparsed header prevents coalesced payload or a
            // following header from reaching the codec before its own admission.
            let mut input = ReadBuf::new(self.header.unfilled_header()?);
            let result = Pin::new(&mut self.stream).poll_read(cx, &mut input);
            let received = input.filled().len();
            match result {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) if received == 0 => {
                    self.ended = true;
                    return Poll::Ready(if self.header.is_partial() {
                        Err(io::ErrorKind::UnexpectedEof.into())
                    } else {
                        Ok(())
                    });
                }
                Poll::Ready(Ok(())) => self.header.received_header(received),
            }
        }
        let header = self.header.header();
        if !header.is_empty() {
            let count = header.len().min(output.remaining());
            output.put_slice(&header[..count]);
            self.header.emitted_header(count);
            return Poll::Ready(Ok(()));
        }
        let limit = self.header.payload_chunk(output.remaining());
        let mut input = ReadBuf::new(output.initialize_unfilled_to(limit));
        match Pin::new(&mut self.stream).poll_read(cx, &mut input) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {
                let count = input.filled().len();
                if count == 0 && limit != 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }
                output.advance(count);
                self.header.emitted_payload(count);
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl<S: AsyncRead + Unpin, A: HeaderAdmission + Unpin> AsyncRead for AdmissionIo<S, A> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.failed {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if this.ended {
            return Poll::Ready(Ok(()));
        }
        let result = this.read_frame_bytes(cx, output);
        if matches!(&result, Poll::Ready(Err(_))) {
            this.failed = true;
        }
        result
    }
}

impl<S: AsyncWrite + Unpin, A: HeaderAdmission + Unpin> AsyncWrite for AdmissionIo<S, A> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
