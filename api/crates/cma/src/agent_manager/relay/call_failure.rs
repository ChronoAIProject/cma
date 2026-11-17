//! Capacity remains recoverable across the private owner hop; receipts stay put.
use layer3_adapter::{agent_sandbox::*, AdapterError};

pub(super) enum CallFailure {
    Unavailable,
    RelayCapacity,
    Runtime(AdapterError),
}

impl From<()> for CallFailure {
    fn from(_: ()) -> Self {
        Self::Unavailable
    }
}

impl From<AdapterError> for CallFailure {
    fn from(error: AdapterError) -> Self {
        Self::Runtime(error)
    }
}

impl CallFailure {
    pub(super) fn into_adapter(self, detail: &str) -> AdapterError {
        match self {
            Self::Unavailable => AdapterError::Unavailable(detail.into()),
            Self::RelayCapacity => {
                AdapterError::Unavailable("relay_capacity_exhausted: slots_exhausted".into())
            }
            Self::Runtime(error) => error,
        }
    }

    pub(super) async fn remote(
        mut response: reqwest::Response,
        class: RuntimeWorkloadClass,
    ) -> Self {
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Self::Unavailable;
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if chunk.len() <= 4096_usize.saturating_sub(bytes.len()) => {
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                _ => return Self::Unavailable,
            }
        }
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Self::Unavailable;
        };
        Self::problem(body["code"].as_str().unwrap_or_default(), class)
    }

    pub(super) fn problem(code: &str, class: RuntimeWorkloadClass) -> Self {
        let kind = match code {
            "relay_capacity_exhausted" => return Self::RelayCapacity,
            "runtime_stream_capacity_exhausted" => RuntimePermitKind::Stream,
            "runtime_request_capacity_exhausted" => RuntimePermitKind::Request,
            _ => return Self::Unavailable,
        };
        Self::Runtime(
            RuntimeCapacityFailure {
                kind,
                class,
                reason: RuntimeCapacityReason::Exhausted,
            }
            .into(),
        )
    }
}

pub(super) fn response(error: AdapterError) -> axum::response::Response {
    use axum::response::IntoResponse;
    match error {
        AdapterError::RuntimeCapacity(failure) => match failure.kind {
            RuntimePermitKind::Stream => {
                layer1_public::rest::HttpProblem::RuntimeStreamCapacityExhausted
            }
            RuntimePermitKind::Request => {
                layer1_public::rest::HttpProblem::RuntimeRequestCapacityExhausted
            }
        }
        .into_response(),
        _ => super::http::unavailable(),
    }
}
