//! Authenticated Kubernetes streams to the fixed runtime port; no local listener.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use kube::api::Portforwarder;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::OwnedSemaphorePermit;

use super::{api_error, rejected, unavailable, AgentSandboxClient, PodBinding};
use crate::{AdapterError, Result, RuntimeHandle};

const RUNTIME_PORT: u16 = 9090;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) trait RuntimeIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RuntimeIo for T {}

pub(crate) enum RuntimeStreamKind {
    Request,
    Conversation,
}

/// Owns the forwarding task for precisely as long as its stream is usable.
/// Kubernetes error-channel contents are deliberately excluded from errors/logs.
pub(crate) struct RuntimeStream {
    stream: Box<dyn RuntimeIo>,
    forwarder: Portforwarder,
    error: Option<Pin<Box<dyn Future<Output = Option<String>> + Send>>>,
    failed: bool,
    _permit: OwnedSemaphorePermit,
}

impl AgentSandboxClient {
    pub(crate) async fn runtime_stream(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        kind: RuntimeStreamKind,
    ) -> Result<RuntimeStream> {
        // No unbounded queue behind long-lived conversations. Clones share this budget.
        let permits = match kind {
            RuntimeStreamKind::Request => &self.runtime_requests,
            RuntimeStreamKind::Conversation => &self.runtime_streams,
        };
        let permit = permits.clone().try_acquire_owned().map_err(|_| {
            AdapterError::AtCapacity("native runtime stream capacity is exhausted".into())
        })?;
        tokio::time::timeout(OPEN_TIMEOUT, async {
            if self.pod_binding(runtime).await? != *expected {
                return Err(rejected("Sandbox Pod changed before runtime stream opened"));
            }
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
            let stream = RuntimeStream {
                stream: Box::new(stream),
                forwarder,
                error: Some(Box::pin(error)),
                failed: false,
                _permit: permit,
            };
            if self.pod_binding(runtime).await? != *expected {
                return Err(rejected("Sandbox Pod changed while runtime stream opened"));
            }
            // No runtime request or token has been sent before both identity checks pass.
            Ok(stream)
        })
        .await
        .map_err(|_| unavailable("Kubernetes runtime stream opening timed out"))?
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
    fn check(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if let Some(error) = &mut self.error {
            if error.as_mut().poll(cx).is_ready() {
                // kube's error-channel contract makes even sender disappearance
                // terminal; do not allow further writes after a silent close.
                self.failed = true;
                self.error = None;
            }
        }
        if self.failed {
            self.forwarder.abort();
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
        self.forwarder.abort();
    }
}
impl AsyncRead for RuntimeStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.check(cx)?;
        let before = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        if matches!(&result, Poll::Ready(Ok(())))
            && buf.filled().len() == before
            && buf.remaining() > 0
        {
            self.failed = true;
            self.forwarder.abort();
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
        self.check(cx)?;
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.check(cx)?;
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
