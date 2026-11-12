//! HTTP attempts and response-body observations, never durable work outcomes.
use axum::{
    body::{Body, Bytes},
    extract::{MatchedPath, Request},
    http::{header, Method},
    middleware::Next,
    response::Response,
};
use http_body::{Body as _, Frame, SizeHint};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

pub(crate) async fn observe(mut request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let method = method_label(request.method());
    let attribution = super::request::Attribution::default();
    request.extensions_mut().insert(attribution.clone());
    let protocol = super::request::protocol(&route);
    let started = Instant::now();
    let mut attempt = Attempt {
        route: route.clone(),
        method,
        started,
        responded: false,
        attribution: attribution.clone(),
        protocol,
    };
    metrics::counter!("cma_http_attempts_total", "route" => route.clone(), "method" => method)
        .increment(1);
    metrics::gauge!("cma_http_in_flight", "route" => route.clone(), "method" => method)
        .increment(1.);
    let response = next.run(request).await;
    attempt.responded = true;
    let status = response.status().as_u16().to_string();
    let consumer = attribution.consumer();
    metrics::counter!("cma_http_classified_responses_total", "route" => route.clone(), "method" => method, "status" => status.clone(), "consumer" => consumer, "protocol" => protocol).increment(1);
    metrics::histogram!("cma_http_classified_headers_seconds", "route" => route.clone(), "method" => method, "consumer" => consumer, "protocol" => protocol).record(started.elapsed().as_secs_f64());
    metrics::counter!("cma_http_responses_total", "route" => route.clone(), "method" => method, "status" => status).increment(1);
    metrics::histogram!("cma_http_response_headers_seconds", "route" => route.clone(), "method" => method).record(started.elapsed().as_secs_f64());
    let sse = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|v| v.trim() == "text/event-stream")
        });
    let (parts, body) = response.into_parts();
    let bytes = metrics::counter!("cma_http_response_body_bytes_total", "route" => route.clone(), "method" => method);
    let mut observed = ObservedBody {
        body,
        bytes,
        route,
        method,
        sse,
        started,
        finished: false,
    };
    if observed.body.is_end_stream() {
        observed.finish("eof");
    }
    Response::from_parts(parts, Body::new(observed))
}

fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

struct Attempt {
    route: String,
    method: &'static str,
    started: Instant,
    responded: bool,
    attribution: super::request::Attribution,
    protocol: &'static str,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        metrics::counter!("cma_http_classified_attempts_total", "route" => self.route.clone(), "method" => self.method, "consumer" => self.attribution.consumer(), "protocol" => self.protocol).increment(1);
        metrics::gauge!("cma_http_in_flight", "route" => self.route.clone(), "method" => self.method).decrement(1.);
        if !self.responded {
            metrics::counter!("cma_http_before_headers_cancelled_total", "route" => self.route.clone(), "method" => self.method).increment(1);
            metrics::histogram!("cma_http_cancelled_before_headers_seconds", "route" => self.route.clone(), "method" => self.method).record(self.started.elapsed().as_secs_f64());
        }
    }
}

struct ObservedBody {
    body: Body,
    // Resolve the label set once, not on each SSE frame.
    bytes: metrics::Counter,
    route: String,
    method: &'static str,
    sse: bool,
    started: Instant,
    finished: bool,
}
impl ObservedBody {
    fn finish(&mut self, reason: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        metrics::counter!("cma_http_body_closed_total", "route" => self.route.clone(), "method" => self.method, "reason" => reason).increment(1);
        metrics::histogram!("cma_http_body_lifetime_seconds", "route" => self.route.clone(), "method" => self.method).record(self.started.elapsed().as_secs_f64());
        if self.sse {
            metrics::counter!("cma_sse_observation_closed_total", "route" => self.route.clone(), "reason" => reason).increment(1);
        }
    }
}
impl http_body::Body for ObservedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let result = Pin::new(&mut self.body).poll_frame(cx);
        match &result {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    self.bytes.increment(data.len() as u64);
                }
                if self.body.is_end_stream() {
                    self.finish("eof");
                }
            }
            Poll::Ready(Some(Err(_))) => self.finish("error"),
            Poll::Ready(None) => self.finish("eof"),
            Poll::Pending => {}
        }
        result
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
impl Drop for ObservedBody {
    fn drop(&mut self) {
        self.finish("dropped");
    }
}

#[cfg(test)]
mod tests;
