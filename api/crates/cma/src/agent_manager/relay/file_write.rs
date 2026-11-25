use super::*;
use axum::{http::Method, response::Response};

pub(super) const NOT_ENTERED_HEADER: &str = "x-cma-file-write-not-entered";

#[derive(serde::Deserialize)]
struct EditPrecondition<'a> {
    expected_sha256: Option<&'a str>,
    #[serde(default)]
    directory: bool,
}

pub(super) fn eligible(path: &str, method: &Method, body: &[u8]) -> bool {
    *method == Method::POST
        && path
            .strip_prefix("/api/v1/agents/")
            .and_then(|suffix| suffix.split_once('/'))
            .is_some_and(|(_, operation)| operation == "files")
        && serde_json::from_slice::<EditPrecondition<'_>>(body).is_ok_and(|input| {
            !input.directory
                && input.expected_sha256.is_some_and(|digest| {
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        })
}

pub(super) fn refusal(
    relay: &Relay,
    proof: &[u8],
    diagnostic: Option<&relay_diagnostics::RelayDiagnostic>,
    reason: Refusal,
) -> Response {
    let mut response = http::unavailable_with(diagnostic, reason);
    response.headers_mut().insert(
        NOT_ENTERED_HEADER,
        relay.authentication.write_not_entered_tag(proof),
    );
    response
}
