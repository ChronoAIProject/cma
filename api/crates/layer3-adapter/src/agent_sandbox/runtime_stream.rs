//! Authenticated Kubernetes streams to the fixed runtime port; no local listener.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use super::{
    api_error, rejected, unavailable, AgentSandboxClient, PodBinding, RuntimePermit,
    RuntimePermitKind, RuntimeWorkloadClass,
};
use crate::hosted::NativeRuntimeTransport;
use crate::{AdapterError, Result, RuntimeHandle};
use kube::api::{AttachedProcess, Portforwarder};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[path = "runtime_exec.rs"]
mod exec;

const RUNTIME_PORT: u16 = 9090;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) trait RuntimeIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RuntimeIo for T {}

pub(crate) enum RuntimeStreamKind {
    #[cfg(test)]
    Request,
    Conversation,
    ContinuityConversation,
    Desktop,
}

/// Owns the forwarding task for precisely as long as its stream is usable.
/// Kubernetes error-channel contents are deliberately excluded from errors/logs.
pub struct RuntimeStream {
    stream: Box<dyn RuntimeIo>,
    owner: StreamOwner,
    // true denotes a successful exec exit; its buffered stdout must still drain.
    terminal: Option<Pin<Box<dyn Future<Output = bool> + Send>>>,
    closed: bool,
    failed: bool,
    _permit: Option<RuntimePermit>,
}

enum StreamOwner {
    Portforward(Portforwarder),
    Exec(AttachedProcess),
}
impl StreamOwner {
    fn abort(&self) {
        match self {
            Self::Portforward(owner) => owner.abort(),
            Self::Exec(owner) => owner.abort(),
        }
    }
}

impl AgentSandboxClient {
    pub(crate) async fn runtime_stream(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        kind: RuntimeStreamKind,
        transport: NativeRuntimeTransport,
    ) -> Result<RuntimeStream> {
        self.runtime_stream_after(runtime, expected, kind, transport, async { Ok(()) })
            .await
    }

    /// Connection setup is authorized by an already-held operation permit. The
    /// cached socket owns no request permit after the operation finishes.
    pub(crate) async fn runtime_request_stream(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        transport: NativeRuntimeTransport,
        permit: &RuntimePermit,
    ) -> Result<RuntimeStream> {
        if !permit.proves_request(&self.runtime_capacity) {
            return Err(rejected("runtime request permit has a different authority"));
        }
        self.open_runtime_stream(runtime, expected, transport, None, async { Ok(()) })
            .await
    }

    /// Prepare only the unauthenticated connection before the actual Open receipt.
    pub(crate) async fn runtime_stream_after(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        kind: RuntimeStreamKind,
        transport: NativeRuntimeTransport,
        authorized: impl Future<Output = Result<()>> + Send,
    ) -> Result<RuntimeStream> {
        let (kind, class) = match kind {
            #[cfg(test)]
            RuntimeStreamKind::Request => {
                (RuntimePermitKind::Request, RuntimeWorkloadClass::Ordinary)
            }
            RuntimeStreamKind::Conversation | RuntimeStreamKind::Desktop => {
                (RuntimePermitKind::Stream, RuntimeWorkloadClass::Ordinary)
            }
            RuntimeStreamKind::ContinuityConversation => {
                (RuntimePermitKind::Stream, RuntimeWorkloadClass::Continuity)
            }
        };
        let permit = self.runtime_capacity.try_acquire(kind, class)?;
        self.open_runtime_stream(runtime, expected, transport, Some(permit), authorized)
            .await
    }

    async fn open_runtime_stream(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        transport: NativeRuntimeTransport,
        permit: Option<RuntimePermit>,
        authorized: impl Future<Output = Result<()>> + Send,
    ) -> Result<RuntimeStream> {
        let started = tokio::time::Instant::now();
        let stream = tokio::time::timeout(OPEN_TIMEOUT, async {
            if self.pod_binding(runtime).await? != *expected {
                return Err(rejected("Sandbox Pod changed before runtime stream opened"));
            }
            let stream = match transport {
                NativeRuntimeTransport::KubernetesPortforward => {
                    self.portforward_stream(expected, permit).await?
                }
                NativeRuntimeTransport::KubernetesExec => {
                    exec::open(self, expected, permit, started + OPEN_TIMEOUT).await?
                }
                NativeRuntimeTransport::Direct => {
                    return Err(rejected("direct transport cannot open a Kubernetes stream"));
                }
            };
            Ok(stream)
        })
        .await
        .map_err(|_| unavailable("Kubernetes runtime stream opening timed out"))??;
        // Waiting for the control mutation does not consume the stream I/O
        // deadline; a slow Open retains its existing independent timeout.
        let remaining = OPEN_TIMEOUT.saturating_sub(started.elapsed());
        authorized.await?;
        tokio::time::timeout(remaining, async {
            if self.pod_binding(runtime).await? != *expected {
                return Err(rejected("Sandbox Pod changed while runtime stream opened"));
            }
            // No runtime request or token has been sent before both identity checks pass.
            Ok(stream)
        })
        .await
        .map_err(|_| unavailable("Kubernetes runtime stream opening timed out"))?
    }

    async fn portforward_stream(
        &self,
        expected: &PodBinding,
        permit: Option<RuntimePermit>,
    ) -> Result<RuntimeStream> {
        let mut forwarder = self
            .pods
            .portforward(&expected.pod_name, &[RUNTIME_PORT])
            .await
            .map_err(stream_error)?;
        let Some(stream) = forwarder.take_stream(RUNTIME_PORT) else {
            forwarder.abort();
            return Err(unavailable("Kubernetes runtime stream is absent"));
        };
        let Some(error) = forwarder.take_error(RUNTIME_PORT) else {
            forwarder.abort();
            return Err(unavailable("Kubernetes runtime error channel is absent"));
        };
        // Construct the drop guard before the next await, including timeout cancellation.
        Ok(RuntimeStream {
            stream: Box::new(stream),
            owner: StreamOwner::Portforward(forwarder),
            terminal: Some(Box::pin(async move {
                let _ = error.await;
                false
            })),
            closed: false,
            failed: false,
            _permit: permit,
        })
    }
}

fn stream_error(error: kube::Error) -> AdapterError {
    match error {
        kube::Error::UpgradeConnection(kube::client::UpgradeConnectionError::ProtocolSwitch(
            status,
        )) => match status.as_u16() {
            429 => AdapterError::AtCapacity("Kubernetes runtime stream was throttled".into()),
            400 | 401 | 403 | 405 | 422 => {
                rejected(&format!("Kubernetes rejected runtime stream ({status})"))
            }
            _ => unavailable("Kubernetes runtime stream upgrade failed"),
        },
        other => api_error(other),
    }
}

impl RuntimeStream {
    fn check(&mut self, cx: &mut Context<'_>, writing: bool) -> io::Result<()> {
        if let Some(terminal) = &mut self.terminal {
            if let Poll::Ready(success) = terminal.as_mut().poll(cx) {
                self.closed = true;
                self.failed = !success;
                self.terminal = None;
            }
        }
        if self.failed || (writing && self.closed) {
            self.owner.abort();
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Kubernetes runtime stream closed",
            ))
        } else {
            Ok(())
        }
    }
}
impl Drop for RuntimeStream {
    fn drop(&mut self) {
        self.owner.abort();
    }
}
impl AsyncRead for RuntimeStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.check(cx, false)?;
        let before = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        if matches!(&result, Poll::Ready(Ok(())))
            && buf.filled().len() == before
            && buf.remaining() > 0
        {
            self.closed = true;
            self.owner.abort();
        }
        result
    }
}
impl AsyncWrite for RuntimeStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.check(cx, true)?;
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // HTTP keeps polling flush while it drains response bytes. A successful
        // exec exit must allow that no-op; new writes still fail in poll_write.
        // A failed or missing exec status continues to reject both operations.
        self.check(cx, false)?;
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
