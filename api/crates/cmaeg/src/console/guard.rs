//! The middleware in front of every console route: it resolves the session
//! cookie, and for every POST it enforces, in this order and before any
//! handler or NyxID call, the exact Origin, a live session and the CSRF
//! token. A forged POST therefore never reaches a handler, and a POST from
//! a foreign origin does not even refresh a session's activity.
use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    extract::{MatchedPath, RawPathParams, Request, State},
    http::Method,
    middleware::Next,
    response::Response,
    RequestPartsExt,
};

use super::{
    cookies, csrf,
    extract::RequestedOwner,
    orgs,
    pages::layout::PERSONAL,
    redirect,
    session::{OwnerSelection, SessionCell},
    Console,
};

/// Console forms are a few short fields.
const MAX_FORM_BYTES: usize = 16 * 1024;

/// The request's live session, if any; inserted for every console request.
#[derive(Clone)]
pub(crate) struct CurrentSession(pub Option<Arc<SessionCell>>);

pub(crate) async fn session_guard(
    State(console): State<Console>,
    request: Request,
    next: Next,
) -> Response {
    let post = request.method() == Method::POST;
    if post && !csrf::origin_matches(request.headers(), console.public_url()) {
        return csrf::reject("origin");
    }
    let session = cookies::get(request.headers(), cookies::SESSION)
        .and_then(|sid| console.sessions().find(sid));
    let mut request = if post {
        let Some(cell) = &session else {
            return redirect::signed_out(true);
        };
        match verified_form(request, cell).await {
            Ok(request) => request,
            Err(response) => return response,
        }
    } else {
        request
    };
    request.extensions_mut().insert(CurrentSession(session));
    next.run(request).await
}

/// Buffer the form, check its CSRF token for this route and record, and hand
/// the same bytes on to the handler.
async fn verified_form(request: Request, cell: &SessionCell) -> Result<Request, Response> {
    let (mut parts, body) = request.into_parts();
    let action = parts
        .extensions
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_default();
    let record = match parts.extract::<RawPathParams>().await {
        Ok(params) => params
            .iter()
            .find_map(|(name, value)| (name == "record_id").then(|| value.to_owned()))
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let bytes = to_bytes(body, MAX_FORM_BYTES)
        .await
        .map_err(|_| csrf::reject("csrf"))?;
    let mut presented = url::form_urlencoded::parse(&bytes)
        .filter(|(name, _)| name == "csrf")
        .map(|(_, value)| value.into_owned());
    let token = match (presented.next(), presented.next()) {
        (Some(token), None) => token,
        _ => return Err(csrf::reject("csrf")),
    };
    let key = cell.meta().csrf_key;
    if action.is_empty() || !csrf::matches(&key, &action, &record, &token) {
        return Err(csrf::reject("csrf"));
    }
    if action == redirect::OWNER {
        // The owner switch's extractor verifies the requested owner afresh.
        if let Some(requested) = requested_owner(&bytes) {
            parts.extensions.insert(RequestedOwner(requested));
        }
    }
    Ok(Request::from_parts(parts, Body::from(bytes)))
}

/// The one `owner` field of the switch form: `personal` or an organization
/// id of the shape `X-CMAEG-Owner` accepts.
fn requested_owner(form: &[u8]) -> Option<OwnerSelection> {
    let mut values = url::form_urlencoded::parse(form)
        .filter(|(name, _)| name == "owner")
        .map(|(_, value)| value.into_owned());
    match (values.next(), values.next()) {
        (Some(value), None) if value == PERSONAL => Some(OwnerSelection::Personal),
        (Some(value), None) if orgs::valid_id(&value) => Some(OwnerSelection::Organization(value)),
        _ => None,
    }
}
