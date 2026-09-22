//! HTTP/1 reuse requires EOF; HTTP/2 cancellation resets only the individual stream.
use super::{Completion, Driver, Idle, H1};
use bytes::Bytes;
use futures_util::Stream;
use http_body_util::BodyExt;
use hyper::body::{Body, Incoming};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{ready, Context, Poll},
    time::{Duration, Instant},
};

pub struct H1Body {
    pub body: Option<Incoming>,
    pub connection: Option<H1>,
    pub idle: Arc<Mutex<Vec<Idle>>>,
    pub complete: Completion,
}
impl H1Body {
    fn recycle(&mut self) {
        if let Some(connection) = self.connection.take() {
            let mut idle = self.idle.lock().unwrap();
            if idle.len() < 64 {
                idle.push(Idle {
                    connection,
                    at: Instant::now(),
                });
            }
        }
    }
}
impl Stream for H1Body {
    type Item = Result<Bytes, io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(body) = &mut self.body else {
                return Poll::Ready(None);
            };
            match ready!(Pin::new(body).poll_frame(cx)) {
                Some(Ok(frame)) => {
                    if let Ok(bytes) = frame.into_data() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Some(Err(_)) => {
                    self.body = None;
                    self.connection = None;
                    return Poll::Ready(Some(Err(io::Error::other("provider body failed"))));
                }
                None => {
                    self.body = None;
                    self.recycle();
                    return Poll::Ready(None);
                }
            }
        }
    }
}
impl Drop for H1Body {
    fn drop(&mut self) {
        if !self.complete.is_complete() {
            return;
        }
        let (Some(mut body), Some(connection)) = (self.body.take(), self.connection.take()) else {
            return;
        };
        let idle = self.idle.clone();
        // A terminal SSE frame may precede EOF. Briefly drain a bounded tail so a
        // well-behaved server can reuse the connection. Cancellation never drains.
        tokio::spawn(async move {
            let drained = tokio::time::timeout(Duration::from_millis(100), async {
                let mut bytes = 0;
                while let Some(frame) = body.frame().await {
                    let frame = frame.map_err(|_| ())?;
                    if let Some(data) = frame.data_ref() {
                        bytes += data.len();
                    }
                    if bytes > 16 * 1024 {
                        return Err(());
                    }
                }
                Ok(())
            })
            .await;
            if matches!(drained, Ok(Ok(()))) {
                let mut idle = idle.lock().unwrap();
                if idle.len() < 64 {
                    idle.push(Idle {
                        connection,
                        at: Instant::now(),
                    });
                }
            }
        });
    }
}

pub struct H2Body {
    pub body: h2::RecvStream,
    pub send: h2::SendStream<Bytes>,
    pub driver: Arc<Driver>,
}
impl Stream for H2Body {
    type Item = Result<Bytes, io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let _keep_alive = &self.driver;
        match ready!(self.body.poll_data(cx)) {
            Some(Ok(bytes)) => {
                if self
                    .body
                    .flow_control()
                    .release_capacity(bytes.len())
                    .is_err()
                {
                    return Poll::Ready(Some(Err(io::Error::other(
                        "provider flow control failed",
                    ))));
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Some(Err(_)) => Poll::Ready(Some(Err(io::Error::other("provider body failed")))),
            None => Poll::Ready(None),
        }
    }
}
impl Drop for H2Body {
    fn drop(&mut self) {
        if !self.body.is_end_stream() {
            self.send.send_reset(h2::Reason::CANCEL);
        }
    }
}
