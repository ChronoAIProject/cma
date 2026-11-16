//! Closed organization failure categories carried only by the current request.
use std::{cell::Cell, future::Future};

#[derive(Clone, Copy)]
pub enum Operation {
    Choices,
    Membership,
}

impl Operation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Choices => "choices",
            Self::Membership => "membership",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Stage {
    Principal,
    Endpoint,
    ResponseHeaders,
    Status,
    Body,
    Decode,
    Evidence,
    Deadline,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Endpoint => "endpoint",
            Self::ResponseHeaders => "response_headers",
            Self::Status => "status",
            Self::Body => "body",
            Self::Decode => "decode",
            Self::Evidence => "evidence",
            Self::Deadline => "deadline",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Reason {
    Unavailable,
    Timeout,
    Connection,
    Transport,
    HttpStatus,
    BodyRead,
    BodyTooLarge,
    InvalidBody,
    InvalidEvidence,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Connection => "connection",
            Self::Transport => "transport",
            Self::HttpStatus => "http_status",
            Self::BodyRead => "body_read",
            Self::BodyTooLarge => "body_too_large",
            Self::InvalidBody => "invalid_body",
            Self::InvalidEvidence => "invalid_evidence",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Failure {
    pub operation: Operation,
    pub stage: Stage,
    pub reason: Reason,
    pub status: Option<u16>,
}

impl Failure {
    /// Retain the latest attempt so a final refusal is never masked by an earlier outage.
    pub fn record(self) {
        let _ = CURRENT.try_with(|current| current.set(Some(self)));
    }

    pub fn detail(self) -> String {
        let status = self
            .status
            .map(|value| format!("; upstream_status={value}"))
            .unwrap_or_default();
        format!("Current organization authorization is unavailable (operation={}; stage={}; reason={}{status}). Retry after the identity provider recovers.",
            self.operation.label(), self.stage.label(), self.reason.label())
    }
}

tokio::task_local! { static CURRENT: Cell<Option<Failure>>; }

pub async fn capture<F: Future>(future: F) -> (F::Output, Option<Failure>) {
    CURRENT
        .scope(Cell::new(None), async {
            let result = future.await;
            (result, CURRENT.with(Cell::get))
        })
        .await
}
